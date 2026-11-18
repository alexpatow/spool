//! What a reviewer does to identities, over the HTTP API against the real
//! binary: merge, undo, split, constraints and dismissing a review item.

mod support;

use std::path::Path;

use serde_json::{Value, json};
use support::*;

/// What the test token writes as.
const REVIEWER: &str = "agent:api";

fn things_server(dir: &Path) -> Server {
    let project = dir.join("project");
    write_things(&project);
    Server::serve(&project, dir, 1)
}

/// Loads things from the shop as `(id, code)` and resolves them.
fn stock(server: &Server, things: &[(&str, &str)]) {
    let rows: Vec<Value> = things
        .iter()
        .map(|(id, code)| json!({ "id": id, "code": code }))
        .collect();
    let response = server.post(
        "/v1/load",
        json!({ "source": "shop", "entity_type": "thing", "rows": rows }),
    );
    assert_eq!(response.ok()["run"]["status"], "succeeded");
    let run = server.call("/v1/match", json!({ "entity_type": "thing" }));
    assert_eq!(run["run"]["status"], "succeeded", "{run}");
}

/// The record the shop holds under a key.
fn thing(server: &Server, key: &str) -> Value {
    let found = server.read(&format!("/v1/records?source=shop&key={key}"));
    let [record] = &found["records"].as_array().unwrap()[..] else {
        panic!("one record under {key}: {found}");
    };
    record.clone()
}

fn head(server: &Server) -> u64 {
    server.read("/v1/changesets")["head"].as_u64().unwrap()
}

