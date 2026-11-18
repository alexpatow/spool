//! `spool serve` and `spool dev`: the server, from a manifest and a store, or
//! straight from a project directory.

use std::hash::{Hash, Hasher};
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use spool::{ApplyReport, Engine, Manifest, Options};
use spool_cli::accounts;
use spool_cli::google::{self, Google};
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
store, named after it: `spool.auth.sqlite` for `spool.duckdb`. While no admin
can sign in, the server prints a link when it starts; whoever opens it makes
the first account and is the admin, who adds everyone else.

Passwords are for getting started. A server that people rely on should sign
them in with Google: with SPOOL_GOOGLE_CLIENT_ID and SPOOL_GOOGLE_CLIENT_SECRET
set to an OAuth client made in Google's console, the sign-in page offers it,
and an admin adds a person by the email of their Google account, which is a
Gmail address or one of an organisation's Google Workspace. The server prints
the redirect URI to give that client when it starts.

A caller that is not a person presents a token, which an admin makes on the
studio's Tokens page and `spool token add` makes on this machine. What it
changes is recorded under the token's name, as `agent:<name>`.

An assistant that speaks the Model Context Protocol, such as Claude or
ChatGPT, is pointed at `/mcp`, where the server offers it tools to look things
up and to change identities. It presents a token as any other caller does,
and what it changes is recorded under that token's name. docs/mcp.md says how
to add the server to each.

Options:
  --manifest <path>   the manifest to run on (default: .spool/manifest.json)
  --store <path>      the store's database file (default: $SPOOL_STORE)
  --listen <addr>     the address to listen on (default: 0.0.0.0:7765)
  --public-url <url>  the address people reach the server by, such as
                      https://spool.example.com (default: $SPOOL_PUBLIC_URL,
                      or the address it listens on when that is on this
                      machine only). Sign-in with Google comes back to it.
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

The server stops on Ctrl-C or SIGTERM, and prints `stopping`. It takes no new
request. Whoever waits on a run with `wait_ms` is answered at once, with the
run as far as it has got. Other requests are answered if that takes no more
than 5 seconds; one that a client has not sent in full by then gets no
answer. A client that was refused while still sending has 1 second more to
read why. So the server is done with its clients within 6 seconds.

It then finishes the runs the engine was given, however long they take, and
prints `stopped`. A run is not cut off half way. If the process is killed
first, the run is recorded as failed when the store is next opened.
";

pub const DEV_USAGE: &str = "\
Usage: spool dev [<dir>] [options]

Runs the server on localhost from a project directory, against a development
store under .spool/. A change to the project's files is applied as it is
saved; a change that does not compile is reported and the last version that
did keeps running.

Open the address it listens on in a browser for the studio, which shows what
the store holds and takes changes. Nobody signs in, and a caller needs no
token: every change is recorded under an account called `dev`, an admin. So
it listens on this machine only, and answers only what is sent to `localhost`
or to a loopback address; `spool serve` runs a server that others reach. A
caller that does present a token writes as that token, as it would there.

An assistant on this machine that speaks the Model Context Protocol, such as
Claude Code, is pointed at `/mcp`, where the server offers it tools to look
things up and to change identities. docs/mcp.md says how.

  <dir>               the project directory (default: the current directory)

Options:
  --listen <addr>     the loopback address to listen on (default: 127.0.0.1:7765)
  --store <path>      the store's database file (default: <dir>/.spool/dev.duckdb)
  --task-timeout <s>  how long a user-code strategy or an extractor may run,
                      in seconds (default: 600)
  --tasks <dir>       where the task directories of user code are made
                      (default: spool-tasks in the temporary directory)

Strategies and extractors that are `.py` files run with `uv`, and `.ts` files
with `bun`. SPOOL_UV and SPOOL_BUN name the commands if they are not on the
search path.

