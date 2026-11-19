//! Lists sorted by a column: the whole list is in that order, and its pages,
//! each continuing after the last entry of the page before, walk it with
//! no entry twice or missing.

mod common;

use std::collections::BTreeMap;
use std::fmt::Debug;

use common::*;
use spool::core::{Actor, Changeset, ConstraintKind, IdentityId, Op, Seq};
use spool::{
    Direction, DocumentFilter, DocumentSort, Engine, IdentityFilter, IdentitySort, LogSort,
    NewDocument, Options, RecordFilter, RecordSort, ResolveOptions, RunFilter, RunSort, Sort,
};

const DIRECTIONS: [Direction; 2] = [Direction::Ascending, Direction::Descending];

/// The tests' things, and a source whose name begins with a capital.
fn engine() -> Engine {
    let mut sources = SOURCES.to_vec();
    sources.push("Zinc");
    let mut thing = thing();
    thing.entity = entity(
        Some("name"),
        &[("name", "name"), ("code", "code")],
        &sources,
    );
    let mut project = project_of(vec![thing]);
    project.sources.push(source("Zinc"));
    let engine = Engine::open_in_memory(Options::seeded(1)).unwrap();
    engine.apply(&project.compile().unwrap()).unwrap();
    engine
}

/// Things whose names differ in case, things with no name, identities of
/// one record and of three, sources and actors whose names differ in case,
/// documents of several lengths and a document with two versions, and
/// changesets with reasons and without.
fn populated() -> Engine {
    let engine = engine();
    load(
        &engine,
        "shop",
        &[
            ("1", Some("anvil"), None),
            ("2", Some("Bolt"), None),
            ("3", None, Some("H1")),
            ("4", Some("crate"), None),
        ],
    );
    load(
        &engine,
        "depot",
        &[
            ("7", Some("anvil"), None),
            ("8", Some("bolt"), None),
            ("9", Some("Drill"), None),
        ],
    );
    load(
        &engine,
        "kiosk",
        &[("5", Some("anvil"), None), ("6", None, Some("K9"))],
    );
    engine.resolve(&ResolveOptions::default()).unwrap();
    load(&engine, "yard", &[("10", Some("easel"), None)]);
    load(&engine, "Zinc", &[("11", Some("Fork"), None)]);

    let document = |key: &str, text: &str| NewDocument {
        key: key.to_owned(),
        uri: None,
        text: text.to_owned(),
    };
    engine
        .add_documents(
            "shop",
            &[
                document("b.txt", "a longer text than the others have"),
                document("A.txt", "short"),
                document("c.txt", "middling text"),
            ],
        )
        .unwrap();
    engine
        .add_documents("depot", &[document("a.txt", "the depot's")])
        .unwrap();
    engine
        .add_documents("Zinc", &[document("d.txt", "the zinc's")])
        .unwrap();
    engine
        .add_documents("shop", &[document("A.txt", "short, and then some")])
        .unwrap();

    let anvil = record_id(&engine, "shop", "1");
    let others = [
        record_id(&engine, "shop", "2"),
        record_id(&engine, "shop", "4"),
        record_id(&engine, "depot", "9"),
        record_id(&engine, "kiosk", "6"),
    ];
    for (index, (reason, actor)) in [
        (Some("Zeta"), "zed"),
        (None, "ada"),
        (Some("alpha"), "Bea"),
        (Some("Beta"), "ada"),
    ]
    .into_iter()
    .enumerate()
    {
        let b = others[index];
        let mut ops = vec![Op::Constrain {
            kind: ConstraintKind::CannotLink,
            a: anvil,
            b,
        }];
        if index == 3 {
            ops.push(Op::Unconstrain {
                kind: ConstraintKind::CannotLink,
                a: anvil,
                b: others[0],
            });
        }
        engine
            .commit(Changeset {
                id: engine.mint_ids(0, 1).changesets[0],
                base: engine.head().unwrap(),
                actor: Actor::User(actor.to_owned()),
                reason: reason.map(str::to_owned),
                run: None,
                ops,
            })
            .unwrap();
    }
    engine
}

