//! The MCP endpoint, called as a client calls it: JSON-RPC over HTTP against
//! the real binary.

use std::path::{Path, PathBuf};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use spool::Rows;

use crate::support::{self, *};

mod changing;
mod documents;
mod large_answers;
mod reading;

/// What a client sends to say it takes either kind of answer.
const ACCEPT: &str = "application/json, text/event-stream";

/// The tools, in the order the server lists them, whether each only reads,
/// and whether it asks for a reason. A tool that writes under its caller's
/// name does. A load and a match run are recorded as runs, which have no
/// author and no reason.
const TOOLS: [(&str, bool, bool); 26] = [
    ("describe_store", true, false),
    ("search_identities", true, false),
    ("get_identity", true, false),
    ("get_identity_history", true, false),
    ("get_identity_at", true, false),
    ("search_records", true, false),
    ("get_record", true, false),
    ("explain_pair", true, false),
    ("list_review_items", true, false),
    ("get_review_case", true, false),
    ("list_constraints", true, false),
    ("list_changesets", true, false),
    ("list_changeset_ops", true, false),
    ("get_run", true, false),
    ("mint_identity_ids", true, false),
    ("add_record", false, false),
    ("add_document", false, false),
    ("load_rows", false, false),
    ("load_documents", false, false),
    ("run_extraction", false, false),
    ("run_match", false, false),
    ("retract_rows", false, true),
    ("commit_changeset", false, true),
    ("split_identity", false, true),
    ("undo_changeset", false, true),
    ("dismiss_review_item", false, true),
];

fn companies() -> PathBuf {
    benchmarks().join("identity/companies-small")
}

/// A server that holds the company scenario: 39 records in 18 identities,
/// and one record that fits two of them, left for review.
fn company_server(dir: &Path) -> Server {
    let project = dir.join("project");
    copy_project(&companies(), &project);
    let server = Server::serve(&project, dir, 1);
    for batch in ["1", "2"] {
        for source in ["crm", "registry"] {
            let file = companies().join(format!("batch-{batch}-{source}.csv"));
            let rows = json_rows(Rows::from_csv(file).unwrap());
            let body = json!({ "source": source, "entity_type": "company", "rows": rows });
            server.call("/v1/load", body);
        }
        let run = server.call("/v1/match", json!({ "entity_type": "company" }));
        assert_eq!(run["run"]["status"], "succeeded", "{run}");
    }
    server
}

/// A server on the `thing` project, an empty store, and the code that makes
/// its first account.
fn thing_server(dir: &Path) -> (Server, String) {
    let project = dir.join("project");
    write_things(&project);
    let server = Server::serve(&project, dir, 1);
    // The server says where the endpoint is when it starts.
    assert_eq!(
        server.printed("mcp"),
        format!("mcp       http://{}/mcp", server.address)
    );
    let link = server.printed("setup");
    let code = link.rsplit_once('#').expect("a code in the link").1;
    (server, code.to_owned())
}

/// A message to the endpoint, with the server's token.
fn message<'a>(server: &'a Server, body: &Value) -> Request<'a> {
    server
        .request("POST", "/mcp")
        .header("Accept", ACCEPT)
        .json(body)
}

/// A request to the endpoint as a client of the protocol's sessions-and-
/// handshake revisions sends it, and its result.
fn ask(server: &Server, method: &str, params: Value) -> Value {
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
    let answer = message(server, &body).send().ok();
    assert!(answer["error"].is_null(), "{method} failed: {answer}");
    answer["result"].clone()
}

/// What a tool call answers: whether it is an error, and the JSON in its
/// text.
fn outcome(result: &Value) -> (bool, Value) {
    let content = result["content"].as_array().expect("content");
    assert_eq!(content.len(), 1, "{result}");
    assert_eq!(content[0]["type"], "text", "{result}");
    let text = content[0]["text"].as_str().expect("text");
    let value = serde_json::from_str(text).unwrap_or_else(|_| panic!("not JSON: {text}"));
    (result["isError"] == true, value)
}

