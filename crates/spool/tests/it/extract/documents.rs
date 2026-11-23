//! Documents and their versions, as they are stored and read.

use super::*;

#[test]
fn a_document_is_stored_once_and_a_changed_text_is_its_next_version() {
    let engine = Engine::open_in_memory(Options::seeded(1)).unwrap();
    let first = engine
        .add_documents(
            "news",
            &documents_of(&[("a", FIRST), ("b", SECOND), ("a", FIRST)]),
        )
        .unwrap();
    assert_eq!(
        (
            first.received,
            first.inserted,
            first.duplicates,
            first.superseded
        ),
        (3, 2, 1, 0)
    );
    // The store needs no manifest to hold documents, and the call is a run.
    let run = engine.run(first.run).unwrap();
    assert_eq!(run.kind, RunKind::Load);
    assert_eq!(run.status, RunStatus::Succeeded);
    assert_eq!(run.entity_type, None);
    assert_eq!(run.load, Some(first.clone()));

    let again = engine
        .add_documents("news", &documents_of(&[("a", FIRST)]))
        .unwrap();
    assert_eq!((again.inserted, again.duplicates), (0, 1));

    // The same key with another text is the next version, and so is the one
    // after it in the same call. The same text from another source is
    // another document.
    let edited = "Acme Inc closed an office.";
    let last = "Acme Inc moved its office.";
    let changed = engine
        .add_documents("news", &documents_of(&[("a", edited), ("a", last)]))
        .unwrap();
    assert_eq!((changed.inserted, changed.superseded), (2, 2));
    let elsewhere = engine
        .add_documents("wire", &documents_of(&[("a", FIRST)]))
        .unwrap();
    assert_eq!((elsewhere.inserted, elsewhere.superseded), (1, 0));

    let original = engine.document(id("a", FIRST)).unwrap();
    assert_eq!(original.source, "news");
    assert_eq!(original.key, "a");
    assert_eq!(original.uri.as_deref(), Some("https://news.test/a"));
    assert_eq!(original.text.as_deref(), Some(FIRST));
    assert_eq!(original.length, FIRST.chars().count() as u64);
    assert_eq!(original.load_run, first.run);
    assert_eq!(original.supersedes, None);
    assert_eq!(original.superseded_by, Some(id("a", edited)));
    assert!(original.extractions.is_empty());
    let middle = engine.document(id("a", edited)).unwrap();
    assert_eq!(middle.supersedes, Some(id("a", FIRST)));
    assert_eq!(middle.superseded_by, Some(id("a", last)));

    // A list leaves the text out, and can leave out what was replaced.
    let all = engine.documents(&DocumentFilter::default()).unwrap();
    assert_eq!(all.len(), 5);
    assert!(all.iter().all(|document| document.text.is_none()));
    assert!(all.windows(2).all(|pair| pair[0].id < pair[1].id));
    let latest: BTreeSet<DocumentId> = engine
        .documents(&DocumentFilter {
            source: Some("news".to_owned()),
            latest: true,
            ..DocumentFilter::default()
        })
        .unwrap()
        .iter()
        .map(|document| document.id)
        .collect();
    assert_eq!(latest, [id("a", last), id("b", SECOND)].into());
    let versions = engine
        .documents(&DocumentFilter {
            source: Some("news".to_owned()),
            key: Some("a".to_owned()),
            ..DocumentFilter::default()
        })
        .unwrap();
    assert_eq!(versions.len(), 3);
    let page = engine
        .documents(&DocumentFilter {
            after: Some(all[1].id),
            limit: 2,
            ..DocumentFilter::default()
        })
        .unwrap();
    assert_eq!(
        page.iter().map(|document| document.id).collect::<Vec<_>>(),
        [all[2].id, all[3].id]
    );

    assert_eq!(engine.info().unwrap().documents, 5);
    assert!(matches!(
        engine.document(id("z", "nothing")),
        Err(Error::UnknownDocument(_))
    ));
    // A document that cannot be stored stores none of the call, and starts
    // no run.
    let before = engine.runs(&RunFilter::default()).unwrap().len();
    let error = engine
        .add_documents("news", &documents_of(&[("c", "text"), ("d", "")]))
        .unwrap_err();
    assert!(matches!(error, Error::Documents(_)), "{error}");
    assert_eq!(engine.runs(&RunFilter::default()).unwrap().len(), before);
    assert_eq!(engine.info().unwrap().documents, 5);
    engine.verify().unwrap();
}

#[test]
fn documents_are_added_by_whoever_is_named() {
    let engine = Engine::open_in_memory(Options::seeded(1)).unwrap();
    let own = engine
        .add_documents("news", &documents_of(&[("a", FIRST)]))
        .unwrap();
    let desk = Actor::Agent("desk".to_owned());
    let named = engine
        .acting(desk.clone())
        .add_documents("news", &documents_of(&[("b", SECOND)]))
        .unwrap();
    assert_eq!(engine.run(own.run).unwrap().started_by, Actor::Engine);
    assert_eq!(engine.run(named.run).unwrap().started_by, desk);
}

