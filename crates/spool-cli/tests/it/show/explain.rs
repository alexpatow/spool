//! `spool explain`: why two records are together or apart, link by link.

use super::*;

#[test]
fn explain_says_why_two_records_are_together_or_apart() {
    let dir = tempfile::tempdir().unwrap();
    let server = workshop(dir.path());
    let (one, two, red) = (
        id_of(&server, "shop", "1"),
        id_of(&server, "shop", "2"),
        id_of(&server, "shop", "3"),
    );

    let together = shown(&server, &["explain", "shop:1", "shop:2"]);
    let mut lines = together.lines();
    let first = lines.next().unwrap();
    assert!(
        first.starts_with("`shop:") && first.contains("belong to the same identity, "),
        "{together}"
    );
    has(
        &together,
        "Run 2 (strategy `rules`) judged them `match` (match rule `same_code` held).",
    );
    has(&together, "features");
    has(&together, "  code    anvil  anvil");
    has(&together, "  colour  black  black");
    has(&together, "rules as they are now");
    has(&together, "  block     same_code     holds");
    has(&together, "  no_match  other_colour  does not hold");
    has(&together, "  match     same_code     holds");
    has(&together, "runs that compared them");
    has(
        &together,
        "  run 2  rules  succeeded  block: same_code  match",
    );
    // Either way of naming a record will do, in either order.
    assert_eq!(shown(&server, &["explain", &two, "shop:1"]), together);

    let apart = shown(&server, &["explain", "shop:1", &red]);
    has(
        &apart,
        "Run 2 (strategy `rules`) judged them `no_match` (no_match rule `other_colour` held).",
    );
    has(&apart, "  no_match  other_colour  holds");
    has(
        &apart,
        "  run 2  rules  succeeded  block: same_code  no_match",
    );

    let never = shown(&server, &["explain", "shop:1", "shop:4"]);
    has(&never, "No run has compared them.");
    has(&never, "  block     same_code     does not hold");
    has(
        &never,
        "  no_match  other_colour  cannot be decided: a feature it reads is null",
    );
    assert!(!never.contains("runs that compared them"), "{never}");

    let answer = answered(&server, &["explain", "shop:1", "shop:2"]);
    assert_eq!(answer, server.read(&format!("/v1/explain?a={one}&b={two}")));
    assert_eq!(answer["outcome"], "same_identity");

    // A reviewer's decisions are part of the explanation.
    let identity = answer["left"]["record"]["identity"].as_str().unwrap();
    server.call(
        &format!("/v1/identities/{identity}/split"),
        json!({ "records": [two], "reason": "a replica" }),
    );
    let split = shown(&server, &["explain", "shop:1", "shop:2"]);
    has(
        &split,
        "A `cannot_link` constraint committed at log position 2 keeps them apart, whatever a strategy judges.",
    );
    has(&split, "constraints between them");
    has(&split, "  cannot_link from log position 2");
    has(
        &split,
        "`shop:2` was placed at log position 2 by `agent:api` (\"a replica\").",
    );
    let identity = shown(&server, &["show", "identities", identity]);
    has(
        &identity,
        "CONSTRAINT   BETWEEN                               AND                                   SINCE",
    );
    assert!(identity.contains("\ncannot_link  "), "{identity}");
    assert!(
        identity.contains("  agent:api  \"a replica\"\n"),
        "{identity}"
    );
    assert!(
        identity.contains(&format!("      move record {two} from ")),
        "{identity}"
    );

    let (code, complaint) = refused(&server, &["explain"]);
    assert_eq!(code, Some(2));
    assert!(
        complaint.contains("required arguments were not provided:\n  <RECORD>"),
        "{complaint}"
    );
    let (code, complaint) = refused(&server, &["explain", "shop:1", "anvil"]);
    assert_eq!(code, Some(2));
    assert!(
        complaint.contains("`anvil` does not name a record"),
        "{complaint}"
    );
    let (code, complaint) = refused(&server, &["explain", "shop:1", "shop:9"]);
    assert_eq!(code, Some(1));
    assert_eq!(
        complaint,
        "error: there is no record with key `9` from source `shop`\n"
    );
    let (code, complaint) = refused(&server, &["explain", "shop:1", "shop:1"]);
    assert_eq!(code, Some(1));
    assert!(complaint.contains("named twice"), "{complaint}");
    let unknown = "00000000-0000-7000-8000-000000000000";
    let (code, complaint) = refused(&server, &["explain", "shop:1", unknown]);
    assert_eq!(code, Some(1));
    assert_eq!(complaint, format!("error: there is no record {unknown}\n"));
}

