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

use clap::Args;

use super::UserCodeArgs;
use crate::Failure;

/// Run the server from a manifest and a store
///
/// Runs the production server. It's the only process that opens the store.
///
/// An empty store is set up from the manifest. A store applied with a
/// different manifest is refused: run `spool apply` on it first.
///
/// A browser at the server's address gets the studio, Spool's web UI. People
/// sign in with a username and password, and their accounts live in a file
/// beside the store: `spool.auth.sqlite` for `spool.duckdb`. Until an admin
/// exists, the server prints a link on startup. Whoever opens it creates the
/// first account and becomes the admin who adds everyone else.
///
/// For a server people rely on, use Google sign-in. Set
/// SPOOL_GOOGLE_CLIENT_ID and SPOOL_GOOGLE_CLIENT_SECRET to an OAuth client
/// from Google's console, and the sign-in page offers it. An admin then adds
/// people by the email of their Google account, Gmail or Google Workspace.
/// The server prints the redirect URI for that client on startup.
///
/// Programs authenticate with a token, which an admin creates on the studio's
/// Tokens page or with `spool token add` on this machine. Changes are
/// recorded under the token's name, as `agent:<name>`.
///
/// Agents that speak the Model Context Protocol, such as Claude or ChatGPT,
/// connect at `/mcp`. They get tools to load rows and documents, match,
/// look things up and change identities, and they authenticate with a token
/// like any other program.
///
/// Strategies and extractors run with `uv` for `.py` files and `bun` for
/// `.ts` files. Set SPOOL_UV and SPOOL_BUN if they aren't on your PATH. The
/// server only runs a file that matches the one the manifest was compiled
/// from. It passes the variables the project's .env sets, except those
/// already set in the server's environment.
///
/// On Ctrl-C or SIGTERM the server prints `stopping` and takes no new
/// requests. Callers waiting on a run with `wait_ms` get the run as it
/// stands. Other requests get up to 5 seconds to finish, plus 1 second for a
/// refused client to read why, so clients are done within 6 seconds.
///
/// It then finishes every run it was given, however long they take, and
/// prints `stopped`. Runs are never cut off halfway. If the process is
/// killed first, the run is marked failed the next time the store opens.
#[derive(Args)]
pub struct ServeArgs {
    /// The manifest to run on
    #[arg(long, value_name = "PATH", default_value = ".spool/manifest.json")]
    manifest: PathBuf,
    /// The store's database file
    #[arg(long, env = "SPOOL_STORE", hide_env_values = true, value_name = "PATH", value_parser = super::any_path())]
    store: Option<PathBuf>,
    /// The address to listen on
    #[arg(long, value_name = "ADDR", default_value = "0.0.0.0:7765")]
    listen: SocketAddr,
    /// The address people reach the server by, such as https://spool.example.com
    ///
    /// The default is the address it listens on when that is on this machine
    /// only. Sign-in with Google comes back to it.
    #[arg(
        long,
        env = "SPOOL_PUBLIC_URL",
        hide_env_values = true,
        value_name = "URL"
    )]
    public_url: Option<String>,
    /// The project directory, where the files the manifest names are found [default: the current directory]
    #[arg(long, value_name = "DIR")]
    project: Option<PathBuf>,
    /// Do not serve the studio
    #[arg(long)]
    no_studio: bool,
    /// Browsers reach the server over HTTP, not HTTPS
    ///
    /// The session cookie may then travel over HTTP. It is the default when
    /// the server listens on this machine only.
    #[arg(long)]
    plain_http: bool,
    /// Makes IDs and timestamps reproducible, for tests
    #[arg(long, hide = true)]
    seed: Option<u64>,
    #[command(flatten)]
    user_code: UserCodeArgs,
}

/// Run a server on localhost that applies file changes as they are saved
///
/// Runs a development server straight from a project directory, with its
/// store under .spool/. Project files are applied as you save them. A change
/// that doesn't compile is reported, and the last good version keeps running.
///
/// Open its address in a browser for the studio. There's no sign-in and no
/// token: every change is recorded under an admin account called `dev`. That's
/// why it only listens on, and answers, localhost. Use `spool serve` for a
/// server other people reach. A caller that does send a token writes as that
/// token.
///
/// Agents on this machine that speak the Model Context Protocol, such as
/// Claude Code, connect at `/mcp`, with tools to load rows and documents,
/// match, look things up and change identities.
///
/// Strategies and extractors run with `uv` for `.py` files and `bun` for
/// `.ts` files. Set SPOOL_UV and SPOOL_BUN if they aren't on your PATH. They
/// get the variables the project's .env sets, except those already set in
/// the server's environment, and .env is reread each time one starts.
///
/// Ctrl-C stops it, as described in `spool serve --help`.
#[derive(Args)]
pub struct DevArgs {
    /// The project directory [default: the current directory]
    dir: Option<PathBuf>,
    /// The loopback address to listen on
    #[arg(long, value_name = "ADDR", default_value = "127.0.0.1:7765")]
    listen: SocketAddr,
    /// The store's database file [default: <DIR>/.spool/dev.duckdb]
    #[arg(long, value_name = "PATH")]
    store: Option<PathBuf>,
    /// Makes IDs and timestamps reproducible, for tests
    #[arg(long, hide = true)]
    seed: Option<u64>,
    #[command(flatten)]
    user_code: UserCodeArgs,
}

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
    Failure::failed(message.into())
}