/// Whether the values run the given way, with those that are missing last.
fn in_order<T: Ord + Debug>(values: &[Option<T>], direction: Direction, what: &str) {
    let present: Vec<&T> = values.iter().flatten().collect();
    let first_missing = values
        .iter()
        .position(Option::is_none)
        .unwrap_or(values.len());
    assert!(
        values[first_missing..].iter().all(Option::is_none),
        "{what}: what is missing comes last: {values:?}"
    );
    let ordered = present.windows(2).all(|pair| match direction {
        Direction::Ascending => pair[0] <= pair[1],
        Direction::Descending => pair[0] >= pair[1],
    });
    assert!(ordered, "{what} {direction:?}: {values:?}");
}

/// Walks the list in pages of every size up to its length and checks that
/// they make the whole of it.
fn walks<T, Id>(
    all: &[T],
    id: impl Fn(&T) -> Id,
    page: impl Fn(Option<Id>, u64) -> Vec<T>,
    what: &str,
) where
    T: PartialEq + Debug,
{
    for limit in 1..=all.len() as u64 {
        let mut walked = Vec::new();
        let mut after = None;
        loop {
            let read = page(after.take(), limit);
            let Some(last) = read.last() else { break };
            after = Some(id(last));
            let full = read.len() as u64 == limit;
            walked.extend(read);
            if !full {
                break;
            }
        }
        assert_eq!(walked, all, "{what} in pages of {limit}");
    }
}

#[test]
fn documents_are_sorted_by_each_column_in_pages() {
    let engine = populated();
    let unsorted = engine.documents(&DocumentFilter::default()).unwrap();
    assert_eq!(unsorted.len(), 6);
    for by in [
        DocumentSort::Id,
        DocumentSort::Key,
        DocumentSort::Source,
        DocumentSort::Length,
        DocumentSort::Loaded,
    ] {
        for direction in DIRECTIONS {
            let sort = Some(Sort { by, direction });
            let list = |after, limit| {
                engine
                    .documents(&DocumentFilter {
                        after,
                        limit,
                        sort,
                        ..DocumentFilter::default()
                    })
                    .unwrap()
            };
            let all = list(None, 1000);
            let what = format!("documents by {by:?}");
            let mut ids: Vec<_> = all.iter().map(|document| document.id).collect();
            ids.sort();
            assert_eq!(
                ids,
                unsorted
                    .iter()
                    .map(|document| document.id)
                    .collect::<Vec<_>>(),
                "{what}: every document"
            );
            let values: Vec<Option<(String, String, String, u64)>> = all
                .iter()
                .map(|document| {
                    let (key, exact) = (document.key.to_lowercase(), document.key.clone());
                    let none = String::new;
                    Some(match by {
                        DocumentSort::Id => (document.id.to_string(), none(), none(), 0),
                        DocumentSort::Key => (key, exact, document.source.to_lowercase(), 0),
                        DocumentSort::Source => (document.source.to_lowercase(), key, exact, 0),
                        DocumentSort::Length => (none(), none(), none(), document.length),
                        DocumentSort::Loaded => (
                            document.loaded_at.clone(),
                            none(),
                            none(),
                            document.load_run.0,
                        ),
                    })
                })
                .collect();
            in_order(&values, direction, &what);
            walks(&all, |document| document.id, list, &what);
        }
    }

    // Whatever the case of a key, and versions of one key the oldest first.
    let by_key = engine
        .documents(&DocumentFilter {
            sort: Some(Sort::ascending(DocumentSort::Key)),
            ..DocumentFilter::default()
        })
        .unwrap();
    let names: Vec<(&str, &str)> = by_key
        .iter()
        .map(|document| (document.source.as_str(), document.key.as_str()))
        .collect();
    assert_eq!(
        names,
        [
            ("shop", "A.txt"),
            ("shop", "A.txt"),
            ("depot", "a.txt"),
            ("shop", "b.txt"),
            ("shop", "c.txt"),
            ("Zinc", "d.txt"),
        ]
    );
    assert_eq!(by_key[1].supersedes, Some(by_key[0].id));
    // A source whose name begins with a capital is where its letters put it.
    let by_source = engine
        .documents(&DocumentFilter {
            sort: Some(Sort::ascending(DocumentSort::Source)),
            ..DocumentFilter::default()
        })
        .unwrap();
    let sources: Vec<&str> = by_source
        .iter()
        .map(|document| document.source.as_str())
        .collect();
    assert_eq!(sources, ["depot", "shop", "shop", "shop", "shop", "Zinc"]);
}