Ctrl-C stops it, the way `spool serve --help` says a server stops.
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
                "--public-url",
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
    let reached = Reached {
        at: public_url(&arguments)?,
        google: GoogleClient::read()?,
    };
    if reached.google.is_some() && reached.at.is_none() && !listen.ip().is_loopback() {
        return Err(Failure::Usage(
            "sign-in with Google comes back to the address people reach this server by: \
             pass it with `--public-url` or in SPOOL_PUBLIC_URL, such as https://spool.example.com"
                .to_owned(),
        ));
    }
    let store = super::store_path(&arguments)?;
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
    let accounts = super::open_accounts(&store)?.with_google(reached.google.is_some());
    say!("accounts  {}", super::accounts_path(&store).display());
    let empty = accounts
        .needs_setup()
        .map_err(|refused| failed(refused.to_string()))?;
    let mut options = ServerOptions::new(accounts);
    options.setup = empty.then(accounts::code);
    options.https = !(arguments.flag("--plain-http") || listen.ip().is_loopback());
    options.studio = !arguments.flag("--no-studio");
    run(Arc::new(engine), listen, options, reached)
}

/// How people reach a server, apart from the address it listens on.
#[derive(Default)]
struct Reached {
    /// The address they type, if it is not the one the server listens on.
    at: Option<String>,
    /// The client the server is to Google, if it signs people in with it.
    google: Option<GoogleClient>,
}

/// An OAuth client made in Google's console.
struct GoogleClient {
    id: String,
    secret: String,
    /// Where Google's sign-in is. A test names what stands in for it.
    issuer: String,
}

impl GoogleClient {
    fn read() -> Result<Option<Self>, Failure> {
        let set = |name: &str| {
            std::env::var(name)
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        };
        const ID: &str = "SPOOL_GOOGLE_CLIENT_ID";
        const SECRET: &str = "SPOOL_GOOGLE_CLIENT_SECRET";
        match (set(ID), set(SECRET)) {
            (Some(id), Some(secret)) => Ok(Some(Self {
                id,
                secret,
                issuer: set("SPOOL_GOOGLE_ISSUER").unwrap_or_else(|| google::ISSUER.to_owned()),
            })),
            (None, None) => Ok(None),
            (id, _) => {
                let (has, lacks) = if id.is_some() {
                    (ID, SECRET)
                } else {
                    (SECRET, ID)
                };
                Err(failed(format!(
                    "sign-in with Google needs both {ID} and {SECRET}: {has} is set and {lacks} is not"
                )))
            }
        }
    }
}

fn public_url(arguments: &Arguments) -> Result<Option<String>, Failure> {
    let given = arguments
        .value("--public-url")
        .map(str::to_owned)
        .or_else(|| std::env::var("SPOOL_PUBLIC_URL").ok())
        .map(|url| url.trim().trim_end_matches('/').to_owned())
        .filter(|url| !url.is_empty());
    match given {
        Some(url) if !(url.starts_with("https://") || url.starts_with("http://")) => {
            Err(Failure::Usage(format!(
                "`--public-url` needs an address such as https://spool.example.com, not `{url}`"
            )))
        }
        given => Ok(given),
    }
}