fn call_tool(request: Request<'_>, tool: &str, arguments: Value) -> (bool, Value) {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": tool, "arguments": arguments },
    });
    let answer = request.header("Accept", ACCEPT).json(&body).send().ok();
    assert!(answer["error"].is_null(), "{tool} failed: {answer}");
    outcome(&answer["result"])
}

/// Calls a tool that has to succeed, with the server's token.
fn tool(server: &Server, tool: &str, arguments: Value) -> Value {
    let (failed, value) = call_tool(server.request("POST", "/mcp"), tool, arguments);
    assert!(!failed, "{tool} failed: {value}");
    value
}

/// Calls a tool that has to fail with the given code of the API, and returns
/// its error.
fn refused(server: &Server, tool: &str, arguments: Value, code: &str) -> Value {
    let (failed, value) = call_tool(server.request("POST", "/mcp"), tool, arguments);
    assert!(failed, "{tool} did not fail: {value}");
    assert_eq!(value["error"]["code"], code, "{value}");
    value["error"].clone()
}

fn head(server: &Server) -> u64 {
    server.read("/v1/info")["store"]["head"].as_u64().unwrap()
}

/// The entry of the log at `seq`.
fn entry(server: &Server, seq: u64) -> Value {
    let log = server.read(&format!("/v1/changesets?after={}&limit=1", seq - 1));
    log["changesets"][0].clone()
}

fn text(value: &Value) -> &str {
    value
        .as_str()
        .unwrap_or_else(|| panic!("not text: {value}"))
}