/// IDs and the clock: the system's, or reproducible ones for tests.
fn options(seed: Option<u64>) -> Options {
    match seed {
        Some(seed) => Options::seeded(seed),
        None => Options::default(),
    }
}

pub fn serve(arguments: ServeArgs) -> Result<(), Failure> {
    let listen = arguments.listen;
    let reached = Reached {
        at: public_url(arguments.public_url.as_deref())?,
        google: GoogleClient::read()?,
    };
    if reached.google.is_some() && reached.at.is_none() && !listen.ip().is_loopback() {
        return Err(Failure::usage(
            "sign-in with Google comes back to the address people reach this server by: \
             pass it with `--public-url` or in SPOOL_PUBLIC_URL, such as https://spool.example.com"
                .to_owned(),
        ));
    }
    let store = super::store_path(&arguments.store)?;
    let manifest_file = arguments.manifest.clone();
    let manifest = Manifest::read_file(&manifest_file)?;
    let project = super::project_dir(&arguments.project);
    let user_code = super::user_code(Some(&arguments.user_code), &project);

    let engine = Engine::open(&store, options(arguments.seed))?;
    engine.set_user_code(user_code);
    match engine.applied_manifest_hash()? {
        None => {
            engine.apply(&manifest)?;
            say!("set up an empty store at {}", store.display());
        }
        Some(applied) if applied == manifest.hash => engine.adopt(&manifest)?,
        Some(applied) => {
            return Err(failed(format!(
                "the store at {} was applied with manifest {applied}, and this server runs on \
                 manifest {}",
                store.display(),
                manifest.hash,
            ))
            .hint(format!(
                "applying a changed project is a step of its own: run `spool apply --store {} \
                 --manifest {}` and start again",
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
    options.https = !(arguments.plain_http || listen.ip().is_loopback());
    options.studio = !arguments.no_studio;
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

fn public_url(given: Option<&str>) -> Result<Option<String>, Failure> {
    let given = given
        .map(|url| url.trim().trim_end_matches('/').to_owned())
        .filter(|url| !url.is_empty());
    match given {
        Some(url) if !(url.starts_with("https://") || url.starts_with("http://")) => {
            Err(Failure::usage(format!(
                "`--public-url` needs an address such as https://spool.example.com, not `{url}`"
            )))
        }
        given => Ok(given),
    }
}

pub fn dev(arguments: DevArgs) -> Result<(), Failure> {
    let dir = super::project_dir(&arguments.dir);
    let listen = arguments.listen;
    // Whoever reaches a development server is its admin.
    if !listen.ip().is_loopback() {
        return Err(Failure::usage(format!(
            "`spool dev` has no sign-in, so it listens on this machine only: `--listen` needs \
             an address such as 127.0.0.1:{DEFAULT_PORT}, not `{listen}`. `spool serve` runs a \
             server that others reach, and they sign in to it."
        )));
    }
    let store = arguments
        .store
        .clone()
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
    let user_code = super::user_code(Some(&arguments.user_code), &dir);
    let engine = Engine::open(&store, options(arguments.seed)).map_err(|error| match error {
        // The usual cause by far, and what to do about it.
        spool::Error::StoreInUse { path, holder } => failed(format!(
            "the store {} is open in another process{}, most likely a `spool dev` that is \
             already running on this project",
            path.display(),
            holder.map_or_else(String::new, |pid| format!(" (PID {pid})")),
        ))
        .hint(
            "one server runs on a store at a time: use the one that is running, or stop it \
             with Ctrl-C and start this one again",
        ),
        // A store that another version of `spool` made.
        spool::Error::FormatVersion { found, expected } => failed(format!(
            "the store {} has format version {found}, and this `spool` reads only version \
             {expected}: it does not convert a store",
            store.display(),
        ))
        .hint("move the file away or name another with `--store`, then load the rows again"),
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
                    "error: {error}\nnote: still running the last version that compiled"
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
            .map_err(|error| cannot_listen(listen, &error, options.dev))?;
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
fn cannot_listen(listen: SocketAddr, error: &std::io::Error, dev: bool) -> Failure {
    if error.kind() != std::io::ErrorKind::AddrInUse {
        return failed(format!("cannot listen on {listen}: {error}"));
    }
    let mut other = listen;
    other.set_port(match listen.port() {
        u16::MAX => u16::MAX - 1,
        port => port + 1,
    });
    if dev {
        failed(format!(
            "cannot listen on {listen}: something else is listening there, perhaps a \
             `spool dev` on another project"
        ))
        .hint(format!(
            "give this one another port by adding `--listen {other}` to the command, and point \
             the commands that call it there with SPOOL_URL=http://{other}"
        ))
    } else {
        failed(format!(
            "cannot listen on {listen}: something else is listening there"
        ))
        .hint(format!(
            "give this server another port by adding `--listen {other}` to the command"
        ))
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