pub fn dev(arguments: &[String]) -> Result<(), Failure> {
    let arguments = super::arguments(
        arguments,
        &Spec {
            values: &["--listen", "--store", "--seed", "--task-timeout", "--tasks"],
            flags: &[],
            positional: 1,
        },
    )?;
    let dir = arguments.project_dir();
    let listen = listen_address(&arguments, &format!("127.0.0.1:{DEFAULT_PORT}"))?;
    // Whoever reaches a development server is its admin.
    if !listen.ip().is_loopback() {
        return Err(Failure::Usage(format!(
            "`spool dev` has no sign-in, so it listens on this machine only: `--listen` needs \
             an address such as 127.0.0.1:{DEFAULT_PORT}, not `{listen}`. `spool serve` runs a \
             server that others reach, and they sign in to it."
        )));
    }
    let store = arguments
        .path("--store")
        .unwrap_or_else(|| dir.join(".spool").join("dev.duckdb"));
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
    let engine = Engine::open(&store, options(&arguments)?).map_err(|error| match error {
        // The usual cause by far, and what to do about it.
        spool::Error::StoreInUse { path, holder } => failed(format!(
            "the store {} is open in another process{}, most likely a `spool dev` that is \
             already running on this project.\n\
             One server runs on a store at a time. Use the one that is running, or stop it \
             with Ctrl-C and start this one again.",
            path.display(),
            holder.map_or_else(String::new, |pid| format!(" (PID {pid})")),
        )),
        other => other.into(),
    })?;
    let engine = Arc::new(engine);
    engine.set_user_code(user_code);
    let report = engine.apply(&manifest)?;
    say!("project   {}", dir.display());
    say!("store     {}", store.display());
    print_applied(&manifest, &report);

    let mut options = ServerOptions::new(super::open_accounts(&store)?);
    say!("accounts  {}", super::accounts_path(&store).display());
    options.dev = true;
    options.https = false;

    let watched = Arc::clone(&engine);
    std::thread::spawn(move || watch(&dir, &watched, seen));
    run(engine, listen, options, Reached::default())
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

fn run(
    engine: Arc<Engine>,
    listen: SocketAddr,
    mut options: ServerOptions,
    reached: Reached,
) -> Result<(), Failure> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| failed(format!("the server could not start: {error}")))?;
    runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind(listen)
            .await
            .map_err(|error| failed(cannot_listen(listen, &error, options.dev)))?;
        let address = listener
            .local_addr()
            .map_err(|error| failed(error.to_string()))?;
        // Tests listen on port 0 and read the port from this line.
        say!("listening on http://{address}");
        let at = reached.at.unwrap_or_else(|| format!("http://{address}"));
        if options.studio {
            say!("studio    {at}/");
        }
        if let Some(client) = reached.google {
            let google =
                Google::new(&client.issuer, &client.id, &client.secret, &at).map_err(|error| {
                    failed(format!("sign-in with Google cannot be set up: {error}"))
                })?;
            say!("google    {}", google.redirect());
            say!(
                "people sign in with Google: that address is the redirect URI of the OAuth \
                 client in Google's console"
            );
            options.google = Some(google);
        }
        say!("mcp       http://{address}{}", server::MCP_PATH);
        match &options.setup {
            Some(code) if options.studio => {
                say!("setup     {at}/setup#{code}");
                say!(
                    "no admin can sign in yet: open that link, at the address people reach \
                     this server by, to make the first account"
                );
            }
            // No page asks for it: the code is sent to the API.
            Some(code) => {
                say!("setup     {code}");
                say!(
                    "no admin can sign in yet: `POST /auth/setup` with that code makes the \
                     first account"
                );
            }
            None => {}
        }
        server::serve(listener, engine, &options, shutdown())
            .await
            .map_err(|error| failed(format!("the server stopped: {error}")))
    })?;
    // This ends the connections that were not done in time, and waits for
    // the work the engine was given: a run is not cut off half way.
    drop(runtime);
    say!("stopped");
    Ok(())
}

/// Why the server could not listen on `listen`. A port that is taken is
/// said in words, with another port to try: the next one up.
fn cannot_listen(listen: SocketAddr, error: &std::io::Error, dev: bool) -> String {
    if error.kind() != std::io::ErrorKind::AddrInUse {
        return format!("cannot listen on {listen}: {error}");
    }
    let mut other = listen;
    other.set_port(match listen.port() {
        u16::MAX => u16::MAX - 1,
        port => port + 1,
    });
    if dev {
        format!(
            "cannot listen on {listen}: something else is listening there, perhaps a \
             `spool dev` on another project.\n\
             Give this one another port by adding `--listen {other}` to the command, and \
             point the commands that call it there with SPOOL_URL=http://{other}."
        )
    } else {
        format!(
            "cannot listen on {listen}: something else is listening there.\n\
             Give this server another port by adding `--listen {other}` to the command."
        )
    }
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
    say!("stopping");
}