/// What each identity is called, as a list of identities says.
fn labels(engine: &Engine) -> BTreeMap<IdentityId, Option<String>> {
    engine
        .identities(&IdentityFilter {
            limit: 1000,
            ..IdentityFilter::default()
        })
        .unwrap()
        .into_iter()
        .map(|identity| (identity.id, identity.label))
        .collect()
}

#[test]
fn records_are_sorted_by_each_column_in_pages() {
    let engine = populated();
    let labels = labels(&engine);
    let unsorted = engine.records(&RecordFilter::default()).unwrap();
    assert_eq!(unsorted.len(), 11);
    for by in [
        RecordSort::Id,
        RecordSort::Source,
        RecordSort::Key,
        RecordSort::Label,
        RecordSort::EntityType,
        RecordSort::Identity,
    ] {
        for direction in DIRECTIONS {
            let sort = Some(Sort { by, direction });
            let list = |after, limit| {
                engine
                    .records(&RecordFilter {
                        after,
                        limit,
                        sort,
                        ..RecordFilter::default()
                    })
                    .unwrap()
            };
            let all = list(None, 1000);
            let what = format!("records by {by:?}");
            assert_eq!(all.len(), unsorted.len(), "{what}: every record");
            let values: Vec<Option<String>> = all
                .iter()
                .map(|record| match by {
                    RecordSort::Id => Some(record.id.to_string()),
                    RecordSort::Source => Some(record.source.to_lowercase()),
                    RecordSort::Key => record.key.as_deref().map(str::to_lowercase),
                    RecordSort::Label => record.label.as_deref().map(str::to_lowercase),
                    RecordSort::EntityType => Some(record.entity_type.to_string()),
                    RecordSort::Identity => record
                        .identity
                        .and_then(|identity| labels[&identity].as_deref().map(str::to_lowercase)),
                })
                .collect();
            in_order(&values, direction, &what);
            walks(&all, |record| record.id, list, &what);
        }
    }
    // Sorted by source whatever its case, and then by key.
    let by_source = engine
        .records(&RecordFilter {
            sort: Some(Sort::ascending(RecordSort::Source)),
            ..RecordFilter::default()
        })
        .unwrap();
    assert_eq!(
        by_source
            .iter()
            .map(|record| record.source.as_str())
            .collect::<Vec<_>>(),
        [
            "depot", "depot", "depot", "kiosk", "kiosk", "shop", "shop", "shop", "shop", "yard",
            "Zinc"
        ]
    );
}

#[test]
fn identities_are_sorted_by_each_column_in_pages() {
    let engine = populated();
    let unsorted = engine.identities(&IdentityFilter::default()).unwrap();
    assert_eq!(unsorted.len(), 7);
    assert!(unsorted.iter().any(|identity| identity.label.is_none()));
    for by in [
        IdentitySort::Id,
        IdentitySort::Label,
        IdentitySort::EntityType,
        IdentitySort::Records,
        IdentitySort::Sources,
        IdentitySort::Created,
    ] {
        for direction in DIRECTIONS {
            let sort = Some(Sort { by, direction });
            let list = |after, limit| {
                engine
                    .identities(&IdentityFilter {
                        after,
                        limit,
                        sort,
                        ..IdentityFilter::default()
                    })
                    .unwrap()
            };
            let all = list(None, 1000);
            let what = format!("identities by {by:?}");
            assert_eq!(all.len(), unsorted.len(), "{what}: every identity");
            // The label the list sorts by is the one it shows.
            let values: Vec<Option<(String, u64)>> = all
                .iter()
                .map(|identity| match by {
                    IdentitySort::Id => Some((identity.id.to_string(), 0)),
                    IdentitySort::Label => identity
                        .label
                        .as_deref()
                        .map(|label| (label.to_lowercase(), 0)),
                    IdentitySort::EntityType => Some((identity.entity_type.to_string(), 0)),
                    IdentitySort::Records => Some((String::new(), identity.members)),
                    IdentitySort::Sources => Some((String::new(), identity.sources.len() as u64)),
                    IdentitySort::Created => Some((String::new(), identity.created.0)),
                })
                .collect();
            in_order(&values, direction, &what);
            walks(&all, |identity| identity.id, list, &what);
        }
    }
    let biggest = engine
        .identities(&IdentityFilter {
            sort: Some(Sort::descending(IdentitySort::Records)),
            limit: 1,
            ..IdentityFilter::default()
        })
        .unwrap();
    assert_eq!(biggest[0].label.as_deref(), Some("anvil"));
    assert_eq!(biggest[0].members, 3);
}