/// Things matched by code and by name: an anvil and a tongs that share no
/// rule are together through a second anvil with the tongs' name.
fn chained(dir: &Path) -> Server {
    let project = dir.join("project");
    write_things(&project);
    std::fs::write(
        project.join("entities/thing/entity.yaml"),
        "label: code\nfeatures: [code, colour, name]\nmapping:\n  shop:\n    code: code\n    colour: colour\n    name: name\n",
    )
    .unwrap();
    for group in ["block", "match"] {
        let file = project.join(format!("entities/thing/rules/{group}/same_name.sql"));
        std::fs::write(file, "l.name = r.name\n").unwrap();
    }
    let server = Server::serve(&project, dir, 1);
    load(
        &server,
        "shop",
        "thing",
        rows(&[
            &["id", "code", "colour", "name"],
            &["1", "anvil", "", "Acme"],
            &["2", "anvil", "", "Smith"],
            &["3", "tongs", "", "Smith"],
            &["4", "tongs", "", "Jones"],
        ]),
    );
    assert_eq!(resolve(&server, "thing")["status"], "succeeded");
    server
}

#[test]
fn explain_names_each_link_of_a_chain_by_its_records_and_its_rule() {
    let dir = tempfile::tempdir().unwrap();
    let server = chained(dir.path());

    let shown_chain = shown(&server, &["explain", "shop:1", "shop:4"]);
    has(&shown_chain, "what joins them");
    // From whichever of the two has the smaller ID.
    let forward = shown_chain.contains("  1. anvil  shop:1");
    let steps: [&str; 4] = [
        "anvil  shop:1",
        "anvil  shop:2",
        "tongs  shop:3",
        "tongs  shop:4",
    ];
    let hows = [
        "`match` in run 2, rule `same_code`: code \"anvil\"",
        "`match` in run 2, rule `same_name`: name \"Smith\"",
        "`match` in run 2, rule `same_code`: code \"tongs\"",
    ];
    let (steps, hows): (Vec<&str>, Vec<&str>) = match forward {
        true => (steps.to_vec(), hows.to_vec()),
        false => (
            steps.into_iter().rev().collect(),
            hows.into_iter().rev().collect(),
        ),
    };
    for (at, step) in steps.iter().enumerate() {
        has(&shown_chain, &format!("{}. {step}", at + 1));
    }
    for how in &hows {
        has(&shown_chain, how);
    }
    assert!(
        shown_chain.contains("They are joined through 2 other members: `shop:"),
        "{shown_chain}"
    );
    // The one link between the anvils and the tongs parts them.
    has(&shown_chain, "what would part them");
    has(
        &shown_chain,
        match forward {
            true => "anvil  shop:2 – tongs  shop:3",
            false => "tongs  shop:3 – anvil  shop:2",
        },
    );
    has(
        &shown_chain,
        "`match` in run 2, rule `same_name`: name \"Smith\"",
    );
    let (near, far) = match forward {
        true => (
            "with left: 2 records (2 × anvil)",
            "with right: 2 records (2 × tongs)",
        ),
        false => (
            "with left: 2 records (2 × tongs)",
            "with right: 2 records (2 × anvil)",
        ),
    };
    has(&shown_chain, near);
    has(&shown_chain, far);

    let answer = answered(&server, &["explain", "shop:1", "shop:4"]);
    let path = answer["path"].as_array().unwrap();
    assert_eq!(path.len(), 3);
    assert_eq!(path[1]["rules"], json!(["same_name"]));
    assert_eq!(
        path[1]["values"],
        json!([{ "feature": "name", "from": "Smith", "to": "Smith" }])
    );
    assert_eq!(answer["cut"]["links"].as_array().unwrap().len(), 1);

    list_identities_by_unlinked_pairs(&server);

    // An identity is explained by its ID or by any of its records.
    let identity = answer["left"]["record"]["identity"]
        .as_str()
        .unwrap()
        .to_owned();
    let explained = shown(&server, &["explain", &identity]);
    assert_eq!(shown(&server, &["explain", "shop:3"]), explained);
    assert!(
        explained.contains("is a `thing` identity of 4 records with 2 different names.\n"),
        "{explained}"
    );
    has(
        &explained,
        "Without the 1 link only rule `same_name` made, it would fall into 2 pieces: 2 (2 × `tongs`), 2 (2 × `anvil`).",
    );
    has(&explained, "same_name  1  1  2  2 (tongs), 2 (anvil)");
    has(
        &explained,
        "Of its 6 pairs of records, 3 are linked directly and 3 are together only through a chain of others, the longest of at least 3 links.",
    );
    // The link between the two names parts it evenly, so it comes first,
    // with either name on the first side.
    assert!(
        explained.contains(
            "Removing 1 link would part 2 records (2 × `anvil`) from the other 2 (2 × `tongs`)."
        ) || explained.contains(
            "Removing 1 link would part 2 records (2 × `tongs`) from the other 2 (2 × `anvil`)."
        ),
        "{explained}"
    );
    has(&explained, "only through chains  3");
    has(&explained, "weakest 1: 1 link would part it");
    assert!(
        explained.contains("  anvil  shop:2 – tongs  shop:3\n")
            || explained.contains("  tongs  shop:3 – anvil  shop:2\n"),
        "{explained}"
    );
    let answer = answered(&server, &["explain", &identity]);
    assert_eq!(
        answer,
        server.read(&format!("/v1/identities/{identity}/explain"))
    );
    assert_eq!(answer["chained_pairs"], 3);
    assert_eq!(answer["weakest"][0]["sides"][1]["records"], 2);

    let unknown = "00000000-0000-7000-8000-000000000000";
    let (code, complaint) = refused(&server, &["explain", unknown]);
    assert_eq!(code, Some(1));
    assert_eq!(
        complaint,
        format!("error: there is no identity or record {unknown}\n")
    );
    server
        .get(&format!("/v1/identities/{unknown}/explain"))
        .error(404, "not_found");
}

