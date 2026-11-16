//! `spool serve` and `spool dev`: the server, from a manifest and a store, or
//! straight from a project directory.

use std::hash::{Hash, Hasher};
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use spool::{ApplyReport, Engine, Manifest, Options};
use spool_cli::server::{self, ServerOptions};

use super::manifest_path;
use crate::Failure;
use crate::args::{Arguments, Spec};

pub const SERVE_USAGE: &str = "\
Usage: spool serve [options]

Runs the server. It is the only process that opens the store, and it needs a
bearer token in SPOOL_TOKEN, which every call must carry.

An empty store is set up from the manifest. A store that was applied with
another manifest is refused: run `spool apply` on it first.

Options:
  --manifest <path>   the manifest to run on (default: .spool/manifest.json)
  --store <path>      the store's database file (default: $SPOOL_STORE)
  --listen <addr>     the address to listen on (default: 0.0.0.0:7765)
";

pub const DEV_USAGE: &str = "\
Usage: spool dev [<dir>] [options]

Runs the server on localhost from a project directory, against a development
store under .spool/. A change to the project's files is applied as it is
saved; a change that does not compile is reported and the last version that
did keeps running.

  <dir>               the project directory (default: the current directory)

Options:
  --listen <addr>     the address to listen on (default: 127.0.0.1:7765)
  --store <path>      the store's database file (default: <dir>/.spool/dev.duckdb)
  --token <token>     the bearer token (default: $SPOOL_TOKEN, or `dev`)
";

const DEFAULT_PORT: u16 = 7765;

fn failed(message: impl Into<String>) -> Failure {
    Failure::Failed(message.into())
}

fn listen_address(arguments: &Arguments, default: &str) -> Result<SocketAddr, Failure> {
    let address = arguments.value("--listen").unwrap_or(default);
    address.parse().map_err(|_| {
        Failure::Usage(format!(
            "`--listen` needs an address such as 127.0.0.1:{DEFAULT_PORT}, not `{address}`"
        ))
    })
}

/// IDs and the clock: the system's, or reproducible ones for tests.
fn options(arguments: &Arguments) -> Result<Options, Failure> {
    Ok(match arguments.number("--seed").map_err(Failure::Usage)? {
        Some(seed) => Options::seeded(seed),
        None => Options::default(),
    })
}

pub fn serve(arguments: &[String]) -> Result<(), Failure> {
    let arguments = super::arguments(
        arguments,
        &Spec {
            // `--seed` is for tests: it makes IDs and timestamps reproducible.
            values: &["--manifest", "--store", "--listen", "--seed"],
            flags: &[],
            positional: 0,
        },
    )?;
    let listen = listen_address(&arguments, &format!("0.0.0.0:{DEFAULT_PORT}"))?;
    let store = super::store_path(&arguments)?;
    let token = std::env::var("SPOOL_TOKEN")
        .ok()
        .filter(|token| !token.trim().is_empty())
        .ok_or_else(|| {
            failed("set SPOOL_TOKEN to the bearer token callers must present; the server does not run without one")
        })?;
    let manifest_file = arguments
        .path("--manifest")
        .unwrap_or_else(|| manifest_path(Path::new(".")));
    let manifest = Manifest::read_file(&manifest_file)?;

    let engine = Engine::open(&store, options(&arguments)?)?;
    match engine.applied_manifest_hash()? {
        None => {
            engine.apply(&manifest)?;
            println!("set up an empty store at {}", store.display());
        }
        Some(applied) if applied == manifest.hash => engine.adopt(&manifest)?,
        Some(applied) => {
            return Err(failed(format!(
                "the store at {} was applied with manifest {applied}, and this server runs on manifest {}.\n\
                 Applying a changed project is a step of its own: run `spool apply --store {} --manifest {}` and start again.",
                store.display(),
                manifest.hash,
                store.display(),
                manifest_file.display(),
            )));
        }
    }
    println!("manifest  {}", manifest.hash);
    run(Arc::new(engine), listen, ServerOptions::new(token))
}

