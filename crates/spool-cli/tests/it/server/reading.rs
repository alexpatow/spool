//! What the server answers about what it holds: lists, pages and explanations.

use super::*;

#[test]
fn identities_records_and_review_items_are_looked_up() {
    let library = library(5);
    let dir = tempfile::tempdir().unwrap();
    let server = company_server(dir.path(), 5);
    run_companies(&server, Encoding::Arrow);

    // A record is found by its source and key, and names its identity.
    let found = server.read("/v1/records?source=crm&key=c-101");
    let [record] = &found["records"].as_array().unwrap()[..] else {
        panic!("one record: {found}");
    };
    assert_eq!(record["source"], "crm");
    assert_eq!(record["key"], "c-101");
    assert_eq!(record["entity_type"], "company");
    assert_eq!(record["payload"]["id"], "c-101");
    let id = record["id"].as_str().unwrap();
    let identity = record["identity"].as_str().unwrap();
    assert_eq!(server.read(&format!("/v1/records/{id}")), *record);

    // The identity lists its members, and its history names the changesets.
    let view = server.read(&format!("/v1/identities/{identity}"));
    assert_eq!(
        view,
        serde_json::to_value(library.identity(identity.parse().unwrap()).unwrap()).unwrap()
    );
    assert_eq!(view["current"], identity);
    let members = view["members"].as_array().unwrap();
    assert!(members.len() > 1, "{view}");
    assert!(members.iter().any(|member| member["record"] == id));
    let history = server.read(&format!("/v1/identities/{identity}/history"));
    assert_eq!(
        history["history"],
        serde_json::to_value(library.history(identity.parse().unwrap()).unwrap()).unwrap()
    );
    assert_eq!(history["history"][0]["seq"], 1);
    assert_eq!(history["history"][0]["ops"][0]["op"], "create_identity");

    // Many records are looked up in one call; an unknown one has none.
    let unknown = "00000000-0000-7000-8000-000000000000";
    let identities = server.call(
        "/v1/records/identities",
        json!({ "records": [id, unknown] }),
    );
    assert_eq!(identities, json!({ "identities": [identity, null] }));

    // Lists come in pages.
    let total = server.read("/v1/info")["store"]["entity_types"][0]["identities"]
        .as_u64()
        .unwrap();
    let mut listed = Vec::new();
    let mut after = String::new();
    loop {
        let page = server.read(&format!(
            "/v1/identities?entity_type=company&limit=4{after}"
        ));
        let identities = page["identities"].as_array().unwrap();
        assert!(identities.len() <= 4);
        listed.extend(identities.iter().map(|identity| identity["id"].clone()));
        match page["next"].as_str() {
            Some(next) => after = format!("&after={next}"),
            None => break,
        }
    }
    assert_eq!(listed.len() as u64, total);
    assert!(listed.contains(&json!(identity)));
    let first = server.read("/v1/records?source=registry&limit=3");
    assert_eq!(first["records"].as_array().unwrap().len(), 3);
    let next = first["next"].as_str().unwrap();
    let second = server.read(&format!("/v1/records?source=registry&limit=3&after={next}"));
    assert_ne!(second["records"][0]["id"], first["records"][2]["id"]);

    // The record that fits two identities is unassigned, and its review item
    // comes back typed, the same as the library reads it.
    let open = server.read("/v1/review?status=open");
    assert_eq!(
        open["items"],
        serde_json::to_value(
            library
                .review_items(&ReviewFilter {
                    status: Some(ReviewStatus::Open),
                    ..ReviewFilter::default()
                })
                .unwrap()
        )
        .unwrap()
    );
    let [item] = &open["items"].as_array().unwrap()[..] else {
        panic!("one open review item: {open}");
    };
    assert_eq!(item["entity_type"], "company");
    assert_eq!(item["status"], "open");
    assert_eq!(item["identities"].as_array().unwrap().len(), 2);
    let stuck = server.read("/v1/records?source=crm&key=c-117");
    assert_eq!(stuck["records"][0]["identity"], Value::Null);
    assert_eq!(item["records"], json!([stuck["records"][0]["id"]]));
    let waiting = server.read("/v1/records?unassigned=true");
    assert_eq!(waiting["records"], stuck["records"]);
    assert_eq!(
        server.read("/v1/info")["store"]["entity_types"][0]["open_review_items"],
        1
    );
    assert_eq!(
        server.read("/v1/review?entity_type=company")["items"],
        open["items"]
    );
}

