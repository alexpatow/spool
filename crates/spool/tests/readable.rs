//! What a person reads of the store: what things are called, where their
//! records came from, what a search finds, and what a review item is about.

mod common;

use common::*;
use serde_json::json;
use spool::core::{Actor, Changeset, IdGenerator, Op, SeededIds, Verdict};
use spool::{
    Engine, Error, IdentityFilter, IngestOptions, ObservationFilter, ResolveOptions, ReviewFilter,
    Rows, label_of,
};

fn resolve(engine: &Engine) {
    engine.resolve(&ResolveOptions::default()).unwrap();
}

fn json_rows(rows: serde_json::Value) -> Rows {
    let rows: Vec<serde_json::Map<String, serde_json::Value>> =
        serde_json::from_value(rows).unwrap();
    Rows::from_json(&rows).unwrap()
}

fn search(engine: &Engine, q: &str) -> Vec<String> {
    let mut found: Vec<String> = engine
        .observations(&ObservationFilter {
            q: Some(q.to_owned()),
            ..ObservationFilter::default()
        })
        .unwrap()
        .iter()
        .map(|record| record.name())
        .collect();
    found.sort();
    found
}

#[test]
fn a_payload_is_called_by_the_field_that_names_it() {
    for (payload, label) in [
        (
            json!({ "id": "1", "name": "Acorn Tools" }),
            Some("Acorn Tools"),
        ),
        // `name` before the other names a payload may have.
        (
            json!({ "legal_name": "ACORN LTD", "name": "Acorn", "trading_name": "Oak" }),
            Some("Acorn"),
        ),
        (
            json!({ "number": "X1", "trading_name": "Oak", "legal_name": "ACORN LTD" }),
            Some("ACORN LTD"),
        ),
        (json!({ "title": "  A report  " }), Some("A report")),
        // Failing those, a field whose name ends in `name`, the first in
        // alphabetical order that holds text.
        (
            json!({ "surname": "Lovelace", "FirstName": "Ada", "age": 36 }),
            Some("Ada"),
        ),
        (
            json!({ "company_name": "", "brand_name": null, "store_name": "Oak" }),
            Some("Oak"),
        ),
        // Only text names anything.
        (
            json!({ "name": 7, "title": ["a"], "label": { "en": "x" } }),
            None,
        ),
        (json!({ "name": "   " }), None),
        (json!({ "code": "K7", "city": "Leeds" }), None),
        (json!({}), None),
        (json!("Acorn"), None),
    ] {
        assert_eq!(label_of(&payload).as_deref(), label, "{payload}");
    }
}

#[test]
fn an_identity_is_called_what_most_of_its_members_are_and_says_where_they_came_from() {
    let engine = engine();
    ingest(&engine, "shop", &[("1", Some("anvil"), Some("K7"))]);
    engine
        .ingest(
            "depot",
            "thing",
            json_rows(json!([
                { "id": "8", "title": "Anvil, forged", "code": "K7" },
                { "id": "9", "title": "Anvil, forged", "code": "K7" },
                { "id": "10", "code": "Z1" },
            ])),
            &IngestOptions::keyed_by("id"),
        )
        .unwrap();
    resolve(&engine);
    let anvil = identity(&engine, "shop", "1").unwrap();
    let nameless = identity(&engine, "depot", "10").unwrap();

    let record = engine
        .observation(observation(&engine, "depot", "8"))
        .unwrap()
        .unwrap();
    assert_eq!(record.label.as_deref(), Some("Anvil, forged"));

    // Two of the three members say `Anvil, forged`.
    let listed = engine.identities(&IdentityFilter::default()).unwrap();
    let found = |id| listed.iter().find(|identity| identity.id == id).unwrap();
    assert_eq!(found(anvil).label.as_deref(), Some("Anvil, forged"));
    assert_eq!(found(anvil).members, 3);
    let sources: Vec<(&str, u64)> = found(anvil)
        .sources
        .iter()
        .map(|source| (source.source.as_str(), source.observations))
        .collect();
    assert_eq!(sources, [("depot", 2), ("shop", 1)]);
    assert_eq!(found(nameless).label, None);
    assert_eq!(found(nameless).sources.len(), 1);
    assert_eq!(
        engine.identity(anvil).unwrap().label.as_deref(),
        Some("Anvil, forged")
    );
    assert_eq!(engine.identity(nameless).unwrap().label, None);

    // The members of one identity are read as observations, with what each
    // says and when it arrived.
    let members = engine
        .observations(&ObservationFilter {
            identity: Some(anvil),
            ..ObservationFilter::default()
        })
        .unwrap();
    let mut names: Vec<String> = members.iter().map(|member| member.name()).collect();
    names.sort();
    assert_eq!(names, ["depot:8", "depot:9", "shop:1"]);
    assert!(members.iter().all(|member| member.identity == Some(anvil)));

    // The store says what each source gave.
    let info = engine.info().unwrap();
    let sources: Vec<(&str, u64, u64)> = info.entity_types[0]
        .sources
        .iter()
        .map(|source| {
            (
                source.source.as_str(),
                source.observations,
                source.unassigned,
            )
        })
        .collect();
    assert_eq!(sources, [("depot", 3, 0), ("shop", 1, 0)]);
    assert_eq!(
        info.entity_types[0].sources[1].last_ingested_at,
        engine
            .observation(observation(&engine, "shop", "1"))
            .unwrap()
            .unwrap()
            .ingested_at
    );
}