/// The keys of an identity's members, in order.
fn members(server: &Server, identity: &Value) -> Vec<String> {
    let view = server.read(&format!("/v1/identities/{}", identity.as_str().unwrap()));
    let mut keys: Vec<String> = view["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|member| member["key"].as_str().unwrap().to_owned())
        .collect();
    keys.sort();
    keys
}

fn reasons(response: &Response) -> Vec<String> {
    response.error(409, "rejected");
    response.json()["error"]["rejections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|rejection| rejection["reason"]["reason"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn identities_are_merged_and_the_merge_is_undone() {
    let dir = tempfile::tempdir().unwrap();
    let server = things_server(dir.path());
    stock(&server, &[("1", "anvil"), ("2", "hammer")]);
    let anvil = thing(&server, "1")["identity"].clone();
    let hammer = thing(&server, "2")["identity"].clone();
    assert_eq!(head(&server), 1);

    // A changeset needs no ID and no base: the server mints one and takes
    // the head of the log.
    let merge = json!({
        "reason": "one tool",
        "ops": [{ "op": "merge", "from": anvil, "into": hammer }],
    });
    let committed = server.call("/v1/changesets", merge.clone());
    assert_eq!(committed["seq"], 2);

    // The merged ID still answers, with the identity that stands for it.
    let view = server.read(&format!("/v1/identities/{}", anvil.as_str().unwrap()));
    assert_eq!(view["id"], anvil);
    assert_eq!(view["current"], hammer);
    assert_eq!(members(&server, &anvil), ["1", "2"]);
    assert_eq!(thing(&server, "1")["identity"], hammer);
    let log = server.read("/v1/changesets?after=1");
    assert_eq!(log["changesets"][0]["changeset"]["actor"], REVIEWER);
    assert_eq!(log["changesets"][0]["changeset"]["reason"], "one tool");

    // The same merge again names an identity that is no longer active.
    assert_eq!(
        reasons(&server.post("/v1/changesets", merge)),
        ["identity_not_active"]
    );

    // An undo can be read before it is made.
    let preview = server.call("/v1/changesets/2/undo", json!({ "commit": false }));
    assert_eq!(preview["committed"], Value::Null);
    let ops: Vec<&str> = preview["changeset"]["ops"]
        .as_array()
        .unwrap()
        .iter()
        .map(|op| op["op"].as_str().unwrap())
        .collect();
    assert_eq!(ops, ["reinstate", "assign"]);
    assert_eq!(head(&server), 2);

    // Made, it brings the merged identity back with the member it had.
    let undone = server.call("/v1/changesets/2/undo", json!({}));
    assert_eq!(undone["committed"]["seq"], 3);
    assert_eq!(
        undone["changeset"]["reason"],
        "undo of the changeset at position 2"
    );
    let view = server.read(&format!("/v1/identities/{}", anvil.as_str().unwrap()));
    assert_eq!(view["current"], anvil);
    assert_eq!(members(&server, &anvil), ["1"]);
    assert_eq!(members(&server, &hammer), ["2"]);

    // A merge written before the undo is stale: its identities have changed.
    let stale = server.post(
        "/v1/changesets",
        json!({
            "base": 1,
            "ops": [{ "op": "merge", "from": anvil, "into": hammer }],
        }),
    );
    assert_eq!(reasons(&stale), ["stale_merge"]);
    assert_eq!(stale.json()["error"]["rejections"][0]["op_index"], 0);

    // What cannot be undone says why.
    server
        .post("/v1/changesets/99/undo", json!({}))
        .error(404, "not_found");
    let minted = server.call("/v1/ids", json!({ "identities": 1 }));
    let created = server.call(
        "/v1/changesets",
        json!({
            "ops": [{ "op": "create_identity", "identity": minted["identities"][0], "entity_type": "thing" }],
        }),
    );
    let message = server
        .post(
            &format!("/v1/changesets/{}/undo", created["seq"]),
            json!({}),
        )
        .error(409, "conflict");
    assert!(message.contains("only created identities"), "{message}");
    server
        .post("/v1/changesets/2/undo", json!({ "by": REVIEWER }))
        .error(400, "invalid");
}

#[test]
fn an_identity_is_split_and_a_constraint_keeps_the_sides_apart() {
    let dir = tempfile::tempdir().unwrap();
    let server = things_server(dir.path());
    stock(&server, &[("1", "anvil"), ("2", "anvil"), ("3", "anvil")]);
    let anvils = thing(&server, "1")["identity"].clone();
    assert_eq!(members(&server, &anvils), ["1", "2", "3"]);
    let third = thing(&server, "3")["id"].clone();

    let split = server.call(
        &format!("/v1/identities/{}/split", anvils.as_str().unwrap()),
        json!({ "records": [third], "reason": "another anvil" }),
    );
    assert_eq!(split["kept"], anvils);
    assert_eq!(split["moved"], json!([third]));
    let other = split["identity"].clone();
    assert_eq!(members(&server, &anvils), ["1", "2"]);
    assert_eq!(members(&server, &other), ["3"]);

    // The split recorded that the sides stay apart.
    let constraints = server.read("/v1/constraints");
    let [constraint] = &constraints["constraints"].as_array().unwrap()[..] else {
        panic!("one constraint: {constraints}");
    };
    assert_eq!(constraint["kind"], "cannot_link");
    assert_eq!(constraint["from"], split["committed"]["seq"]);
    assert_eq!(constraint["to"], Value::Null);
    assert!([&constraint["a"], &constraint["b"]].contains(&&third));
    for filter in [
        format!("record={}", third.as_str().unwrap()),
        format!("identity={}", other.as_str().unwrap()),
        format!("identity={}", anvils.as_str().unwrap()),
    ] {
        assert_eq!(
            server.read(&format!("/v1/constraints?{filter}")),
            constraints,
            "{filter}"
        );
    }
    let unrelated = thing(&server, "2")["id"].clone();
    let none = server.read(&format!(
        "/v1/constraints?record={}",
        unrelated.as_str().unwrap()
    ));
    let named = [&constraint["a"], &constraint["b"]].contains(&&unrelated);
    assert_eq!(
        none["constraints"].as_array().unwrap().len(),
        named as usize
    );

    // Merging the sides again would break the constraint, and is refused.
    let merge = server.post(
        "/v1/changesets",
        json!({
            "ops": [{ "op": "merge", "from": other, "into": anvils }],
        }),
    );
    assert_eq!(reasons(&merge), ["constraint_violated"]);

    // A run does not bring them together either: an anvil that matches both
    // sides is left for a reviewer.
    stock(&server, &[("4", "anvil")]);
    assert_eq!(thing(&server, "4")["identity"], Value::Null);
    let open = server.read("/v1/review?status=open");
    let [item] = &open["items"].as_array().unwrap()[..] else {
        panic!("one open review item: {open}");
    };
    let id = item["id"].as_u64().unwrap();
    assert_eq!(server.read(&format!("/v1/review/{id}")), *item);

    // The reviewer closes the item without placing the anvil, and later runs
    // leave it alone.
    let dismissed = server.call(
        &format!("/v1/review/{id}/dismiss"),
        json!({ "reason": "not ours" }),
    );
    assert_eq!(dismissed["status"], "dismissed");
    assert_eq!(dismissed["dismissed"]["by"], REVIEWER);
    assert_eq!(dismissed["dismissed"]["reason"], "not ours");
    assert_eq!(server.read(&format!("/v1/review/{id}")), dismissed);
    assert_eq!(
        server.read("/v1/review?status=dismissed")["items"],
        json!([dismissed])
    );
    let run = server.call("/v1/match", json!({ "entity_type": "thing" }));
    assert_eq!(run["run"], Value::Null, "there is nothing to examine");
    assert_eq!(server.read("/v1/review?status=open")["items"], json!([]));
    assert_eq!(thing(&server, "4")["identity"], Value::Null);

    // Removing the constraint leaves it in the history.
    let removed = server.call(
        "/v1/changesets",
        json!({
            "ops": [{ "op": "unconstrain", "kind": "cannot_link", "a": constraint["a"], "b": constraint["b"] }],
        }),
    );
    assert_eq!(server.read("/v1/constraints")["constraints"], json!([]));
    let history = server.read("/v1/constraints?removed=true");
    assert_eq!(history["constraints"][0]["to"], removed["seq"]);

    // Calls that cannot be made say why.
    let dismiss = |item: u64| server.post(&format!("/v1/review/{item}/dismiss"), json!({}));
    dismiss(id).error(409, "conflict");
    dismiss(999).error(404, "not_found");
    server.get("/v1/review/999").error(404, "not_found");
    let split = |identity: &str, records: Value| {
        server.post(
            &format!("/v1/identities/{identity}/split"),
            json!({ "records": records }),
        )
    };
    split(anvils.as_str().unwrap(), json!([third])).error(409, "conflict");
    split(anvils.as_str().unwrap(), json!([])).error(409, "conflict");
    split("00000000-0000-7000-8000-000000000000", json!([third])).error(404, "not_found");
    server
        .get("/v1/constraints?kind=cannot_link")
        .error(400, "invalid");
}

#[test]
fn a_row_that_changed_is_read_with_the_version_it_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let server = things_server(dir.path());
    stock(&server, &[("1", "anvil")]);
    let first = thing(&server, "1");

    let rows = json!([{ "id": "1", "code": "anvil", "colour": "black" }]);
    let load = server.call(
        "/v1/load",
        json!({ "source": "shop", "entity_type": "thing", "rows": rows }),
    );
    assert_eq!(load["run"]["load"]["superseded"], 1);
    server.call("/v1/match", json!({ "entity_type": "thing" }));

    let versions = server.read("/v1/records?source=shop&key=1");
    let versions = versions["records"].as_array().unwrap();
    assert_eq!(versions.len(), 2);
    let old = server.read(&format!("/v1/records/{}", first["id"].as_str().unwrap()));
    let new = versions
        .iter()
        .find(|version| version["id"] != first["id"])
        .unwrap();
    assert_eq!(new["supersedes"], first["id"]);
    assert_eq!(old["superseded_by"], new["id"]);
    // The new version belongs where the old one does.
    assert_eq!(new["identity"], first["identity"]);
    assert_eq!(old["identity"], first["identity"]);
}