#[test]
#[expect(clippy::too_many_lines)]
fn a_client_connects_and_is_told_the_tools() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _) = thing_server(dir.path());

    // The handshake of the revisions before 2026-07-28.
    let client = json!({
        "protocolVersion": "2025-11-25",
        "capabilities": {},
        "clientInfo": { "name": "test", "version": "0" },
    });
    let hello = ask(&server, "initialize", client);
    assert_eq!(hello["protocolVersion"], "2025-11-25");
    let info = &hello["serverInfo"];
    assert_eq!(info["name"], "spool");
    assert_eq!(info["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(info["title"], "Spool");
    assert!(text(&info["description"]).contains("persistent identities"));
    assert_eq!(info["websiteUrl"], "https://spool.alexpatow.com");
    let icon = &info["icons"][0];
    assert_eq!(icon["mimeType"], "image/png");
    assert_eq!(icon["sizes"], json!(["48x48"]));
    let encoded = text(&icon["src"])
        .strip_prefix("data:image/png;base64,")
        .expect("embedded PNG");
    assert_eq!(
        STANDARD.decode(encoded).unwrap(),
        include_bytes!("../../assets/mcp-icon.png")
    );
    assert!(hello["capabilities"]["tools"].is_object(), "{hello}");
    for named in [
        "describe_store",
        "add_record",
        "add_document",
        "load_rows",
        "load_documents",
        "run_extraction",
        "run_match",
        "retract_rows",
    ] {
        assert!(text(&hello["instructions"]).contains(named), "{hello}");
    }
    let ready = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
    let ready = message(&server, &ready).send();
    assert_eq!(ready.status, 202);
    // The server keeps no session, so a client has none to send back.
    assert_eq!(ready.header("mcp-session-id"), None);

    let listed = ask(&server, "tools/list", json!({}));
    let tools = listed["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|tool| text(&tool["name"])).collect();
    assert_eq!(names, TOOLS.map(|(name, _, _)| name));
    assert!(listed["nextCursor"].is_null(), "{listed}");
    for (tool, (name, reads, says_why)) in tools.iter().zip(TOOLS) {
        assert!(text(&tool["description"]).len() > 40, "{name}");
        assert_eq!(tool["annotations"]["readOnlyHint"], reads, "{name}");
        assert_eq!(tool["annotations"]["destructiveHint"], !reads, "{name}");
        assert_eq!(tool["annotations"]["openWorldHint"], false, "{name}");
        let schema = &tool["inputSchema"];
        assert_eq!(schema["type"], "object", "{name}");
        assert_eq!(schema["additionalProperties"], false, "{name}");
        // Nothing a caller sends says who it is.
        let empty = serde_json::Map::new();
        let properties = schema["properties"].as_object().unwrap_or(&empty);
        for (argument, described) in properties {
            assert!(!["actor", "author", "user"].contains(&argument.as_str()));
            // An argument that may be left out is described as what it is.
            assert!(described.get("default").is_none(), "{name}.{argument}");
            assert!(described.get("anyOf").is_none(), "{name}.{argument}");
            assert!(!described["type"].is_array(), "{name}.{argument}");
        }
        // Every change made under the caller's name says why.
        let required = schema["required"].as_array().cloned().unwrap_or_default();
        assert_eq!(required.contains(&json!("reason")), says_why, "{name}");
    }
    let search = &tools[1]["inputSchema"];
    assert_eq!(search["properties"]["q"]["type"], "string");
    assert_eq!(search["properties"]["after"]["$ref"], "#/$defs/IdentityId");
    assert!(search.get("required").is_none(), "{search}");
    // The extractor may be left out, as it may be of `spool extract`.
    let extraction = tools
        .iter()
        .find(|tool| tool["name"] == "run_extraction")
        .unwrap();
    assert!(
        extraction["inputSchema"].get("required").is_none(),
        "{extraction}"
    );
    // A row loaded by mistake can be taken back, so the description of a
    // load does not warn an assistant off a row it was asked to load.
    let load = tools
        .iter()
        .find(|tool| tool["name"] == "load_rows")
        .unwrap();
    let said = text(&load["description"]);
    assert!(
        said.contains("Load every row you were asked to load"),
        "{said}"
    );
    assert!(said.contains("`retract_rows`"), "{said}");
    for scare in ["for good", "nothing deletes"] {
        assert!(!said.contains(scare), "{said}");
        assert!(!text(&hello["instructions"]).contains(scare), "{hello}");
    }

    // The revision that has no handshake: each request says what it speaks.
    let meta = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": { "name": "test", "version": "0" },
        "io.modelcontextprotocol/clientCapabilities": {},
    });
    let modern = |method: &str, mut params: Value| {
        params["_meta"] = meta.clone();
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        let mut request = message(&server, &body)
            .header("MCP-Protocol-Version", "2026-07-28")
            .header("Mcp-Method", method);
        if let Some(name) = params["name"].as_str() {
            request = request.header("Mcp-Name", name);
        }
        let answer = request.send().ok();
        assert!(answer["error"].is_null(), "{method} failed: {answer}");
        answer["result"].clone()
    };
    let discovered = modern("server/discover", json!({}));
    let versions = discovered["supportedVersions"].as_array().unwrap();
    assert!(versions.contains(&json!("2026-07-28")), "{discovered}");
    assert_eq!(
        &discovered["_meta"]["io.modelcontextprotocol/serverInfo"],
        info
    );
    assert_eq!(modern("tools/list", json!({}))["tools"], listed["tools"]);
    let described = modern(
        "tools/call",
        json!({ "name": "describe_store", "arguments": {} }),
    );
    assert_eq!(outcome(&described), (false, server.read("/v1/info")));

    // What is not a tool is an error of the protocol.
    let unknown = json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "tools/call",
        "params": { "name": "drop_everything", "arguments": {} },
    });
    let answer = message(&server, &unknown).send().json();
    assert_eq!(answer["error"]["code"], -32602, "{answer}");
}