/// Every page of a list, read by passing each page's `next` as `after`.
fn every_page(server: &Server, path: &str, list: &str) -> Vec<Value> {
    let mut listed = Vec::new();
    let mut after = String::new();
    loop {
        let page = server.read(&format!("{path}{after}"));
        listed.extend(page[list].as_array().unwrap().iter().cloned());
        match &page["next"] {
            Value::Null => break,
            next => after = format!("&after={}", next.as_str().unwrap()),
        }
    }
    listed
}

#[test]
fn lists_are_sorted_as_the_query_says_and_read_in_pages_in_that_order() {
    let dir = tempfile::tempdir().unwrap();
    let server = company_server(dir.path(), 5);
    run_companies(&server, Encoding::Arrow);

    // Pages of a sorted list walk the order of the whole of it.
    let whole = server.read("/v1/identities?sort=records&order=desc&limit=1000")["identities"]
        .as_array()
        .unwrap()
        .clone();
    assert!(whole.len() > 8, "{whole:?}");
    let members: Vec<u64> = whole
        .iter()
        .map(|identity| identity["members"].as_u64().unwrap())
        .collect();
    assert!(
        members.windows(2).all(|pair| pair[0] >= pair[1]),
        "{members:?}"
    );
    assert!(members[0] > *members.last().unwrap(), "{members:?}");
    assert_eq!(
        every_page(
            &server,
            "/v1/identities?sort=records&order=desc&limit=3",
            "identities"
        ),
        whole
    );
    let records = server.read("/v1/records?sort=label&limit=1000")["records"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(
        every_page(&server, "/v1/records?sort=label&limit=4", "records"),
        records
    );
    let labels: Vec<String> = records
        .iter()
        .filter_map(|record| record["label"].as_str().map(str::to_lowercase))
        .collect();
    assert!(
        labels.windows(2).all(|pair| pair[0] <= pair[1]),
        "{labels:?}"
    );

    // A list that is cut at `limit` is sorted before it is cut.
    let runs = server.read("/v1/runs?sort=kind&limit=1000")["runs"].clone();
    assert_eq!(
        server.read("/v1/runs?sort=kind&limit=2")["runs"]
            .as_array()
            .unwrap()[..],
        runs.as_array().unwrap()[..2]
    );
    // `order` alone turns the list's own order: runs are newest first.
    assert_eq!(
        server.read("/v1/runs?order=asc&limit=1")["runs"][0]["run"],
        1
    );
    assert_ne!(server.read("/v1/runs?limit=1")["runs"][0]["run"], 1);

    // Who did something is sorted by what they are called, which the
    // server knows and the store does not.
    server.get("/v1/runs?sort=started_by").expect(200);
    server
        .get("/v1/changesets?sort=actor&order=desc")
        .expect(200);

    // The log, newest first, a page at a time after a position.
    let head = server.read("/v1/info")["store"]["head"].as_u64().unwrap();
    let newest = server.read("/v1/changesets?sort=seq&order=desc&limit=1");
    assert_eq!(newest["changesets"][0]["seq"], head);
    let next = server.read(&format!(
        "/v1/changesets?sort=seq&order=desc&limit=1&after={head}"
    ));
    assert_eq!(next["changesets"][0]["seq"], head - 1);

    // What a list cannot be sorted by is refused, with what it can be.
    let refused = server.get("/v1/identities?sort=colour").expect(400);
    let said = refused["error"]["message"].as_str().unwrap();
    assert!(
        said.contains("colour") && said.contains("records"),
        "{said}"
    );
    server.get("/v1/documents?order=sideways").expect(400);
}

#[test]
#[expect(clippy::too_many_lines)]
fn what_is_read_says_what_things_are_called_where_they_came_from_and_what_waits() {
    let library = library(5);
    let dir = tempfile::tempdir().unwrap();
    let server = company_server(dir.path(), 5);
    run_companies(&server, Encoding::Json);

    // A record is called what its payload names it, whatever the field.
    let crm = server.read("/v1/records?source=crm&key=c-102");
    assert_eq!(crm["records"][0]["label"], "Acorn Tools");
    let registry = server.read("/v1/records?source=registry&key=X0000002");
    assert_eq!(registry["records"][0]["label"], "ACORN TOOL COMPANY LTD");
    let identity = crm["records"][0]["identity"].as_str().unwrap();
    assert_eq!(registry["records"][0]["identity"], identity);

    // An identity is called what most of its records are, and says how many
    // each source gave.
    let view = server.read(&format!("/v1/identities/{identity}"));
    assert_eq!(view["label"], "Acorn Tools");
    let page = server.read("/v1/identities?limit=1000");
    assert_eq!(
        page["identities"],
        serde_json::to_value(
            library
                .identities(&IdentityFilter {
                    limit: 1000,
                    ..IdentityFilter::default()
                })
                .unwrap()
        )
        .unwrap()
    );
    let listed = page["identities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|listed| listed["id"] == identity)
        .unwrap();
    assert_eq!(listed["label"], "Acorn Tools");
    assert_eq!(
        listed["sources"],
        json!([
            { "source": "crm", "records": 2 },
            { "source": "registry", "records": 1 },
        ])
    );

    // Its members are read as records, with what each of them says.
    let members = server.read(&format!("/v1/records?identity={identity}"));
    let mut labels: Vec<&str> = members["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|member| member["label"].as_str().unwrap())
        .collect();
    labels.sort();
    assert_eq!(
        labels,
        ["ACORN TOOL COMPANY LTD", "Acorn Tool Co.", "Acorn Tools"]
    );
    server
        .get("/v1/records?identity=acorn")
        .error(400, "invalid");

    // Identities are read by their IDs, as the list gives them.
    let all = page["identities"].as_array().unwrap();
    let other = all.iter().find(|listed| listed["id"] != identity).unwrap()["id"]
        .as_str()
        .unwrap();
    let named = server.read(&format!("/v1/identities?ids={other},{identity}"));
    let wanted: Vec<&serde_json::Value> = all
        .iter()
        .filter(|listed| listed["id"] == identity || listed["id"] == other)
        .collect();
    assert_eq!(named["identities"], json!(wanted));
    assert_eq!(named["next"], json!(null));
    let filtered = server.read(&format!("/v1/identities?ids={identity}&q=sheffield"));
    assert_eq!(filtered["identities"].as_array().unwrap().len(), 1);
    assert_eq!(filtered["identities"][0]["id"], identity);
    assert_eq!(filtered["identities"][0]["match"]["kind"], "record");
    assert_eq!(filtered["identities"][0]["match"]["field"], "city");
    assert_eq!(server.read("/v1/identities?ids=")["identities"], json!([]));
    server
        .get(&format!("/v1/identities?ids={identity},acorn"))
        .error(400, "invalid");
    let many = vec![identity; 1001].join(",");
    server
        .get(&format!("/v1/identities?ids={many}"))
        .error(400, "invalid");

    // Records and identities are found by the words they hold.
    let found = server.read("/v1/identities?q=acorn%20sheffield");
    assert_eq!(found["identities"].as_array().unwrap().len(), 1);
    assert_eq!(found["identities"][0]["id"], identity);
    let found = server.read("/v1/records?q=ACORN&source=registry");
    assert_eq!(found["records"], registry["records"]);
    assert_eq!(
        server.read("/v1/records?q=no-such-company")["records"],
        json!([])
    );

    // The store says what each source gave.
    let info = server.read("/v1/info");
    let sources = info["store"]["entity_types"][0]["sources"]
        .as_array()
        .unwrap();
    assert_eq!(
        sources
            .iter()
            .map(|source| (
                source["source"].as_str().unwrap(),
                source["records"].as_u64().unwrap(),
                source["unassigned"].as_u64().unwrap()
            ))
            .collect::<Vec<_>>(),
        [("crm", 23, 1), ("registry", 16, 0)]
    );
    assert_eq!(
        info["store"],
        serde_json::to_value(library.info().unwrap()).unwrap()
    );
    assert_eq!(
        sources[0]["records"].as_u64().unwrap() + sources[1]["records"].as_u64().unwrap(),
        info["store"]["entity_types"][0]["records"]
    );

    // A review item is read with the records it is about: the one that
    // waits, the identities it matched with their members, and the
    // judgements that say so.
    let case = server.read("/v1/review/1/case");
    assert_eq!(
        case,
        serde_json::to_value(library.review_case(1).unwrap()).unwrap()
    );
    assert_eq!(case["item"], server.read("/v1/review/1"));
    assert_eq!(case["records"][0]["key"], "c-117");
    assert!(case["records"][0]["label"].is_string());
    let candidates = case["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 2);
    for candidate in candidates {
        assert!(candidate["identity"]["label"].is_string(), "{candidate}");
        assert_eq!(
            candidate["members"].as_array().unwrap().len() as u64,
            candidate["identity"]["members"].as_u64().unwrap()
        );
    }
    let judgements = case["judgements"].as_array().unwrap();
    assert!(judgements.len() >= 2, "{case}");
    assert!(
        judgements
            .iter()
            .all(|judgement| judgement["record"] == case["records"][0]["id"])
    );
    assert!(
        judgements
            .iter()
            .any(|judgement| judgement["verdict"] == "match")
    );
    // Each names the candidate it is about.
    for judgement in judgements {
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate["identity"]["id"] == judgement["identity"]),
            "{judgement}"
        );
    }
    server.get("/v1/review/99/case").error(404, "not_found");
    server.get("/v1/review/x/case").error(400, "invalid");
}

#[test]
#[expect(clippy::too_many_lines)]
fn a_pair_of_records_is_explained() {
    let dir = tempfile::tempdir().unwrap();
    let server = things_server(dir.path());
    load_things(
        &server,
        "shop",
        &[
            &["id", "code", "colour"],
            &["1", "anvil", "black"],
            &["2", "anvil", "black"],
            &["3", "anvil", "red"],
        ],
    );
    server.call("/v1/match", json!({ "entity_type": "thing" }));
    let id = |key: &str| {
        server.read(&format!("/v1/records?source=shop&key={key}"))["records"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let (first, second, red) = (id("1"), id("2"), id("3"));

    let explanation = server.read(&format!("/v1/explain?a={first}&b={second}"));
    assert_eq!(explanation["outcome"], "same_identity");
    assert_eq!(
        explanation["left"]["features"],
        json!({ "code": "anvil", "colour": "black" })
    );
    assert_eq!(explanation["comparisons"][0]["strategy"]["name"], "rules");
    assert_eq!(
        explanation["comparisons"][0]["judgements"][0]["verdict"],
        "match"
    );
    assert_eq!(explanation["rules"]["verdict"], "match");
    assert_eq!(explanation["path"][0]["kind"], "match");
    assert!(
        explanation["summary"][0]
            .as_str()
            .unwrap()
            .contains("belong to the same identity"),
        "{explanation}"
    );
    // The two can be named in either order.
    assert_eq!(
        server.read(&format!("/v1/explain?a={second}&b={first}")),
        explanation
    );

    let apart = server.read(&format!("/v1/explain?a={first}&b={red}"));
    assert_eq!(apart["outcome"], "different_identities");
    assert_eq!(apart["rules"]["verdict"], "no_match");
    assert_eq!(
        apart["rules"]["no_match"],
        json!([{ "name": "other_colour", "predicate": "l.colour <> r.colour", "holds": true }])
    );
    assert_eq!(apart["path"], json!([]));

    let unknown = "00000000-0000-7000-8000-000000000000";
    let message = server
        .get(&format!("/v1/explain?a={first}&b={unknown}"))
        .error(404, "not_found");
    assert_eq!(message, format!("there is no record {unknown}"));
    let message = server
        .get(&format!("/v1/explain?a={first}&b={first}"))
        .error(400, "invalid");
    assert!(message.contains("named twice"), "{message}");
    server
        .get(&format!("/v1/explain?a={first}"))
        .error(400, "invalid");
    let message = server
        .get(&format!("/v1/explain?a={first}&b=nothing"))
        .error(400, "invalid");
    assert!(message.contains("<source>:<key>"), "{message}");

    // A record is named by its source and key as well. That is the row the
    // source holds under the key now, whatever it held before.
    assert_eq!(server.read("/v1/explain?a=shop:1&b=shop:2"), explanation);
    assert_eq!(
        server.read(&format!("/v1/explain?a=shop:3&b={first}")),
        server.read(&format!("/v1/explain?a={red}&b={first}"))
    );
    let message = server
        .get("/v1/explain?a=shop:1&b=shop:9")
        .error(404, "not_found");
    assert_eq!(
        message,
        "there is no record with key `9` from source `shop`"
    );
    let unknown = server
        .get("/v1/explain?a=shop:1&b=shed:2")
        .error(404, "not_found");
    assert!(
        unknown.starts_with(
            "there is no record with key `2` from source `shed`: the project's sources are `"
        ),
        "{unknown}"
    );
    assert!(unknown.contains("`shop`"), "{unknown}");
    load_things(
        &server,
        "shop",
        &[
            &["id", "code", "colour"],
            &["2", "anvil", "blue"],
            &["a:b", "anvil", "black"],
        ],
    );
    let versions = server.read("/v1/records?source=shop&key=2");
    let versions = versions["records"].as_array().unwrap();
    assert_eq!(versions.len(), 2);
    let latest = versions
        .iter()
        .find(|version| version["superseded_by"].is_null())
        .unwrap();
    assert_eq!(latest["payload"]["colour"], "blue");
    let by_key = server.read("/v1/explain?a=shop:1&b=shop:2");
    let (left, right) = (&by_key["left"]["record"], &by_key["right"]["record"]);
    assert!(
        left["id"] == latest["id"] || right["id"] == latest["id"],
        "{by_key}"
    );
    // A key may have a colon in it: the first one ends the source's name.
    let colon = server.read("/v1/explain?a=shop:1&b=shop:a:b");
    let keys = [
        &colon["left"]["record"]["key"],
        &colon["right"]["record"]["key"],
    ];
    assert!(keys.contains(&&json!("a:b")), "{colon}");

    // Review items are listed by a record they name.
    assert_eq!(
        server.read(&format!("/v1/review?record={first}"))["items"],
        json!([])
    );
    server
        .get("/v1/review?record=nothing")
        .error(400, "invalid");
}

#[test]
fn records_are_listed_by_source_and_key_and_the_sources_are_named() {
    let dir = tempfile::tempdir().unwrap();
    let server = things_server(dir.path());
    load_things(
        &server,
        "shop",
        &[
            &["id", "code"],
            &["b", "bolt"],
            &["10", "ten"],
            &["2", "two"],
        ],
    );
    load_things(&server, "bulk", &[&["id", "code"], &["z", "zip"]]);
    load_things(&server, "shop", &[&["id", "code"], &["10", "ten again"]]);

    // A source's rows are together, by key as text, and a row's versions
    // follow each other.
    let all = server.read("/v1/records")["records"].clone();
    let listed: Vec<(&str, &str, &str)> = all
        .as_array()
        .unwrap()
        .iter()
        .map(|record| {
            (
                record["source"].as_str().unwrap(),
                record["key"].as_str().unwrap(),
                record["payload"]["code"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        listed,
        [
            ("bulk", "z", "zip"),
            ("shop", "10", "ten"),
            ("shop", "10", "ten again"),
            ("shop", "2", "two"),
            ("shop", "b", "bolt"),
        ]
    );
    // Pages follow the same order: `next` is the last record of a page.
    let mut walked = Vec::new();
    let mut after = String::new();
    loop {
        let page = server.read(&format!("/v1/records?limit=2{after}"));
        walked.extend(page["records"].as_array().unwrap().iter().cloned());
        match page["next"].as_str() {
            Some(next) => {
                assert_eq!(walked.last().unwrap()["id"], next);
                after = format!("&after={next}");
            }
            None => break,
        }
    }
    assert_eq!(json!(walked), all);
    // A place in the list is a record the server holds.
    let message = server
        .get("/v1/records?after=00000000-0000-7000-8000-000000000000")
        .error(404, "not_found");
    assert_eq!(
        message,
        "there is no record 00000000-0000-7000-8000-000000000000"
    );

    // What a client needs to know to send rows: the sources, the columns
    // they list, and the entity types that read them.
    let sources = server.read("/v1/info")["store"]["sources"].clone();
    let names: Vec<&str> = sources
        .as_array()
        .unwrap()
        .iter()
        .map(|source| source["name"].as_str().unwrap())
        .collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted);
    let shop = sources
        .as_array()
        .unwrap()
        .iter()
        .find(|source| source["name"] == "shop")
        .unwrap();
    assert_eq!(
        *shop,
        json!({
            "name": "shop",
            "key": "id",
            "evidence": null,
            "columns": ["id", "code", "colour", "name", "weight"],
            "entity_types": ["thing"],
        })
    );
}
