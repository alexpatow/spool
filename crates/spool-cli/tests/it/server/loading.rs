//! Rows loaded over HTTP: the formats taken, and the rows refused.

use super::*;

#[test]
fn a_first_line_of_too_many_columns_is_refused_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let server = things_server(dir.path());
    let csv = |body: &str| {
        server
            .request("POST", "/v1/load?source=shop&entity_type=thing")
            .header("Content-Type", "text/csv")
            .body(body.as_bytes())
            .send()
    };
    let line = |columns: usize| -> String {
        let names: Vec<String> = (0..columns).map(|column| format!("c{column}")).collect();
        names.join(",")
    };

    // Half a megabyte of names and no rows: what it costs to refuse does
    // not grow faster than the line does.
    let began = std::time::Instant::now();
    let message = csv(&line(80_000)).error(400, "invalid");
    assert_eq!(
        message,
        "the body cannot be read as CSV: line 1 names 80000 columns, which is more than the \
         10000 a file of rows may have"
    );
    // As many as there may be are looked at, and so is a name that comes
    // twice among them.
    let message = csv(&format!("{},c0\n", line(9_999))).error(400, "invalid");
    assert_eq!(
        message,
        "the body cannot be read as CSV: line 1: two columns are named `c0`"
    );
    assert!(
        began.elapsed() < std::time::Duration::from_secs(5),
        "the first lines took {:?}",
        began.elapsed()
    );
    // The server goes on reading rows.
    let answer = csv("id,code\n1,anvil\n").ok();
    assert_eq!(answer["run"]["load"]["inserted"], 1, "{answer}");
}

#[test]
fn rows_are_taken_as_csv_and_as_tab_separated_text() {
    let dir = tempfile::tempdir().unwrap();
    let server = things_server(dir.path());
    let csv = |query: &str, content_type: &str, body: &str| {
        server
            .request("POST", &format!("/v1/load?{query}"))
            .header("Content-Type", content_type)
            .body(body.as_bytes())
            .send()
    };
    let shop = "source=shop&entity_type=thing";

    // The first line names the columns, and every value is the text the
    // line has. A field with nothing in it is a value the row does not have.
    let body = "id,code,colour,name\n1,anvil,black,\"Anvil, large\"\n2,hammer,,\"\"\n";
    let answer = csv(shop, "text/csv", body).ok();
    assert_eq!(answer["run"]["status"], "succeeded");
    assert_eq!(answer["run"]["load"]["received"], 2);
    assert_eq!(answer["run"]["load"]["inserted"], 2);
    assert_eq!(
        held(&server, "shop"),
        [
            (
                json!("1"),
                json!({ "id": "1", "code": "anvil", "colour": "black", "name": "Anvil, large" })
            ),
            (json!("2"), json!({ "id": "2", "code": "hammer" })),
        ]
    );

    // The same rows are the same records however they come: as CSV with
    // its character set named and Windows line ends, as tabs, and as JSON.
    let again = csv(shop, "text/csv; charset=utf-8", &body.replace('\n', "\r\n")).ok();
    assert_eq!(again["run"]["load"]["duplicates"], 2, "{again}");
    let tabs = "id\tcode\tcolour\tname\n1\tanvil\tblack\tAnvil, large\n2\thammer\t\t\n";
    let again = csv(shop, "text/tab-separated-values", tabs).ok();
    assert_eq!(again["run"]["load"]["duplicates"], 2, "{again}");
    let again = server.call(
        "/v1/load",
        json!({
            "source": "shop",
            "entity_type": "thing",
            "rows": [
                { "id": "1", "code": "anvil", "colour": "black", "name": "Anvil, large" },
                { "id": "2", "code": "hammer", "colour": null },
            ],
        }),
    );
    assert_eq!(again["run"]["load"]["duplicates"], 2, "{again}");
    assert_eq!(held(&server, "shop").len(), 2);

    // What is wrong with a body is said with the line it is on.
    let message = csv(shop, "text/csv", "id,code\n3,saw\n4,file,flat\n").error(400, "invalid");
    assert_eq!(
        message,
        "the body cannot be read as CSV: line 3 has 3 fields, and the first line names 2 columns"
    );
    let message = csv(shop, "text/tab-separated-values", "id\tcode\n3\n").error(400, "invalid");
    assert_eq!(
        message,
        "the body cannot be read as tab-separated text: line 2 has 1 field, and the first line \
         names 2 columns"
    );
    let message = csv(shop, "text/csv", "").error(400, "invalid");
    assert_eq!(
        message,
        "the body cannot be read as CSV: it is empty: the first line has to name the columns"
    );
    let message = csv(shop, "text/csv", "id,code,id\n3,saw,3\n").error(400, "invalid");
    assert_eq!(
        message,
        "the body cannot be read as CSV: line 1: two columns are named `id`"
    );
    // Where the rows go is in the query, as it is for Arrow.
    let message = csv("entity_type=thing", "text/csv", body).error(400, "invalid");
    assert!(message.contains("source"), "{message}");
    // A column the source does not list is refused, with the ones it does.
    let message = csv(shop, "text/csv", "id,code,price\n3,saw,12\n").error(400, "invalid");
    assert_eq!(
        message,
        "load rejected: the rows have a column `price`, which sources/shop.yaml does not \
         list. It lists `id`, `code`, `colour`, `name`, `weight`. Add the column to that file to \
         keep it, or leave it out of the rows"
    );
    assert_eq!(held(&server, "shop").len(), 2, "none of those stored a row");

    // The first line alone is no rows, which is not a mistake.
    let none = csv(shop, "text/csv", "id,code\n").ok();
    assert_eq!(none["run"]["load"]["received"], 0, "{none}");
}