#[test]
fn the_endpoint_answers_only_a_caller_the_api_answers() {
    let dir = tempfile::tempdir().unwrap();
    let (server, code) = thing_server(dir.path());
    let list = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" });

    // Nothing is answered without the token or a session, not even what the
    // tools are.
    // The refusal says where an assistant reads how to sign in with OAuth:
    // see `oauth.rs`.
    let nobody = message(&server, &list).without("Authorization").send();
    nobody.error(401, "unauthorized");
    let metadata = format!(
        "http://{}/.well-known/oauth-protected-resource/mcp",
        server.address
    );
    assert_eq!(
        nobody.header("www-authenticate"),
        Some(format!("Bearer resource_metadata=\"{metadata}\", scope=\"spool\"").as_str())
    );
    let wrong = message(&server, &list)
        .header("Authorization", "Bearer not-it")
        .send();
    wrong.error(401, "unauthorized");
    assert!(
        wrong
            .header("www-authenticate")
            .is_some_and(|said| said.ends_with("error=\"invalid_token\"")),
        "{:?}",
        wrong.header("www-authenticate")
    );
    let hello = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "test", "version": "0" },
        },
    });
    let nobody = message(&server, &hello).without("Authorization").send();
    nobody.error(401, "unauthorized");

    // Someone signed in is answered, as on the API.
    let account =
        json!({ "code": code, "username": "ada", "name": "Ada", "password": "a long enough one" });
    let first = server
        .request("POST", "/auth/setup")
        .without("Authorization")
        .json(&account)
        .send();
    let person = text(&first.ok()["person"]["id"]).to_owned();
    let cookie = first.header("set-cookie").expect("a session cookie");
    let cookie = cookie.split(';').next().unwrap();
    let browser = || {
        server
            .request("POST", "/mcp")
            .without("Authorization")
            .header("Cookie", cookie)
    };
    let minted = tool(&server, "mint_identity_ids", json!({ "count": 2 }));
    let create = |index: usize, reason: &str| {
        json!({
            "ops": [{ "op": "create_identity", "identity": minted["identities"][index], "entity_type": "thing" }],
            "reason": reason,
        })
    };
    let (failed, by_person) = call_tool(browser(), "commit_changeset", create(0, "by a person"));
    assert!(!failed, "{by_person}");
    let by_agent = tool(&server, "commit_changeset", create(1, "by an agent"));

    // Each change is recorded under whoever the server knows the caller to
    // be, with the reason given.
    let first = entry(&server, by_person["seq"].as_u64().unwrap());
    assert_eq!(first["changeset"]["actor"], format!("user:{person}"));
    assert_eq!(first["changeset"]["reason"], "by a person");
    let second = entry(&server, by_agent["seq"].as_u64().unwrap());
    assert_eq!(second["changeset"]["actor"], "agent:api");
    assert_eq!(second["changeset"]["reason"], "by an agent");

    // An assistant with a token of its own is its own author in the log,
    // until the token is revoked.
    let store = dir.path().join("store.duckdb");
    let token = succeed(&["token", "add", "claude", "--store", path(&store)]);
    let assistant = || {
        let request = server.request("POST", "/mcp");
        request.header("Authorization", &format!("Bearer {}", token.trim()))
    };
    let more = tool(&server, "mint_identity_ids", json!({}));
    let create = json!({
        "ops": [{ "op": "create_identity", "identity": more["identities"][0], "entity_type": "thing" }],
        "reason": "by an assistant",
    });
    let (failed, by_assistant) = call_tool(assistant(), "commit_changeset", create);
    assert!(!failed, "{by_assistant}");
    let third = entry(&server, by_assistant["seq"].as_u64().unwrap());
    assert_eq!(third["changeset"]["actor"], "agent:claude");
    // A run is started by its caller in the same way.
    let anvil = json!({ "source": "shop", "entity_type": "thing", "rows": [{ "id": "1", "code": "anvil" }] });
    let (failed, by_person) = call_tool(browser(), "load_rows", anvil.clone());
    assert!(!failed, "{by_person}");
    assert_eq!(by_person["run"]["started_by"], format!("user:{person}"));
    let (failed, by_assistant) =
        call_tool(assistant(), "run_match", json!({ "entity_type": "thing" }));
    assert!(!failed, "{by_assistant}");
    assert_eq!(by_assistant["run"]["started_by"], "agent:claude");
    succeed(&["token", "revoke", "claude", "--store", path(&store)]);
    let revoked = assistant().header("Accept", ACCEPT).json(&list).send();
    revoked.error(401, "unauthorized");

    // A page of another site cannot use the session of whoever is looking
    // at it.
    let elsewhere = browser()
        .header("Origin", "https://elsewhere.example")
        .header("Accept", ACCEPT)
        .json(&list)
        .send();
    elsewhere.error(403, "forbidden");

    // The endpoint is reached by whatever name the server has.
    let named = message(&server, &list).header("Host", "spool.example");
    assert_eq!(
        named.send().ok()["result"]["tools"]
            .as_array()
            .unwrap()
            .len(),
        TOOLS.len()
    );

    // It takes messages and gives no stream to listen on.
    let listen = server
        .request("GET", "/mcp")
        .header("Accept", "text/event-stream");
    let listen = listen.send();
    listen.error(405, "invalid");
    assert_eq!(listen.header("allow"), Some("POST"));
    // What the transport refuses is said in the server's own words.
    let untyped = server.request("POST", "/mcp").header("Accept", ACCEPT);
    let untyped = untyped.body(b"{}").send().error(415, "invalid");
    assert!(untyped.contains("application/json"), "{untyped}");
}

