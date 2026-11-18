//! `spool serve` and `spool dev`: the server, from a manifest and a store, or
//! straight from a project directory.

use std::hash::{Hash, Hasher};
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use spool::{ApplyReport, Engine, Manifest, Options};
use spool_cli::accounts::{self, Accounts};
use spool_cli::server::{self, ServerOptions};

use super::manifest_path;
use crate::Failure;
use crate::args::{Arguments, Spec};

pub const SERVE_USAGE: &str = "\
Usage: spool serve [options]

Runs the server. It is the only process that opens the store.

An empty store is set up from the manifest. A store that was applied with
another manifest is refused: run `spool apply` on it first.

The server answers a browser at its address with the studio, which shows what
the store holds and takes the changes of whoever is signed in. People sign in
with a username and a password. Their accounts are kept in a file beside the
store, named after it: `spool.auth.sqlite` for `spool.duckdb`. While nobody
has an account, the server prints a link when it starts; whoever opens it
makes the first account and is the admin, who adds everyone else.

A caller that is not a person sends the bearer token in SPOOL_TOKEN, if one
is set. What it changes is recorded under `agent:api`.

Options:
  --manifest <path>   the manifest to run on (default: .spool/manifest.json)
  --store <path>      the store's database file (default: $SPOOL_STORE)
  --listen <addr>     the address to listen on (default: 0.0.0.0:7765)
  --project <dir>     the project directory, where the strategy and extractor
                      files the manifest names are found (default: the
                      current directory)
  --task-timeout <s>  how long a user-code strategy or an extractor may run,
                      in seconds (default: 600)
  --tasks <dir>       where the task directories of user code are made
                      (default: spool-tasks in the temporary directory)
  --no-studio         do not serve the studio
  --plain-http        browsers reach the server over HTTP, not HTTPS, so the
                      session cookie may travel over HTTP (the default when
                      listening on this machine only)

Strategies and extractors that are `.py` files run with `uv`, and `.ts` files
with `bun`. SPOOL_UV and SPOOL_BUN name the commands if they are not on the
search path. The server runs such a file only while it is the one the
manifest was compiled from.
";

pub const DEV_USAGE: &str = "\
Usage: spool dev [<dir>] [options]

Runs the server on localhost from a project directory, against a development
store under .spool/. A change to the project's files is applied as it is
saved; a change that does not compile is reported and the last version that
did keeps running.

Open the address it listens on in a browser for the studio, which shows what
the store holds and takes changes. Nobody signs in: every change is recorded
under an account called `dev`.

  <dir>               the project directory (default: the current directory)

Options:
  --listen <addr>     the address to listen on (default: 127.0.0.1:7765)
  --store <path>      the store's database file (default: <dir>/.spool/dev.duckdb)
  --token <token>     the bearer token (default: $SPOOL_TOKEN, or `dev`)
  --task-timeout <s>  how long a user-code strategy or an extractor may run,
                      in seconds (default: 600)
  --tasks <dir>       where the task directories of user code are made
                      (default: spool-tasks in the temporary directory)

Strategies and extractors that are `.py` files run with `uv`, and `.ts` files
with `bun`. SPOOL_UV and SPOOL_BUN name the commands if they are not on the
search path.
";