#[test]
fn runs_are_sorted_over_all_of_them_before_the_limit() {
    let engine = populated();
    let unsorted = engine
        .runs(&RunFilter {
            limit: 1000,
            ..RunFilter::default()
        })
        .unwrap();
    assert!(unsorted.len() >= 6, "{unsorted:?}");
    for by in [
        RunSort::Run,
        RunSort::Kind,
        RunSort::Status,
        RunSort::Details,
        RunSort::StartedBy,
        RunSort::Started,
    ] {
        for direction in DIRECTIONS {
            let sort = Some(Sort { by, direction });
            let list = |limit| {
                engine
                    .runs(&RunFilter {
                        limit,
                        sort,
                        ..RunFilter::default()
                    })
                    .unwrap()
            };
            let all = list(1000);
            let what = format!("runs by {by:?}");
            assert_eq!(all.len(), unsorted.len(), "{what}: every run");
            let values: Vec<Option<(String, u64)>> = all
                .iter()
                .map(|run| match by {
                    RunSort::Run => Some((String::new(), run.run.0)),
                    RunSort::Kind => Some((run.kind.as_str().to_owned(), run.run.0)),
                    RunSort::Status => Some((run.status.as_str().to_owned(), run.run.0)),
                    RunSort::Details => {
                        let details: Vec<&str> = [
                            run.entity_type.as_ref().map(|entity| entity.as_str()),
                            run.strategy.as_ref().map(|strategy| strategy.name.as_str()),
                            run.extractor
                                .as_ref()
                                .map(|extractor| extractor.name.as_str()),
                        ]
                        .into_iter()
                        .flatten()
                        .collect();
                        (!details.is_empty()).then(|| (details.join(", ").to_lowercase(), 0))
                    }
                    RunSort::StartedBy => Some((run.started_by.to_string().to_lowercase(), 0)),
                    RunSort::Started => Some((run.started_at.clone(), 0)),
                })
                .collect();
            in_order(&values, direction, &what);
            for limit in 1..=all.len() as u64 {
                assert_eq!(
                    list(limit),
                    all[..limit as usize],
                    "{what}, the first {limit}"
                );
            }
        }
    }
}

#[test]
fn the_log_is_sorted_by_each_column_in_pages() {
    let engine = populated();
    let log = engine.logged_after(Seq::ZERO, 1000).unwrap();
    assert_eq!(log.len(), 5);
    for by in [
        LogSort::Seq,
        LogSort::Actor,
        LogSort::Operations,
        LogSort::Run,
        LogSort::Reason,
    ] {
        for direction in DIRECTIONS {
            let sort = Sort { by, direction };
            let list = |after, limit| engine.logged_sorted(sort, after, limit, &[]).unwrap();
            let all = list(None, 1000);
            let what = format!("the log by {by:?}");
            let mut seqs: Vec<Seq> = all.iter().map(|entry| entry.entry.seq).collect();
            seqs.sort();
            assert_eq!(
                seqs,
                log.iter().map(|entry| entry.entry.seq).collect::<Vec<_>>(),
                "{what}: every changeset"
            );
            // Each changeset is read whole, as the log holds it.
            for entry in &all {
                assert!(log.contains(entry), "{what}: {entry:?}");
            }
            let values: Vec<Option<(String, u64)>> = all
                .iter()
                .map(|logged| {
                    let changeset = &logged.entry.changeset;
                    match by {
                        LogSort::Seq => Some((String::new(), logged.entry.seq.0)),
                        LogSort::Actor => Some((changeset.actor.to_string().to_lowercase(), 0)),
                        LogSort::Operations => Some((String::new(), changeset.ops.len() as u64)),
                        LogSort::Run => changeset.run.map(|run| (String::new(), run.0)),
                        LogSort::Reason => changeset
                            .reason
                            .as_deref()
                            .map(|reason| (reason.to_lowercase(), 0)),
                    }
                })
                .collect();
            in_order(&values, direction, &what);
            walks(&all, |logged| logged.entry.seq, list, &what);
        }
    }
    let newest = engine
        .logged_sorted(Sort::descending(LogSort::Seq), None, 2, &[])
        .unwrap();
    assert_eq!(
        newest
            .iter()
            .map(|entry| entry.entry.seq.0)
            .collect::<Vec<_>>(),
        [5, 4]
    );
}