#[test]
fn a_type_with_no_rules_is_explained() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    std::fs::remove_dir_all(project.join("entities/thing/rules")).unwrap();
    let server = Server::serve(&project, dir.path(), 1);
    load(
        &server,
        "shop",
        "thing",
        rows(&[&["id", "code"], &["1", "anvil"], &["2", "anvil"]]),
    );

    let explanation = shown(&server, &["explain", "shop:1", "shop:2"]);
    has(
        &explanation,
        "The entity type has no block rule, so a run of the rules would not compare them.",
    );
    has(&explanation, "rules as they are now");
    has(&explanation, "  the entity type has none");
}

#[test]
fn a_rule_that_sql_refuses_on_a_pair_is_shown_as_one_that_cannot_be_applied() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_things(&project);
    // The match rule is safe on what the block rule lets through.
    for (file, sql) in [
        (
            "rules/block/same_code.sql",
            "l.code = r.code AND TRY_CAST(l.code AS INTEGER) IS NOT NULL\n",
        ),
        (
            "rules/match/same_code.sql",
            "CAST(l.code AS INTEGER) = CAST(r.code AS INTEGER)\n",
        ),
    ] {
        std::fs::write(project.join("entities/thing").join(file), sql).unwrap();
    }
    let server = Server::serve(&project, dir.path(), 1);
    load(
        &server,
        "shop",
        "thing",
        rows(&[&["id", "code"], &["1", "anvil"], &["2", "anvil"]]),
    );
    assert_eq!(resolve(&server, "thing")["status"], "succeeded");

    let explanation = shown(&server, &["explain", "shop:1", "shop:2"]);
    has(&explanation, "  block     same_code     does not hold");
    has(
        &explanation,
        "  match     same_code     cannot be applied to these values",
    );
    has(
        &explanation,
        "  applying the `no_match` and `match` rules failed: Conversion Error: Could not convert string 'anvil' to INT32 when casting from source column code",
    );
    // The server answers the next call.
    has(&shown(&server, &["show"]), "  identities  2");
}

/// After a hammer joins the chained things, the list says how many names
/// each identity has and how much of it only chains hold, largest first.
fn list_identities_by_unlinked_pairs(server: &Server) {
    load(
        server,
        "shop",
        "thing",
        rows(&[
            &["id", "code", "colour", "name"],
            &["5", "hammer", "", "Pein"],
        ]),
    );
    resolve(server, "thing");
    let largest = shown(
        server,
        &["show", "identities", "--sort", "records", "--order", "desc"],
    );
    let lines: Vec<&str> = largest.lines().collect();
    assert_eq!(
        lines[0].split_whitespace().collect::<Vec<_>>(),
        [
            "IDENTITY", "NAME", "MEMBERS", "NAMES", "UNLINKED", "PAIRS", "CREATED", "AT"
        ]
    );
    assert_eq!(
        lines[1].split_whitespace().skip(2).collect::<Vec<_>>(),
        ["4", "2", "50%", "log", "position", "1"],
        "{largest}"
    );
    assert_eq!(
        lines[2].split_whitespace().skip(1).collect::<Vec<_>>(),
        ["hammer", "1", "1", "-", "log", "position", "2"],
        "{largest}"
    );
    let listed = answered(
        server,
        &["show", "identities", "--sort", "names", "--order", "desc"],
    );
    assert_eq!(listed["identities"][0]["unlinked_pairs"], 3);
    assert_eq!(listed["identities"][0]["linked_pairs"], 3);
    let (code, complaint) = refused(server, &["show", "identities", "--sort", "size"]);
    assert_eq!(code, Some(2));
    assert!(
        complaint.contains("invalid value 'size' for '--sort <SORT>'"),
        "{complaint}"
    );
    let (code, complaint) = refused(server, &["show", "runs", "--sort", "records"]);
    assert_eq!(code, Some(2));
    assert!(
        complaint.contains("unexpected argument '--sort'"),
        "{complaint}"
    );
}