#[test]
fn a_document_that_holds_again_the_text_it_had_has_a_version_more() {
    let engine = Engine::open_in_memory(Options::seeded(1)).unwrap();
    let edited = "Acme Inc closed an office.";
    let add = |documents: &[(&str, &str)]| {
        engine
            .add_documents("news", &documents_of(documents))
            .unwrap()
    };
    add(&[("a", FIRST)]);
    add(&[("a", edited)]);

    // The text is one the store holds as the document's first version, and
    // the document has another since. This is its third.
    let back = add(&[("a", FIRST)]);
    assert_eq!((back.inserted, back.duplicates, back.superseded), (1, 0, 1));
    let third = id("a", FIRST).held_again(id("a", edited));
    let latest = engine
        .documents(&DocumentFilter {
            latest: true,
            ..DocumentFilter::default()
        })
        .unwrap();
    assert_eq!(
        latest
            .iter()
            .map(|document| document.id)
            .collect::<Vec<_>>(),
        [third]
    );
    let document = engine.document(third).unwrap();
    assert_eq!(document.text.as_deref(), Some(FIRST));
    assert_eq!(document.supersedes, Some(id("a", edited)));
    let first = engine.document(id("a", FIRST)).unwrap();
    assert_eq!(
        (first.supersedes, first.superseded_by),
        (None, Some(id("a", edited)))
    );

    // What the document holds now is nothing new, and it can change the
    // same way again, in one call too. Two that say the same one after the
    // other are one version.
    let again = add(&[("a", FIRST)]);
    assert_eq!((again.inserted, again.duplicates), (0, 1));
    let more = add(&[("a", edited), ("a", edited), ("a", FIRST)]);
    assert_eq!((more.inserted, more.duplicates, more.superseded), (2, 1, 2));
    let fourth = id("a", edited).held_again(third);
    let fifth = id("a", FIRST).held_again(fourth);
    let document = engine.document(fifth).unwrap();
    assert_eq!(
        (document.supersedes, document.superseded_by),
        (Some(fourth), None)
    );
    assert_eq!(engine.info().unwrap().documents, 5);
    engine.verify().unwrap();
}

#[test]
#[ignore = "runs uv"]
fn only_the_latest_version_of_a_document_is_read() {
    let news = News::new();
    news.extractor("names.py", NAMES);
    let engine = &news.engine;
    news.add(&[("a", FIRST)]);
    assert_eq!(news.extract("names").unwrap().mentions, 2);

    // The document changes. Its new version is read, and what the old one
    // mentioned stays, under the old version's ID.
    let edited = "Initech Ltd opened an office. Acme Inc did not.";
    news.add(&[("a", edited)]);
    let report = news.extract("names").unwrap();
    assert_eq!(
        (report.documents, report.mentions, report.inserted),
        (1, 2, 2)
    );
    assert_eq!(report.superseded, 0);
    assert_eq!(news.mentions(id("a", FIRST)).len(), 2);
    assert_eq!(news.mentions(id("a", edited)).len(), 2);
    assert!(engine.document(id("a", edited)).unwrap().extractions.len() == 1);

    // Reading everything again reads the version that is current only.
    let again = engine
        .extract(&ExtractOptions {
            again: true,
            ..ExtractOptions::with("names")
        })
        .unwrap();
    assert_eq!((again.documents, again.duplicates), (1, 2));
    assert_eq!(
        engine.document(id("a", FIRST)).unwrap().extractions.len(),
        1
    );

    engine.resolve(&ResolveOptions::default()).unwrap();
    assert_eq!(
        news.identity(id("a", FIRST), 2, 10),
        news.identity(id("a", edited), 30, 38)
    );
    engine.verify().unwrap();
}

#[test]
#[ignore = "runs uv"]
fn a_document_that_changes_back_is_read_as_the_version_it_is_now() {
    let news = News::new();
    news.extractor("names.py", NAMES);
    let engine = &news.engine;
    news.add(&[("a", FIRST)]);
    news.extract("names").unwrap();
    let edited = "Initech Ltd opened an office. Acme Inc did not.";
    news.add(&[("a", edited)]);
    news.extract("names").unwrap();
    engine.resolve(&ResolveOptions::default()).unwrap();

    // The first text again is the document's latest version, which the
    // extractor has not read. What it finds there is found in that
    // version, and is the company the first version mentioned.
    news.add(&[("a", FIRST)]);
    let third = id("a", FIRST).held_again(id("a", edited));
    let report = news.extract("names").unwrap();
    assert_eq!(
        (report.documents, report.mentions, report.inserted),
        (1, 2, 2)
    );
    assert_eq!(news.mentions(third).len(), 2);
    assert_eq!(news.mentions(id("a", FIRST)).len(), 2);
    assert_eq!(news.extract("names").unwrap().documents, 0);

    engine.resolve(&ResolveOptions::default()).unwrap();
    assert_eq!(
        news.identity(third, 2, 10),
        news.identity(id("a", FIRST), 2, 10)
    );
    engine.verify().unwrap();
}