/// Takes a capitalised word to be a thing, with the word in lower case as
/// its code.
const WORDS: &str = r#"
import re

from spool import extractor

@extractor
def find(document):
    if document.text == "unreadable":
        yield document.failed("cannot read this text")
        return
    for found in re.finditer(r"[A-Z][a-z]+", document.text):
        yield document.mention("thing", found.start(), found.end(), code=found.group().lower())
"#;

#[test]
fn a_development_server_answers_the_endpoint_only_by_this_machines_names() {
    let dir = tempfile::tempdir().unwrap();
    write_things(dir.path());
    let server = Server::start(&["dev", path(dir.path()), "--listen", "127.0.0.1:0"]);
    assert_eq!(
        server.printed("mcp"),
        format!("mcp       http://{}/mcp", server.address)
    );
    let port = server.address.rsplit_once(':').unwrap().1;
    let list = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" });
    let nobody = || {
        server
            .request("POST", "/mcp")
            .without("Authorization")
            .header("Accept", ACCEPT)
            .json(&list)
    };

    // Nobody signs in to a development server, so a page of another site
    // that has a name of its own for this machine must not be answered.
    let reached = nobody().header("Host", &format!("elsewhere.example:{port}"));
    reached.send().error(403, "forbidden");
    for host in ["localhost", "127.0.0.1"] {
        let reached = nobody().header("Host", &format!("{host}:{port}"));
        assert_eq!(
            reached.send().ok()["result"]["tools"]
                .as_array()
                .unwrap()
                .len(),
            TOOLS.len(),
            "{host}"
        );
    }

    // What an assistant on this machine changes is the `dev` account's.
    let dev = server
        .request("GET", "/auth/session")
        .without("Authorization")
        .send()
        .ok();
    let minted = tool(&server, "mint_identity_ids", json!({}));
    let create = json!({
        "ops": [{ "op": "create_identity", "identity": minted["identities"][0], "entity_type": "thing" }],
        "reason": "trying it out",
    });
    let request = server.request("POST", "/mcp").without("Authorization");
    let (failed, committed) = call_tool(request, "commit_changeset", create);
    assert!(!failed, "{committed}");
    let logged = entry(&server, committed["seq"].as_u64().unwrap());
    let dev_is = format!("user:{}", text(&dev["person"]["id"]));
    assert_eq!(logged["changeset"]["actor"], dev_is.as_str());
    // And what it loads is started by that account.
    let anvil = json!({ "source": "shop", "entity_type": "thing", "rows": [{ "id": "1", "code": "anvil" }] });
    let request = server.request("POST", "/mcp").without("Authorization");
    let (failed, loaded) = call_tool(request, "load_rows", anvil);
    assert!(!failed, "{loaded}");
    assert_eq!(loaded["run"]["started_by"], dev_is.as_str());
}