#[test]
fn a_name_that_two_records_have_is_refused_with_their_ids() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    write_two_of_each(&project);
    let server = Server::serve(&project, dir.path(), 1);
    let load = |entity_type: &str, rows: Value| {
        let body = json!({ "source": "crm", "entity_type": entity_type, "rows": rows });
        server.call("/v1/load", body);
    };
    // The source sends rows of two types, and has a company and a person
    // under the key 1. Each is the row of its type that nothing replaced.
    load(
        "company",
        json!([{ "id": "1", "name": "Dana" }, { "id": "2", "name": "Dana" }]),
    );
    load("person", json!([{ "id": "1", "name": "Dana Reyes" }]));
    let held = |key: &str, entity_type: &str| -> String {
        let records = server.read(&format!(
            "/v1/records?source=crm&key={key}&entity_type={entity_type}"
        ));
        let current = records["records"]
            .as_array()
            .unwrap()
            .iter()
            .find(|record| record["superseded_by"].is_null())
            .unwrap();
        current["id"].as_str().unwrap().to_owned()
    };
    let (company, person) = (held("1", "company"), held("1", "person"));

    // The name does not say which, so neither is taken for it.
    let message = server
        .get("/v1/explain?a=crm:1&b=crm:2")
        .error(400, "invalid");
    assert!(
        message.starts_with("2 records are named `crm:1`: name one by its ID, of "),
        "{message}"
    );
    assert!(
        message.contains(&format!("{company} (a `company`)")),
        "{message}"
    );
    assert!(
        message.contains(&format!("{person} (a `person`)")),
        "{message}"
    );
    // Whichever side it is on.
    server
        .get("/v1/explain?a=crm:2&b=crm:1")
        .error(400, "invalid");
    // By its ID the company is explained, and a name one record has still
    // names it.
    let by_id = server.read(&format!("/v1/explain?a={company}&b=crm:2"));
    let ids = [
        &by_id["left"]["record"]["id"],
        &by_id["right"]["record"]["id"],
    ];
    assert!(ids.contains(&&json!(company)), "{by_id}");

    // A new version of the company's row leaves two that nothing replaced,
    // the new one and the person.
    load("company", json!([{ "id": "1", "name": "Dana Ltd" }]));
    let message = server
        .get("/v1/explain?a=crm:1&b=crm:2")
        .error(400, "invalid");
    assert!(message.starts_with("2 records are named"), "{message}");
    assert!(!message.contains(&company), "{message}");
    assert!(message.contains(&held("1", "company")), "{message}");
}