/// Prints a line, and carries on if nobody is reading: a server must not
/// stop because its output was closed.
macro_rules! say {
    ($($argument:tt)*) => {{
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, $($argument)*);
        let _ = out.flush();
    }};
}

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
            values: &[
                "--manifest",
                "--store",
                "--listen",
                "--seed",
                "--project",
                "--task-timeout",
                "--tasks",
            ],
            flags: &["--no-studio", "--plain-http"],
            positional: 0,
        },
    )?;
    let listen = listen_address(&arguments, &format!("0.0.0.0:{DEFAULT_PORT}"))?;
    let store = super::store_path(&arguments)?;
    // Trimmed, as the token a caller presents is: a secret read from a file
    // often ends with a newline.
    let token = std::env::var("SPOOL_TOKEN")
        .ok()
        .map(|token| token.trim().to_owned())
        .filter(|token| !token.is_empty());
    let manifest_file = arguments
        .path("--manifest")
        .unwrap_or_else(|| manifest_path(Path::new(".")));
    let manifest = Manifest::read_file(&manifest_file)?;
    let project = arguments
        .path("--project")
        .unwrap_or_else(|| PathBuf::from("."));
    let user_code = super::user_code(&arguments, &project)?;

    let engine = Engine::open(&store, options(&arguments)?)?;
    engine.set_user_code(user_code);
    match engine.applied_manifest_hash()? {
        None => {
            engine.apply(&manifest)?;
            say!("set up an empty store at {}", store.display());
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
    say!("manifest  {}", manifest.hash);
    let accounts = open_accounts(&store)?;
    let empty = accounts
        .is_empty()
        .map_err(|refused| failed(refused.to_string()))?;
    let mut options = ServerOptions::new(accounts);
    options.token = token;
    options.setup = empty.then(accounts::code);
    options.https = !(arguments.flag("--plain-http") || listen.ip().is_loopback());
    options.studio = !arguments.flag("--no-studio");
    run(Arc::new(engine), listen, options)
}

pub fn dev(arguments: &[String]) -> Result<(), Failure> {
    let arguments = super::arguments(
        arguments,
        &Spec {
            values: &[
                "--listen",
                "--store",
                "--token",
                "--seed",
                "--task-timeout",
                "--tasks",
            ],
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
        .map(|token| token.trim().to_owned())
        .filter(|token| !token.is_empty())
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
    let user_code = super::user_code(&arguments, &dir)?;
    let engine = Arc::new(Engine::open(&store, options(&arguments)?)?);
    engine.set_user_code(user_code);
    let report = engine.apply(&manifest)?;
    say!("project   {}", dir.display());
    say!("store     {}", store.display());
    say!("token     {token}");
    print_applied(&manifest, &report);

    let mut options = ServerOptions::new(open_accounts(&store)?);
    options.token = Some(token);
    options.dev = true;
    options.https = false;

    let watched = Arc::clone(&engine);
    std::thread::spawn(move || watch(&dir, &watched, seen));
    run(engine, listen, options)
}

/// The accounts of the server over `store`: a file beside it, named after it.
fn open_accounts(store: &Path) -> Result<Accounts, Failure> {
    let path = store.with_extension("auth.sqlite");
    let accounts = Accounts::open(&path)
        .map_err(|refused| failed(format!("{}: {refused}", path.display())))?;
    say!("accounts  {}", path.display());
    Ok(accounts)
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
    say!("applied   {} ({})", manifest.hash, parts.join("; "));
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
                let _ = writeln!(
                    std::io::stderr(),
                    "spool: {error}\nspool: still running the last version that compiled"
                );
            }
        }
    }
}

/// A number that changes when one of the project's files is added, removed
/// or edited. Contents are read, not timestamps: projects are small, and two
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
    for part in ["sources", "transforms", "entities", "extractors"] {
        visit(&dir.join(part), &mut hasher);
    }
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
        say!("listening on http://{address}");
        if options.studio {
            say!("studio    http://{address}/");
        }
        match &options.setup {
            Some(code) if options.studio => {
                say!("setup     http://{address}/setup#{code}");
                say!(
                    "nobody has an account yet: open that link, at the address people reach \
                     this server by, to make the first one"
                );
            }
            // No page asks for it: the code is sent to the API.
            Some(code) => {
                say!("setup     {code}");
                say!(
                    "nobody has an account yet: `POST /auth/setup` with that code makes the \
                     first one"
                );
            }
            None => {}
        }
        server::serve(listener, engine, &options, shutdown())
            .await
            .map_err(|error| failed(format!("the server stopped: {error}")))
    })?;
    say!("stopped");
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