pub fn dev(arguments: &[String]) -> Result<(), Failure> {
    let arguments = super::arguments(
        arguments,
        &Spec {
            values: &["--listen", "--store", "--token", "--seed"],
            flags: &[],
            positional: 1,
        },
    )?;
    let dir = arguments.project_dir();
    let listen = listen_address(&arguments, &format!("127.0.0.1:{DEFAULT_PORT}"))?;
    let store = arguments
        .path("--store")
        .unwrap_or_else(|| dir.join(".spool").join("dev.duckdb"));
    let token = arguments
        .value("--token")
        .map(str::to_owned)
        .or_else(|| std::env::var("SPOOL_TOKEN").ok())
        .filter(|token| !token.trim().is_empty())
        .unwrap_or_else(|| "dev".to_owned());

    let seen = fingerprint(&dir);
    let manifest = Manifest::load(&dir)?;
    if let Some(parent) = store
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .map_err(|error| failed(format!("{}: {error}", parent.display())))?;
    }
    let engine = Arc::new(Engine::open(&store, options(&arguments)?)?);
    let report = engine.apply(&manifest)?;
    println!("project   {}", dir.display());
    println!("store     {}", store.display());
    println!("token     {token}");
    print_applied(&manifest, &report);

    let watched = Arc::clone(&engine);
    std::thread::spawn(move || watch(&dir, &watched, seen));
    run(engine, listen, ServerOptions::new(token))
}

fn print_applied(manifest: &Manifest, report: &ApplyReport) {
    let names = |types: &[spool::core::EntityType]| {
        types
            .iter()
            .map(|name| name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut parts = Vec::new();
    if !report.declared.is_empty() {
        parts.push(format!("declared {}", names(&report.declared)));
    }
    if !report.rebuilt.is_empty() {
        parts.push(format!("rebuilt {}", names(&report.rebuilt)));
    }
    if parts.is_empty() {
        parts.push("rules only".to_owned());
    }
    println!("applied   {} ({})", manifest.hash, parts.join("; "));
    let _ = std::io::stdout().flush();
}

/// Applies the project again whenever its files change. A project that does
/// not compile is reported, and the engine keeps the last one that did.
fn watch(dir: &Path, engine: &Engine, mut seen: u64) {
    loop {
        std::thread::sleep(Duration::from_millis(300));
        let now = fingerprint(dir);
        if now == seen {
            continue;
        }
        seen = now;
        let applied = Manifest::load(dir).and_then(|manifest| {
            if engine
                .manifest()
                .is_some_and(|current| current.hash == manifest.hash)
            {
                return Ok(None);
            }
            let report = engine.apply(&manifest)?;
            Ok(Some((manifest, report)))
        });
        match applied {
            Ok(Some((manifest, report))) => print_applied(&manifest, &report),
            Ok(None) => {}
            Err(error) => {
                eprintln!("spool: {error}\nspool: still running the last version that compiled");
            }
        }
    }
}

/// A number that changes when a file under `entities/` is added, removed or
/// edited. Contents are read, not timestamps: projects are small, and two
/// saves within a timestamp's resolution must both be seen.
fn fingerprint(dir: &Path) -> u64 {
    fn visit(path: &Path, hasher: &mut impl Hasher) {
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        let mut entries: Vec<PathBuf> = entries
            .filter_map(|entry| Some(entry.ok()?.path()))
            .collect();
        entries.sort();
        for entry in entries {
            entry.hash(hasher);
            if entry.is_dir() {
                visit(&entry, hasher);
            } else if let Ok(contents) = std::fs::read(&entry) {
                contents.hash(hasher);
            }
        }
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    visit(&dir.join("entities"), &mut hasher);
    hasher.finish()
}

fn run(engine: Arc<Engine>, listen: SocketAddr, options: ServerOptions) -> Result<(), Failure> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| failed(format!("the server could not start: {error}")))?;
    runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind(listen)
            .await
            .map_err(|error| failed(format!("cannot listen on {listen}: {error}")))?;
        let address = listener
            .local_addr()
            .map_err(|error| failed(error.to_string()))?;
        // Tests listen on port 0 and read the port from this line.
        println!("listening on http://{address}");
        let _ = std::io::stdout().flush();
        server::serve(listener, engine, &options, shutdown())
            .await
            .map_err(|error| failed(format!("the server stopped: {error}")))
    })?;
    println!("stopped");
    Ok(())
}

/// Completes when the process is asked to stop: Ctrl-C, or SIGTERM from
/// whatever runs the container.
async fn shutdown() {
    let interrupt = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = interrupt => {}
        () = terminate => {}
    }
}