#[test]
fn of_labels_that_as_many_members_have_the_first_to_arrive_stands() {
    let engine = engine();
    engine
        .ingest(
            "registry",
            "thing",
            json_rows(json!([{ "id": "1", "legal_name": "ANVIL WORKS LTD", "code": "K7" }])),
            &IngestOptions::keyed_by("id"),
        )
        .unwrap();
    ingest(&engine, "shop", &[("2", Some("Anvil Works"), Some("K7"))]);
    resolve(&engine);
    let works = identity(&engine, "shop", "2").unwrap();
    assert_eq!(works, identity(&engine, "registry", "1").unwrap());
    assert_eq!(
        engine.identity(works).unwrap().label.as_deref(),
        Some("ANVIL WORKS LTD")
    );

    // A second record that says the same as the later one outnumbers it.
    ingest(&engine, "depot", &[("3", Some("Anvil Works"), Some("K7"))]);
    resolve(&engine);
    assert_eq!(
        engine.identity(works).unwrap().label.as_deref(),
        Some("Anvil Works")
    );

    // An identity that was merged away is called what the one that stands
    // for it is called.
    ingest(&engine, "shop", &[("4", Some("Hammer"), Some("H1"))]);
    resolve(&engine);
    let hammer = identity(&engine, "shop", "4").unwrap();
    engine
        .commit(Changeset {
            id: SeededIds::new(9).changeset_id(),
            base: engine.head().unwrap(),
            actor: Actor::User("reviewer".to_owned()),
            reason: None,
            run: None,
            ops: vec![Op::Merge {
                from: hammer,
                into: works,
            }],
        })
        .unwrap();
    let merged = engine.identity(hammer).unwrap();
    assert_eq!(merged.current, works);
    assert_eq!(merged.label.as_deref(), Some("Anvil Works"));
    // It has no members of its own any more.
    assert!(
        engine
            .observations(&ObservationFilter {
                identity: Some(hammer),
                ..ObservationFilter::default()
            })
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_search_finds_the_records_that_hold_every_word() {
    let engine = engine();
    engine
        .ingest(
            "crm",
            "thing",
            json_rows(json!([
                { "id": "c-1", "name": "Müller & Söhne GmbH", "city": "Köln", "code": "M1" },
                { "id": "c-2", "name": "Acorn Tools", "city": "Sheffield", "code": "A1", "staff": 40 },
                { "id": "c-3", "name": "Acorn Foods", "city": "Leeds", "code": "A2" },
            ])),
            &IngestOptions::keyed_by("id"),
        )
        .unwrap();
    engine
        .ingest(
            "registry",
            "thing",
            json_rows(json!([
                { "id": "X9", "legal_name": "ACORN TOOL COMPANY LTD", "code": "A1" },
                { "id": "X10", "legal_name": "OAK 100% LTD", "code": "O_1" },
            ])),
            &IngestOptions::keyed_by("id"),
        )
        .unwrap();
    resolve(&engine);

    // Whatever the case and the accents, in any field, and in any order.
    assert_eq!(
        search(&engine, "acorn"),
        ["crm:c-2", "crm:c-3", "registry:X9"]
    );
    assert_eq!(search(&engine, "  TOOLS   acorn "), ["crm:c-2"]);
    assert_eq!(search(&engine, "muller koln"), ["crm:c-1"]);
    assert_eq!(search(&engine, "SÖHNE"), ["crm:c-1"]);
    assert_eq!(search(&engine, "acorn leeds"), ["crm:c-3"]);
    // A number, the key, and the name of the source.
    assert_eq!(search(&engine, "40"), ["crm:c-2"]);
    assert_eq!(search(&engine, "x9"), ["registry:X9"]);
    assert_eq!(search(&engine, "registry oak"), ["registry:X10"]);
    // The name of a field is not what a record says.
    assert_eq!(search(&engine, "legal_name"), [] as [&str; 0]);
    assert_eq!(search(&engine, "acorn hammer"), [] as [&str; 0]);
    // What a pattern would read as a wildcard is a character like any other.
    assert_eq!(search(&engine, "100%"), ["registry:X10"]);
    assert_eq!(search(&engine, "o_1"), ["registry:X10"]);
    assert_eq!(search(&engine, "a%"), [] as [&str; 0]);
    assert_eq!(search(&engine, "' OR 1=1 --"), [] as [&str; 0]);
    // No word finds everything.
    assert_eq!(search(&engine, "   ").len(), 5);

    // With the other filters, and in pages.
    let narrowed = engine
        .observations(&ObservationFilter {
            q: Some("acorn".to_owned()),
            source: Some("crm".to_owned()),
            limit: 1,
            ..ObservationFilter::default()
        })
        .unwrap();
    assert_eq!(narrowed.len(), 1);
    let rest = engine
        .observations(&ObservationFilter {
            q: Some("acorn".to_owned()),
            source: Some("crm".to_owned()),
            after: Some(narrowed[0].id),
            ..ObservationFilter::default()
        })
        .unwrap();
    assert_eq!(rest.len(), 1);
    assert_ne!(rest[0].id, narrowed[0].id);

    // An identity is found by what any one of its members holds.
    let tools = identity(&engine, "crm", "c-2").unwrap();
    assert_eq!(tools, identity(&engine, "registry", "X9").unwrap());
    let found = |q: &str| -> Vec<_> {
        engine
            .identities(&IdentityFilter {
                q: Some(q.to_owned()),
                ..IdentityFilter::default()
            })
            .unwrap()
            .into_iter()
            .map(|identity| identity.id)
            .collect()
    };
    assert_eq!(found("acorn tool company"), [tools]);
    assert_eq!(found("sheffield"), [tools]);
    let mut acorns = vec![tools, identity(&engine, "crm", "c-3").unwrap()];
    acorns.sort();
    assert_eq!(found("ACORN"), acorns);
    // The words are held by one member: no record says both.
    assert_eq!(found("sheffield company"), []);
    assert_eq!(found("hammer"), []);
    assert_eq!(found("").len(), 4);
}

#[test]
fn a_review_case_shows_the_records_the_identities_they_matched_and_what_was_judged() {
    let engine = engine();
    ingest(
        &engine,
        "shop",
        &[
            ("1", Some("anvil"), None),
            ("2", Some("anvil"), None),
            ("3", None, Some("H1")),
        ],
    );
    resolve(&engine);
    ingest(&engine, "depot", &[("9", Some("anvil"), Some("H1"))]);
    resolve(&engine);
    let [item] = &engine.review_items(&ReviewFilter::open()).unwrap()[..] else {
        panic!("one open review item");
    };

    let case = engine.review_case(item.id).unwrap();
    assert_eq!(&case.item, item);
    let waiting = observation(&engine, "depot", "9");
    assert_eq!(case.observations.len(), 1);
    assert_eq!(case.observations[0].id, waiting);
    assert_eq!(case.observations[0].label.as_deref(), Some("anvil"));
    assert_eq!(case.observations[0].identity, None);

    let anvil = identity(&engine, "shop", "1").unwrap();
    let hammer = identity(&engine, "shop", "3").unwrap();
    let candidate = |id| {
        case.candidates
            .iter()
            .find(|candidate| candidate.identity.id == id)
            .unwrap()
    };
    assert_eq!(case.candidates.len(), 2);
    assert!(case.candidates[0].identity.id < case.candidates[1].identity.id);
    assert_eq!(candidate(anvil).identity.label.as_deref(), Some("anvil"));
    assert_eq!(candidate(anvil).identity.members, 2);
    let names = |id| -> Vec<String> {
        let mut names: Vec<String> = candidate(id)
            .members
            .iter()
            .map(|member| member.name())
            .collect();
        names.sort();
        names
    };
    assert_eq!(names(anvil), ["shop:1", "shop:2"]);
    assert_eq!(names(hammer), ["shop:3"]);
    assert_eq!(candidate(hammer).identity.label, None);
    assert_eq!(
        candidate(hammer).members[0].payload,
        json!({ "id": "3", "code": "H1" })
    );

    // It matched each of the three, and the case says which run judged so.
    let mut judged: Vec<(String, Verdict)> = case
        .judgements
        .iter()
        .map(|judgement| {
            assert_eq!(judgement.observation, waiting);
            let other = engine.observation(judgement.other).unwrap().unwrap();
            (other.name(), judgement.verdict)
        })
        .collect();
    judged.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        judged,
        [
            ("shop:1".to_owned(), Verdict::Match),
            ("shop:2".to_owned(), Verdict::Match),
            ("shop:3".to_owned(), Verdict::Match),
        ]
    );
    let run = case.judgements[0].run;
    assert!(case.judgements.iter().all(|judgement| judgement.run == run));
    assert_eq!(engine.run(run).unwrap().kind, spool::RunKind::Match);
    assert_eq!(
        case.judgements
            .iter()
            .find(|judgement| judgement.other == observation(&engine, "shop", "3"))
            .unwrap()
            .detail,
        Some(json!({ "block": ["same_code"], "match": ["same_code"], "no_match": [] }))
    );

    // Once the two identities are one, the case shows the one that stands.
    engine
        .commit(Changeset {
            id: SeededIds::new(9).changeset_id(),
            base: engine.head().unwrap(),
            actor: Actor::User("reviewer".to_owned()),
            reason: None,
            run: None,
            ops: vec![Op::Merge {
                from: hammer,
                into: anvil,
            }],
        })
        .unwrap();
    let case = engine.review_case(item.id).unwrap();
    assert_eq!(case.item.identities.len(), 2);
    let [merged] = &case.candidates[..] else {
        panic!("one identity stands for the two");
    };
    assert_eq!(merged.identity.id, anvil);
    assert_eq!(merged.identity.members, 3);
    assert_eq!(merged.members.len(), 3);
    assert_eq!(case.judgements.len(), 3);

    let error = engine.review_case(99).unwrap_err();
    assert!(matches!(error, Error::UnknownReviewItem(99)), "{error}");
}

#[test]
fn a_case_names_the_members_that_were_judged_before_the_others() {
    let engine = engine();
    // An identity of many records that share a name, one of which has the
    // code the waiting record has.
    let many: Vec<(String, Option<&str>, Option<&str>)> = (0..40)
        .map(|n| (format!("a{n:02}"), Some("anvil"), None))
        .collect();
    let many: Vec<(&str, Option<&str>, Option<&str>)> = many
        .iter()
        .map(|(id, name, code)| (id.as_str(), *name, *code))
        .collect();
    ingest(&engine, "shop", &many);
    ingest(&engine, "shop", &[("h", Some("hammer"), Some("H1"))]);
    resolve(&engine);
    ingest(&engine, "late", &[("z", Some("anvil"), None)]);
    resolve(&engine);
    ingest(&engine, "depot", &[("9", Some("hammer"), Some("H2"))]);
    resolve(&engine);
    assert!(
        engine
            .review_items(&ReviewFilter::open())
            .unwrap()
            .is_empty()
    );

    // A record that is an anvil by name and a hammer by code.
    ingest(&engine, "depot", &[("10", Some("anvil"), Some("H1"))]);
    resolve(&engine);
    let [item] = &engine.review_items(&ReviewFilter::open()).unwrap()[..] else {
        panic!("one open review item");
    };
    let case = engine.review_case(item.id).unwrap();
    let anvil = identity(&engine, "shop", "a00").unwrap();
    let anvils = case
        .candidates
        .iter()
        .find(|candidate| candidate.identity.id == anvil)
        .unwrap();
    assert_eq!(anvils.identity.members, 41);
    assert_eq!(anvils.members.len() as u64, spool::CASE_MEMBERS);
    // Every one shown was judged against the record, though more were.
    let judged: Vec<_> = case
        .judgements
        .iter()
        .map(|judgement| judgement.other)
        .collect();
    assert!(
        anvils
            .members
            .iter()
            .all(|member| judged.contains(&member.id))
    );
    assert_eq!(case.judgements.len(), 42);
}
