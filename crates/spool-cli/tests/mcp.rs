//! The MCP endpoint, called as a client calls it: JSON-RPC over HTTP against
//! the real binary.

mod support;

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use spool::Rows;
use support::*;

/// What a client sends to say it takes either kind of answer.
const ACCEPT: &str = "application/json, text/event-stream";

/// The tools, in the order the server lists them, and whether each only
/// reads.
const TOOLS: [(&str, bool); 16] = [
    ("describe_store", true),
    ("search_identities", true),
    ("get_identity", true),
    ("get_identity_history", true),
    ("search_records", true),
    ("get_record", true),
    ("explain_pair", true),
    ("list_review_items", true),
    ("get_review_case", true),
    ("list_constraints", true),
    ("list_changesets", true),
    ("mint_identity_ids", true),
    ("commit_changeset", false),
    ("split_identity", false),
    ("undo_changeset", false),
    ("dismiss_review_item", false),
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
fn a_client_connects_and_is_told_the_tools() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _) = thing_server(dir.path());

    // The handshake of the revisions before 2026-07-28.
    let client = json!({
        "protocolVersion": "2025-06-18",
        "capabilities": {},
        "clientInfo": { "name": "test", "version": "0" },
    });
    let hello = ask(&server, "initialize", client);
    assert_eq!(hello["protocolVersion"], "2025-06-18");
    assert_eq!(hello["serverInfo"]["name"], "spool");
    assert_eq!(hello["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
    assert!(hello["capabilities"]["tools"].is_object(), "{hello}");
    assert!(
        text(&hello["instructions"]).contains("describe_store"),
        "{hello}"
    );
    let ready = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
    let ready = message(&server, &ready).send();
    assert_eq!(ready.status, 202);
    // The server keeps no session, so a client has none to send back.
    assert_eq!(ready.header("mcp-session-id"), None);

    let listed = ask(&server, "tools/list", json!({}));
    let tools = listed["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|tool| text(&tool["name"])).collect();
    assert_eq!(names, TOOLS.map(|(name, _)| name));
    assert!(listed["nextCursor"].is_null(), "{listed}");
    for (tool, (name, reads)) in tools.iter().zip(TOOLS) {
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
        // Every change says why.
        let required = schema["required"].as_array().cloned().unwrap_or_default();
        assert_eq!(required.contains(&json!("reason")), !reads, "{name}");
    }
    let search = &tools[1]["inputSchema"];
    assert_eq!(search["properties"]["q"]["type"], "string");
    assert_eq!(search["properties"]["after"]["$ref"], "#/$defs/IdentityId");
    assert!(search.get("required").is_none(), "{search}");

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
    let nobody = message(&server, &list).without("Authorization").send();
    nobody.error(401, "unauthorized");
    assert_eq!(nobody.header("www-authenticate"), Some("Bearer"));
    let wrong = message(&server, &list).header("Authorization", "Bearer not-it");
    wrong.send().error(401, "unauthorized");
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

    // The server is not an OAuth authorization server, and says so to a
    // client that looks for one after a 401.
    for at in [
        "/.well-known/oauth-protected-resource",
        "/.well-known/oauth-protected-resource/mcp",
        "/.well-known/oauth-authorization-server",
    ] {
        let found = server.request("GET", at).without("Authorization").send();
        found.error(404, "not_found");
    }

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

#[test]
fn a_tool_that_looks_something_up_answers_what_the_api_answers() {
    let dir = tempfile::tempdir().unwrap();
    let server = company_server(dir.path());

    assert_eq!(
        tool(&server, "describe_store", json!({})),
        server.read("/v1/info")
    );

    let found = tool(&server, "search_identities", json!({ "q": "acorn" }));
    assert_eq!(found, server.read("/v1/identities?q=acorn&limit=20"));
    assert_eq!(found["identities"][0]["label"], "Acorn Tools");
    let acorn = text(&found["identities"][0]["id"]);

    // A list is short unless the call asks for more.
    let page = tool(
        &server,
        "search_identities",
        json!({ "entity_type": "company" }),
    );
    assert_eq!(
        page,
        server.read("/v1/identities?entity_type=company&limit=20")
    );
    let page = tool(&server, "search_identities", json!({ "limit": 2 }));
    assert_eq!(page["identities"].as_array().unwrap().len(), 2);
    let second = text(&page["identities"][1]["id"]);
    let ids = format!("{acorn},{second}");
    assert_eq!(
        tool(
            &server,
            "search_identities",
            json!({ "ids": [acorn, second] })
        ),
        server.read(&format!("/v1/identities?ids={ids}&limit=20"))
    );
    assert_eq!(
        tool(
            &server,
            "search_identities",
            json!({ "after": page["next"], "limit": 3 })
        ),
        server.read(&format!(
            "/v1/identities?after={}&limit=3",
            text(&page["next"])
        ))
    );

    let identity = tool(&server, "get_identity", json!({ "identity": acorn }));
    assert_eq!(identity, server.read(&format!("/v1/identities/{acorn}")));
    assert_eq!(identity["members"].as_array().unwrap().len(), 3);
    assert_eq!(
        tool(
            &server,
            "get_identity_history",
            json!({ "identity": acorn })
        ),
        server.read(&format!("/v1/identities/{acorn}/history"))
    );

    let records = tool(&server, "search_records", json!({ "identity": acorn }));
    assert_eq!(
        records,
        server.read(&format!("/v1/records?identity={acorn}&limit=20"))
    );
    let a = text(&records["records"][0]["id"]);
    let b = text(&records["records"][1]["id"]);
    assert_eq!(
        tool(
            &server,
            "search_records",
            json!({ "source": "crm", "key": "c-102" })
        ),
        server.read("/v1/records?source=crm&key=c-102&limit=20")
    );
    let unplaced = tool(&server, "search_records", json!({ "unassigned": true }));
    assert_eq!(
        unplaced,
        server.read("/v1/records?unassigned=true&limit=20")
    );
    assert_eq!(unplaced["records"].as_array().unwrap().len(), 1);
    assert_eq!(
        tool(&server, "get_record", json!({ "record": a })),
        server.read(&format!("/v1/records/{a}"))
    );
    assert_eq!(
        tool(&server, "explain_pair", json!({ "a": a, "b": b })),
        server.read(&format!("/v1/explain?a={a}&b={b}"))
    );

    let open = tool(&server, "list_review_items", json!({}));
    assert_eq!(open, server.read("/v1/review?status=open"));
    assert_eq!(open["items"].as_array().unwrap().len(), 1);
    let none = tool(
        &server,
        "list_review_items",
        json!({ "status": "dismissed" }),
    );
    assert_eq!(none, json!({ "items": [] }));
    assert_eq!(
        tool(&server, "get_review_case", json!({ "item": 1 })),
        server.read("/v1/review/1/case")
    );
    assert_eq!(
        tool(&server, "list_constraints", json!({})),
        server.read("/v1/constraints")
    );
    assert_eq!(
        tool(&server, "list_changesets", json!({ "after": 1 })),
        server.read("/v1/changesets?after=1&limit=20")
    );

    // What the API does not find, or cannot read, the tool says as the API
    // does, in an answer the model can read.
    let unused = "01a1ee50-0000-7000-8000-000000000000";
    let missing = refused(
        &server,
        "get_identity",
        json!({ "identity": unused }),
        "not_found",
    );
    assert_eq!(
        missing["message"].as_str(),
        Some(
            server
                .get(&format!("/v1/identities/{unused}"))
                .error(404, "not_found")
                .as_str()
        )
    );
    refused(
        &server,
        "get_review_case",
        json!({ "item": 99 }),
        "not_found",
    );
    let unread = refused(
        &server,
        "get_identity",
        json!({ "identity": "acorn" }),
        "invalid",
    );
    assert!(
        text(&unread["message"]).contains("get_identity"),
        "{unread}"
    );
    refused(&server, "get_identity", json!({}), "invalid");
    refused(
        &server,
        "search_records",
        json!({ "limit": "ten" }),
        "invalid",
    );
    refused(
        &server,
        "describe_store",
        json!({ "verbose": true }),
        "invalid",
    );
    refused(
        &server,
        "list_review_items",
        json!({ "status": "pending" }),
        "invalid",
    );
    // Looking things up changed nothing.
    assert_eq!(head(&server), 2);
}

#[test]
fn a_tool_that_changes_something_is_held_to_the_rules_of_the_api() {
    let dir = tempfile::tempdir().unwrap();
    let server = company_server(dir.path());
    let before = head(&server);
    let identities = server.read("/v1/identities?q=atlas");
    let [from, into] = [0, 1].map(|index| text(&identities["identities"][index]["id"]).to_owned());
    let merge = json!({ "op": "merge", "from": from, "into": into });

    // A caller does not name the author of a change.
    for name in ["actor", "author"] {
        let mut named = json!({ "ops": [merge], "reason": "the same company" });
        named[name] = json!("user:someone-else");
        let error = refused(&server, "commit_changeset", named, "invalid");
        assert!(text(&error["message"]).contains(name), "{error}");
    }
    // A change says why.
    refused(
        &server,
        "commit_changeset",
        json!({ "ops": [merge] }),
        "invalid",
    );
    // A changeset the server rejects says which operation broke which rule.
    let itself = json!({ "op": "merge", "from": into, "into": into });
    let both = json!({ "ops": [merge, itself], "reason": "the same company" });
    let rejected = refused(&server, "commit_changeset", both, "rejected");
    assert_eq!(rejected["rejections"][0]["op_index"], 1, "{rejected}");
    assert_eq!(
        rejected["rejections"][0]["reason"]["reason"],
        "merge_into_itself"
    );
    // None of that changed anything: a changeset is committed whole or not
    // at all.
    assert_eq!(head(&server), before);

    let committed = tool(
        &server,
        "commit_changeset",
        json!({ "ops": [merge], "reason": "the same company", "base": before }),
    );
    assert_eq!(committed["seq"], before + 1);
    let logged = entry(&server, before + 1);
    assert_eq!(logged["changeset"]["actor"], "agent:api");
    assert_eq!(logged["changeset"]["reason"], "the same company");
    assert_eq!(logged["changeset"]["base"], before);
    assert_eq!(logged["changeset"]["ops"], json!([merge]));
    let merged = tool(&server, "get_identity", json!({ "identity": from }));
    assert_eq!(merged["current"], into.as_str());
    assert_eq!(merged["members"].as_array().unwrap().len(), 4);

    // A change made on what has since changed is refused, as on the API.
    let others = server.read("/v1/identities?q=halden");
    let other = text(&others["identities"][0]["id"]);
    let stale = json!({
        "ops": [{ "op": "merge", "from": other, "into": into }],
        "reason": "read before the merge",
        "base": before,
    });
    refused(&server, "commit_changeset", stale, "rejected");

    // Undoing shows what it would commit, then commits it.
    let undo = json!({ "seq": before + 1, "reason": "two companies after all", "commit": false });
    let shown = tool(&server, "undo_changeset", undo);
    assert!(shown["committed"].is_null(), "{shown}");
    assert_eq!(shown["changeset"]["ops"][0]["op"], "reinstate");
    assert_eq!(head(&server), before + 1);
    let undo = json!({ "seq": before + 1, "reason": "two companies after all" });
    let undone = tool(&server, "undo_changeset", undo);
    assert_eq!(undone["committed"]["seq"], before + 2);
    let logged = entry(&server, before + 2);
    assert_eq!(logged["changeset"]["actor"], "agent:api");
    assert_eq!(logged["changeset"]["reason"], "two companies after all");
    let restored = tool(&server, "get_identity", json!({ "identity": from }));
    assert_eq!(restored["current"], from.as_str());
    assert_eq!(restored["members"].as_array().unwrap().len(), 2);
    refused(
        &server,
        "undo_changeset",
        json!({ "seq": 99, "reason": "no such change" }),
        "not_found",
    );

    // A split moves the records named to a new identity and keeps the two
    // apart.
    let acorn = server.read("/v1/identities?q=acorn");
    let acorn = text(&acorn["identities"][0]["id"]);
    let members = tool(&server, "get_identity", json!({ "identity": acorn }));
    let moved = text(&members["members"][0]["record"]);
    let split = tool(
        &server,
        "split_identity",
        json!({ "identity": acorn, "records": [moved], "reason": "another Acorn", "base": before + 2 }),
    );
    assert_eq!(split["committed"]["seq"], before + 3, "{split}");
    let logged = entry(&server, before + 3);
    assert_eq!(logged["changeset"]["actor"], "agent:api");
    assert_eq!(logged["changeset"]["reason"], "another Acorn");
    let record = tool(&server, "get_record", json!({ "record": moved }));
    assert_ne!(record["identity"], acorn);
    let apart = tool(&server, "list_constraints", json!({ "record": moved }));
    assert_eq!(apart["constraints"].as_array().unwrap().len(), 1, "{apart}");
    assert_eq!(apart["constraints"][0]["kind"], "cannot_link");
    // The server holds a later change to what was decided.
    let back = json!({
        "ops": [{ "op": "assign", "record": moved, "from": record["identity"], "to": acorn }],
        "reason": "put it back",
    });
    let held = refused(&server, "commit_changeset", back, "rejected");
    assert_eq!(held["rejections"][0]["op_index"], 0, "{held}");
    let nothing = json!({ "identity": acorn, "records": [], "reason": "nothing to move" });
    refused(&server, "split_identity", nothing, "conflict");

    // A review item is closed with who closed it and why.
    refused(
        &server,
        "dismiss_review_item",
        json!({ "item": 1 }),
        "invalid",
    );
    let dismissed = tool(
        &server,
        "dismiss_review_item",
        json!({ "item": 1, "reason": "too little to go on" }),
    );
    assert_eq!(dismissed["status"], "dismissed", "{dismissed}");
    assert_eq!(dismissed["dismissed"]["by"], "agent:api");
    assert_eq!(dismissed["dismissed"]["reason"], "too little to go on");
    assert_eq!(
        tool(&server, "list_review_items", json!({})),
        json!({ "items": [] })
    );
    let again = json!({ "item": 1, "reason": "again" });
    refused(&server, "dismiss_review_item", again, "conflict");

    // IDs are minted a few at a time.
    let minted = tool(&server, "mint_identity_ids", json!({}));
    assert_eq!(minted["identities"].as_array().unwrap().len(), 1);
    assert_eq!(minted["changesets"], json!([]));
    refused(
        &server,
        "mint_identity_ids",
        json!({ "count": 101 }),
        "invalid",
    );
    assert_eq!(head(&server), before + 3);
}

#[test]
fn an_answer_too_large_for_a_model_is_refused_and_says_what_to_do() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _) = thing_server(dir.path());
    let name = "a name that takes some room ".repeat(20);
    let rows: Vec<Value> = (0..600)
        .map(|row| json!({ "id": format!("k-{row}"), "code": format!("c-{row}"), "name": name }))
        .collect();
    server.call(
        "/v1/load",
        json!({ "source": "bulk", "entity_type": "thing", "rows": rows }),
    );

    let few = tool(&server, "search_records", json!({}));
    assert_eq!(few["records"].as_array().unwrap().len(), 20);
    assert!(few["next"].is_string(), "{few}");
    let all = refused(
        &server,
        "search_records",
        json!({ "limit": 600 }),
        "too_large",
    );
    assert!(text(&all["message"]).contains("`limit`"), "{all}");
}

#[test]
fn a_change_whose_answer_is_too_large_is_still_answered_as_made() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _) = thing_server(dir.path());
    let rows: Vec<Value> = (0..5000)
        .map(|row| json!({ "id": format!("k-{row}"), "code": format!("c-{row}") }))
        .collect();
    server.call(
        "/v1/load",
        json!({ "source": "bulk", "entity_type": "thing", "rows": rows }),
    );
    let mut records: Vec<Value> = Vec::new();
    let mut after = String::new();
    loop {
        let page = server.read(&format!("/v1/records?limit=1000{after}"));
        records.extend(
            page["records"]
                .as_array()
                .unwrap()
                .iter()
                .map(|record| record["id"].clone()),
        );
        match page["next"].as_str() {
            Some(next) => after = format!("&after={next}"),
            None => break,
        }
    }
    assert_eq!(records.len(), 5000);

    // One identity holds them all.
    let minted = tool(&server, "mint_identity_ids", json!({}));
    let whole = minted["identities"][0].clone();
    let mut ops =
        vec![json!({ "op": "create_identity", "identity": whole, "entity_type": "thing" })];
    ops.extend(
        records
            .iter()
            .map(|record| json!({ "op": "assign", "record": record, "from": null, "to": whole })),
    );
    let placed = tool(
        &server,
        "commit_changeset",
        json!({ "ops": ops, "reason": "all one thing" }),
    );
    assert_eq!(placed["seq"], 1);

    // Splitting it answers with the changeset that did, which is more than
    // a tool returns. The split was made, so the answer says so and keeps
    // what the caller needs: where it was committed and the new identity.
    let side = &records[..2000];
    let split = tool(
        &server,
        "split_identity",
        json!({ "identity": whole, "records": side, "reason": "two things", "keep_apart": false }),
    );
    assert_eq!(split["committed"]["seq"], 2, "{split}");
    assert_eq!(split["kept"], whole);
    assert_eq!(split["left_out"], json!(["changeset", "moved"]), "{split}");
    assert!(text(&split["note"]).contains("succeeded"), "{split}");
    assert_eq!(head(&server), 2);
    let new = tool(
        &server,
        "get_identity",
        json!({ "identity": split["identity"] }),
    );
    assert_eq!(new["members"].as_array().unwrap().len(), 2000);

    // So does taking it back, whether shown or committed.
    let shown = tool(
        &server,
        "undo_changeset",
        json!({ "seq": 2, "reason": "one thing after all", "commit": false }),
    );
    assert!(shown["committed"].is_null(), "{shown}");
    assert_eq!(shown["left_out"], json!(["changeset"]));
    assert_eq!(head(&server), 2);
    let undone = tool(
        &server,
        "undo_changeset",
        json!({ "seq": 2, "reason": "one thing after all" }),
    );
    assert_eq!(undone["committed"]["seq"], 3, "{undone}");
    assert_eq!(undone["left_out"], json!(["changeset"]));
    assert_eq!(head(&server), 3);
}