#[test]
fn a_load_replaces_a_row_unless_it_is_told_not_to() {
    let dir = tempfile::tempdir().unwrap();
    let server = things_server(dir.path());
    let load = |rows: Value, replace: Option<bool>| {
        let mut body = json!({ "source": "shop", "entity_type": "thing", "rows": rows });
        if let Some(replace) = replace {
            body["replace"] = json!(replace);
        }
        server.post("/v1/load", body)
    };
    let csv = |query: &str, body: &str| {
        server
            .request(
                "POST",
                &format!("/v1/load?source=shop&entity_type=thing{query}"),
            )
            .header("Content-Type", "text/csv")
            .body(body.as_bytes())
            .send()
    };
    let first = json!([{ "id": "1", "code": "anvil" }, { "id": "2", "code": "saw" }]);
    load(first.clone(), Some(false)).ok();

    // Told not to replace, a load of a row the store holds with other
    // values is refused whole, in JSON and in CSV alike.
    let changed = json!([{ "id": "3", "code": "file" }, { "id": "2", "code": "saw, hand" }]);
    let refusal = "load rejected: 1 row has a key the store holds with other values: `2`. \
                   Nothing was loaded, because this load was not to replace rows. If the row is \
                   meant to change, load again with `replace` set to true";
    assert_eq!(
        load(changed.clone(), Some(false)).error(400, "invalid"),
        refusal
    );
    let lines = "id,code\n3,file\n2,\"saw, hand\"\n";
    assert_eq!(csv("&replace=false", lines).error(400, "invalid"), refusal);
    assert_eq!(held(&server, "shop").len(), 2);
    // The rows the store holds as they are add nothing and are not refused.
    let same = load(first, Some(false)).ok();
    assert_eq!(same["run"]["load"]["duplicates"], 2, "{same}");

    // A load that does not say replaces the row, as a load that says it may.
    let replaced = load(changed, None).ok();
    assert_eq!(replaced["run"]["load"]["superseded"], 1, "{replaced}");
    let again = csv("&replace=true", "id,code\n2,\"saw, bow\"\n").ok();
    assert_eq!(again["run"]["load"]["superseded"], 1, "{again}");
    let again = csv("", "id,code\n2,\"saw, pit\"\n").ok();
    assert_eq!(again["run"]["load"]["superseded"], 1, "{again}");
    csv("&replace=perhaps", "id,code\n2,saw\n").error(400, "invalid");
}

#[test]
fn a_load_and_a_match_are_called_by_the_words_the_commands_have() {
    let dir = tempfile::tempdir().unwrap();
    let server = things_server(dir.path());
    let rows = json!({
        "source": "shop",
        "entity_type": "thing",
        "rows": [{ "id": "1", "code": "anvil" }],
    });

    // The paths are the words of `spool load` and `spool match`, and what
    // comes back is in those words too.
    let loaded = server.call("/v1/load", rows.clone());
    assert_eq!(loaded["run"]["kind"], "load");
    assert_eq!(loaded["run"]["load"]["inserted"], 1);
    let matched = server.call("/v1/match", json!({ "entity_type": "thing" }));
    assert_eq!(matched["run"]["kind"], "match");
    let record = server.read("/v1/records")["records"][0].clone();
    assert_eq!(record["load_run"], loaded["run"]["run"]);
    assert!(record["loaded_at"].is_string(), "{record}");
    assert_eq!(
        server.read("/v1/runs?kind=load")["runs"][0]["run"],
        loaded["run"]["run"]
    );

    // Runs are read where they were, and nothing is started there.
    server
        .post("/v1/runs", json!({ "entity_type": "thing" }))
        .error(405, "invalid");
    server.get("/v1/match").error(405, "invalid");
    // The words the calls had before are no longer answered.
    server.post("/v1/ingest", rows).error(404, "not_found");
    server.get("/v1/runs?kind=ingest").error(400, "invalid");
}