#[test]
fn who_did_something_is_sorted_by_what_they_are_called() {
    let engine = engine();
    load(
        &engine,
        "shop",
        &[("1", Some("anvil"), None), ("2", Some("bolt"), None)],
    );
    engine.resolve(&ResolveOptions::default()).unwrap();
    let (zed, ada) = (Actor::User("zz".to_owned()), Actor::User("aa".to_owned()));
    let document = |key: &str| NewDocument {
        key: key.to_owned(),
        uri: None,
        text: format!("the text of {key}"),
    };
    engine
        .acting(zed.clone())
        .add_documents("shop", &[document("a.txt")])
        .unwrap();
    engine
        .acting(ada.clone())
        .add_documents("shop", &[document("b.txt")])
        .unwrap();
    // Zoe keeps the two apart, and Ada takes that back.
    let (anvil, bolt) = (
        record_id(&engine, "shop", "1"),
        record_id(&engine, "shop", "2"),
    );
    let ops = [
        Op::Constrain {
            kind: ConstraintKind::CannotLink,
            a: anvil,
            b: bolt,
        },
        Op::Unconstrain {
            kind: ConstraintKind::CannotLink,
            a: anvil,
            b: bolt,
        },
    ];
    for (actor, op) in [&ada, &zed].into_iter().zip(ops) {
        engine
            .commit(Changeset {
                id: engine.mint_ids(0, 1).changesets[0],
                base: engine.head().unwrap(),
                actor: actor.clone(),
                reason: None,
                run: None,
                ops: vec![op],
            })
            .unwrap();
    }
    // `zz` is Ada and `aa` is Zoe: by name, Ada comes first, then the engine, then Zoe.
    let names = vec![
        (zed.clone(), "Ada".to_owned()),
        (ada.clone(), "Zoe".to_owned()),
    ];

    let runs = engine
        .runs(&RunFilter {
            sort: Some(Sort::ascending(RunSort::StartedBy)),
            names: names.clone(),
            ..RunFilter::default()
        })
        .unwrap();
    let started: Vec<Actor> = runs.iter().map(|run| run.started_by.clone()).collect();
    assert_eq!(started.first(), Some(&zed), "{started:?}");
    assert_eq!(started.last(), Some(&ada), "{started:?}");
    assert!(
        started[1..started.len() - 1]
            .iter()
            .all(|actor| *actor == Actor::Engine)
    );
    // Without the names, each is sorted by itself.
    let unnamed = engine
        .runs(&RunFilter {
            sort: Some(Sort::ascending(RunSort::StartedBy)),
            ..RunFilter::default()
        })
        .unwrap();
    assert_eq!(unnamed.last().map(|run| &run.started_by), Some(&zed));

    let log = engine
        .logged_sorted(Sort::ascending(LogSort::Actor), None, 100, &names)
        .unwrap();
    let actors: Vec<Actor> = log
        .iter()
        .map(|entry| entry.entry.changeset.actor.clone())
        .collect();
    assert_eq!(actors.first(), Some(&zed), "{actors:?}");
    assert_eq!(actors.last(), Some(&ada), "{actors:?}");
}