#[test]
fn a_refusal_too_large_for_a_model_gives_the_first_of_its_reasons() {
    let dir = tempfile::tempdir().unwrap();
    let (server, _) = thing_server(dir.path());
    let ops: Vec<Value> = (0..5000)
        .map(|op| {
            let identity = format!("01a1ee5d-3154-710c-8a15-{op:012x}");
            json!({ "op": "merge", "from": identity, "into": identity })
        })
        .collect();

    // The API answers with every operation that broke a rule, which is more
    // than a tool returns.
    let whole = server
        .request("POST", "/v1/changesets")
        .json(&json!({ "ops": ops, "reason": "each into itself" }))
        .send();
    assert_eq!(whole.status, 409);
    assert!(whole.body.len() > 256 * 1024, "{}", whole.body.len());

    let error = refused(
        &server,
        "commit_changeset",
        json!({ "ops": ops, "reason": "each into itself" }),
        "rejected",
    );
    // With what the error is wrapped in, it is no more than a tool returns.
    let size = json!({ "error": error }).to_string().len();
    assert!(size <= 256 * 1024, "{size}");
    let given = error["rejections"].as_array().unwrap();
    assert!(!given.is_empty() && given.len() < 5000, "{}", given.len());
    for (index, rejection) in given.iter().enumerate() {
        assert_eq!(rejection["op_index"], index, "{rejection}");
    }
    let said = text(&error["message"]);
    let count = format!("the first {}", given.len());
    assert!(said.contains(&count), "{said}");
    assert!(said.contains("Nothing was changed"), "{said}");
    assert_eq!(head(&server), 0);

    // An error that repeats what the caller sent repeats the beginning of
    // it: an argument the tool does not take, and a tool there is not.
    let long = "x".repeat(300 * 1024);
    let mut named = json!({ "ops": [], "reason": "nothing" });
    named[&long] = json!(true);
    let error = refused(&server, "commit_changeset", named, "invalid");
    let said = text(&error["message"]);
    assert!(said.contains("`commit_changeset`"), "{said}");
    assert!(said.len() <= 4 * 1024, "{}", said.len());
    let unknown = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": long, "arguments": {} },
    });
    let answer = message(&server, &unknown).send().json();
    assert_eq!(
        answer["error"]["code"], -32602,
        "{}",
        answer["error"]["code"]
    );
    let said = text(&answer["error"]["message"]);
    assert!(said.starts_with("there is no tool called `xxx"), "{said}");
    assert!(said.len() <= 5 * 1024, "{}", said.len());
}

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
    assert_eq!(
        logged["changeset"]["actor"],
        format!("user:{}", text(&dev["person"]["id"]))
    );
}
