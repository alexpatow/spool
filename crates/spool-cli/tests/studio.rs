//! The studio: the page the server answers a browser with.

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;

use support::*;

fn repository() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The files, with the path and the media type each is served at.
const FILES: [(&str, &str, &str); 9] = [
    ("/", "index.html", "text/html; charset=utf-8"),
    ("/studio.js", "studio.js", "text/javascript; charset=utf-8"),
    ("/studio.css", "studio.css", "text/css; charset=utf-8"),
    (
        "/hanken-grotesk-latin-wght-normal.woff2",
        "hanken-grotesk-latin-wght-normal.woff2",
        "font/woff2",
    ),
    (
        "/hanken-grotesk-latin-ext-wght-normal.woff2",
        "hanken-grotesk-latin-ext-wght-normal.woff2",
        "font/woff2",
    ),
    (
        "/newsreader-latin-opsz-normal.woff2",
        "newsreader-latin-opsz-normal.woff2",
        "font/woff2",
    ),
    (
        "/newsreader-latin-ext-opsz-normal.woff2",
        "newsreader-latin-ext-opsz-normal.woff2",
        "font/woff2",
    ),
    (
        "/commit-mono-latin-400-normal.woff2",
        "commit-mono-latin-400-normal.woff2",
        "font/woff2",
    ),
    (
        "/commit-mono-latin-700-normal.woff2",
        "commit-mono-latin-700-normal.woff2",
        "font/woff2",
    ),
];

fn built(file: &str) -> Vec<u8> {
    std::fs::read(repository().join("crates/spool-cli/src/studio").join(file)).unwrap()
}

fn serve_things(dir: &Path, more: &[&str]) -> Server {
    write_things(dir);
    let manifest = build(dir);
    let store = dir.join("store.duckdb");
    let mut arguments = vec![
        "serve",
        "--manifest",
        path(&manifest),
        "--store",
        path(&store),
        "--listen",
        "127.0.0.1:0",
    ];
    arguments.extend(more);
    Server::start(&arguments)
}

/// What the studio takes from the site, as `(site, studio)` under each's `src`.
const SHARED: [(&str, &str); 5] = [
    (
        "styles/shelf/tokens.stylex.ts",
        "styles/shelf/tokens.stylex.ts",
    ),
    ("styles/shelf/themes.ts", "styles/shelf/themes.ts"),
    (
        "styles/shelf/conditions.stylex.ts",
        "styles/shelf/conditions.stylex.ts",
    ),
    ("styles/shelf/fonts.css", "styles/shelf/fonts.css"),
    ("components/brand.tsx", "brand.tsx"),
];

#[test]
fn the_studio_looks_as_the_site_does_from_the_sites_own_files() {
    for (site, studio) in SHARED {
        let theirs = std::fs::read_to_string(repository().join("site/src").join(site)).unwrap();
        let ours =
            std::fs::read_to_string(repository().join("packages/studio/src").join(studio)).unwrap();
        // The copy says where it came from, and then is the site's file.
        assert!(
            ours.contains(&format!("site/src/{site}")) && ours.ends_with(&theirs),
            "packages/studio/src/{studio} is not site/src/{site}: run `bun run build` in packages/studio"
        );
    }
}

#[test]
fn the_server_answers_a_browser_with_the_studio_and_asks_no_token_for_it() {
    let dir = tempfile::tempdir().unwrap();
    let server = serve_things(dir.path(), &[]);
    assert_eq!(
        server.printed("studio"),
        format!("studio    http://{}/", server.address)
    );

    for (at, file, media_type) in FILES {
        // A browser that opens the address has no token to send.
        let response = server.request("GET", at).without("Authorization").send();
        assert_eq!(response.status, 200, "{at}");
        assert_eq!(response.header("content-type"), Some(media_type), "{at}");
        assert!(response.body == built(file), "{at} is not {file}");
    }

    // The page names its script and its style sheet from the root, so it
    // finds them at the address of any screen, and it has nothing inline
    // that its policy would have to allow.
    let page = text(&built("index.html"));
    assert!(page.contains(r#"src="/studio.js""#), "{page}");
    assert!(page.contains(r#"href="/studio.css""#), "{page}");
    assert!(!page.contains("<style"), "{page}");
    assert_eq!(page.matches("<script").count(), 1, "{page}");

    // The style sheet names its fonts from the root too, and names no file
    // the server does not have.
    let style = text(&built("studio.css"));
    let mut named: Vec<&str> = style
        .split("url(")
        .skip(1)
        .map(|rest| rest.split(')').next().unwrap().trim_matches('"'))
        .collect();
    named.sort_unstable();
    let mut fonts: Vec<String> = FILES
        .iter()
        .filter(|(_, _, media_type)| *media_type == "font/woff2")
        .map(|(at, _, _)| (*at).to_owned())
        .collect();
    fonts.sort_unstable();
    assert_eq!(named, fonts);

    // What the store holds needs a caller the server knows.
    server
        .request("GET", "/v1/info")
        .without("Authorization")
        .send()
        .error(401, "unauthorized");
    server.read("/v1/info");
}

#[test]
fn the_studio_may_load_nothing_but_itself_and_call_nothing_but_its_server() {
    let dir = tempfile::tempdir().unwrap();
    let server = serve_things(dir.path(), &[]);

    for (at, _, _) in FILES {
        let response = server.request("GET", at).without("Authorization").send();
        let policy = response.header("content-security-policy").unwrap();
        let directives: Vec<&str> = policy.split(';').map(str::trim).collect();
        assert_eq!(
            directives,
            [
                "default-src 'none'",
                "script-src 'self'",
                "style-src 'self'",
                "font-src 'self'",
                "img-src 'self' data:",
                "connect-src 'self'",
                "base-uri 'none'",
                "form-action 'none'",
                "frame-ancestors 'none'",
            ],
            "{at}"
        );
        assert_eq!(response.header("x-content-type-options"), Some("nosniff"));
        assert_eq!(response.header("referrer-policy"), Some("no-referrer"));
        // No other origin is told it may read the server.
        assert_eq!(response.header("access-control-allow-origin"), None);
    }
    assert_eq!(
        server.get("/v1/info").header("access-control-allow-origin"),
        None
    );
}

#[test]
fn a_browser_is_told_when_the_studio_it_has_is_still_the_one() {
    let dir = tempfile::tempdir().unwrap();
    let server = serve_things(dir.path(), &[]);

    let mut tags = Vec::new();
    for (at, _, media_type) in FILES {
        let first = server.request("GET", at).send();
        // It asks each time, so a new binary's studio is the one it shows.
        assert_eq!(first.header("cache-control"), Some("no-cache"), "{at}");
        let tag = first.header("etag").unwrap().to_owned();
        assert!(
            tag.len() == 34 && tag.starts_with('"') && tag.ends_with('"'),
            "{tag}"
        );
        assert_eq!(server.request("GET", at).send().header("etag"), Some(&*tag));

        for sent in [
            tag.clone(),
            format!("W/{tag}"),
            format!("\"another\", {tag}"),
            "*".to_owned(),
        ] {
            let again = server
                .request("GET", at)
                .header("If-None-Match", &sent)
                .send();
            assert_eq!(again.status, 304, "{at} {sent}");
            assert!(again.body.is_empty(), "{at} {sent}");
            assert_eq!(again.header("etag"), Some(&*tag), "{at} {sent}");
            assert_eq!(again.header("content-type"), Some(media_type));
            assert!(again.header("content-security-policy").is_some());
        }

        let other = server
            .request("GET", at)
            .header("If-None-Match", "\"0123456789abcdef0123456789abcdef\"")
            .send();
        assert_eq!(other.status, 200, "{at}");
        assert!(!other.body.is_empty(), "{at}");
        tags.push(tag);
    }
    tags.sort();
    tags.dedup();
    assert_eq!(tags.len(), FILES.len(), "each file has its own tag");
}

#[test]
fn the_studio_is_its_own_paths_and_takes_only_reads() {
    let dir = tempfile::tempdir().unwrap();
    let server = serve_things(dir.path(), &[]);

    for (at, _, _) in FILES {
        let response = server.request("POST", at).body(b"{}").send();
        response.error(405, "invalid");
        let head = server.request("HEAD", at).send();
        assert_eq!(head.status, 200, "{at}");
        assert!(head.body.is_empty(), "{at}");
    }
    // Every other path is still nothing.
    for at in [
        "/index.html",
        "/studio",
        "/studio.js.map",
        "/commit-mono-latin-400-normal.woff",
        "/fonts/hanken-grotesk-latin-wght-normal.woff2",
        // The studio's typeface before it took the site's.
        "/geist-latin-wght-normal.woff2",
        "/record",
        "/runs/",
        "/runsheet",
        "/favicon.ico",
        "/v1",
        "/v1/",
        "/v1/studio.js",
        "/healthz/runs",
        "//studio.js",
        "/../studio.js",
    ] {
        let response = server.request("GET", at).send();
        response.error(404, "not_found");
        assert_eq!(response.header("content-security-policy"), None, "{at}");
    }
    // A query does not make it another page.
    let asked = server.request("GET", "/?token=nothing-reads-this").send();
    assert_eq!(asked.status, 200);
    assert!(asked.body == built("index.html"));
}

#[test]
fn the_address_of_a_screen_is_answered_with_the_page() {
    let dir = tempfile::tempdir().unwrap();
    let server = serve_things(dir.path(), &[]);
    let page = server.request("GET", "/").without("Authorization").send();

    for at in [
        "/review",
        "/review/7",
        "/drafts/3",
        "/identities",
        "/identities/0190a5c8-0000-7000-8000-000000000000",
        "/identities?q=halden+energy",
        "/records/0190a5c8-0000-7000-8000-000000000000",
        "/records?source=crm&key=c-104",
        "/documents/reports%2F2024.txt",
        "/documents/a/b.txt",
        "/explain?a=1&b=2",
        "/runs",
        "/runs/9",
        "/log",
    ] {
        // A browser that opens the address has no token to send.
        let response = server.request("GET", at).without("Authorization").send();
        assert_eq!(response.status, 200, "{at}");
        assert!(response.body == built("index.html"), "{at}");
        for header in ["content-type", "etag", "content-security-policy"] {
            assert_eq!(response.header(header), page.header(header), "{at}");
        }
        assert_eq!(server.request("HEAD", at).send().status, 200, "{at}");
        server
            .request("POST", at)
            .body(b"{}")
            .send()
            .error(405, "invalid");
    }

    // The names belong to the studio: without it they are nothing.
    let bare = tempfile::tempdir().unwrap();
    let server = serve_things(bare.path(), &["--no-studio"]);
    for at in ["/runs", "/identities/1"] {
        server.get(at).error(404, "not_found");
    }
}

#[test]
fn serve_can_be_told_not_to_serve_the_studio() {
    let dir = tempfile::tempdir().unwrap();
    let server = serve_things(dir.path(), &["--no-studio"]);

    for (at, _, _) in FILES {
        server
            .request("GET", at)
            .without("Authorization")
            .send()
            .error(404, "not_found");
        server.get(at).error(404, "not_found");
    }
    server.read("/v1/info");
    assert_eq!(server.get("/healthz").status, 200);

    // It says nothing about a studio either: the first account is made
    // through the API.
    let printed = server.stopped();
    assert!(printed[0].starts_with("mcp       http://"), "{printed:?}");
    assert!(printed[1].starts_with("setup     "), "{printed:?}");
    assert!(
        printed[1..].iter().all(|line| !line.contains("http://")),
        "{printed:?}"
    );
}

#[test]
fn dev_serves_the_studio_and_says_where() {
    let dir = tempfile::tempdir().unwrap();
    write_things(dir.path());
    let server = Server::start_with(&["dev", path(dir.path()), "--listen", "127.0.0.1:0"]).unwrap();
    assert_eq!(
        server.printed("studio"),
        format!("studio    http://{}/", server.address)
    );
    let page = server.request("GET", "/").without("Authorization").send();
    assert_eq!(page.status, 200);
    assert!(page.body == built("index.html"));
    // Nobody signs in to a development server.
    let info = server
        .request("GET", "/v1/info")
        .without("Authorization")
        .send();
    assert_eq!(info.ok()["api"], "v1");
}

#[test]
fn the_usage_says_how_to_open_the_studio_and_how_to_turn_it_off() {
    let serve = succeed(&["serve", "--help"]);
    assert!(serve.contains("--no-studio"), "{serve}");
    assert!(serve.contains("the studio"), "{serve}");
    let dev = succeed(&["dev", "--help"]);
    assert!(dev.contains("in a browser for the studio"), "{dev}");
    assert!(!dev.contains("--no-studio"), "{dev}");

    let refused = run(&["dev", "--no-studio"]);
    assert_eq!(refused.status.code(), Some(2));
    assert!(
        text(&refused.stderr).contains("unknown option `--no-studio`"),
        "{}",
        text(&refused.stderr)
    );
}

/// Runs a `bun` command in the studio package, against this build's binary.
fn bun(arguments: &[&str]) {
    let output = Command::new("bun")
        .args(arguments)
        .current_dir(repository().join("packages/studio"))
        .env("SPOOL_BIN", env!("CARGO_BIN_EXE_spool"))
        .output()
        .expect("bun is installed");
    assert!(
        output.status.success(),
        "bun {arguments:?} failed:\n{}{}",
        text(&output.stdout),
        text(&output.stderr)
    );
}

/// The part of the package's test files that this run takes: `1/2` is the
/// first of two. All of them, unless `SPOOL_STUDIO_SHARD` names a part, as
/// it does where the parts are run on machines of their own.
fn shard() -> Option<String> {
    std::env::var("SPOOL_STUDIO_SHARD")
        .ok()
        .filter(|shard| !shard.is_empty())
}

#[test]
#[ignore = "needs bun, and the network the first time"]
fn the_studio_this_binary_serves_is_built_from_its_source_and_passes_its_tests() {
    bun(&["install", "--frozen-lockfile"]);
    let shard = shard();
    // The checks are in no test file, so no part holds them: the first part
    // makes them, as a run of every part does.
    if shard.as_deref().is_none_or(|shard| shard.starts_with("1/")) {
        bun(&["run", "typecheck"]);
        // The files compiled into the binary are what the source builds to.
        bun(&["run", "check-build"]);
    }
    // Every screen is rendered against a real server of this binary. The
    // test files run side by side, each with servers of its own.
    let mut tests = vec!["run", "test", "--parallel"];
    let part = shard.map(|shard| format!("--shard={shard}"));
    tests.extend(part.as_deref());
    bun(&tests);
}
