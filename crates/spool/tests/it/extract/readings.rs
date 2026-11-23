//! What a reading records, and what it takes back when it no longer finds it.

use super::*;

#[test]
#[ignore = "runs uv"]
fn a_type_the_project_no_longer_declares_is_left_alone_by_a_reading() {
    let news = News::new();
    news.write(
        "entities/person/entity.yaml",
        "features: [name]\nmapping:\n  news:\n    name: name\n",
    );
    news.extractor(
        "names.py",
        r#"
from spool import extractor

@extractor
def find(document):
    for start, end in document.find("Acme Inc"):
        yield document.mention("company", start, end, name="Acme Inc")
    for start, end in document.find("Globex Ltd"):
        yield document.mention("person", start, end, name="Globex Ltd")
"#,
    );
    news.add(&[("a", FIRST)]);
    assert_eq!(news.extract("names").unwrap().mentions, 2);

    // The project drops `person`, and its extractor finds companies alone.
    fs::remove_dir_all(news.project().join("entities/person")).unwrap();
    news.extractor("names.py", NAMES);
    let again = news
        .engine
        .extract(&ExtractOptions {
            again: true,
            ..ExtractOptions::with("names")
        })
        .unwrap();
    assert_eq!(
        (again.taken_back, again.retract_runs.len()),
        (0, 0),
        "{again:?}"
    );
    let person = news
        .engine
        .records(&RecordFilter::keyed(
            "news",
            locator(id("a", FIRST), "person", 29, 39),
        ))
        .unwrap();
    assert_eq!(person[0].retracted, None);
}

#[test]
#[ignore = "runs uv"]
fn a_reading_is_recorded_against_the_version_it_found() {
    let news = News::new();
    let saying = |form: &str| {
        format!(
            r#"
from spool import extractor

@extractor
def find(document):
    for start, end in document.find("Acme Inc"):
        yield document.mention("company", start, end, name="Acme Inc", form="{form}")
"#
        )
    };
    news.add(&[("a", FIRST)]);
    let document = id("a", FIRST);
    let mut runs = Vec::new();
    // The mention says Inc, then Ltd, then Inc again: three versions, the
    // last of which holds what the first held.
    for form in ["Inc", "Ltd", "Inc"] {
        news.extractor("names.py", &saying(form));
        let report = news
            .engine
            .extract(&ExtractOptions {
                again: true,
                ..ExtractOptions::with("names")
            })
            .unwrap();
        runs.push(report.run);
    }
    let versions = news
        .engine
        .records(&RecordFilter::keyed(
            "news",
            locator(document, "company", 2, 10),
        ))
        .unwrap();
    assert_eq!(versions.len(), 3);
    let current = versions
        .iter()
        .find(|record| record.superseded_by.is_none())
        .unwrap();
    let found: Vec<_> = current.found_by.iter().map(|found| found.run).collect();
    assert_eq!(found, [runs[2]], "{versions:?}");
    // Each earlier version keeps the reading that found it.
    let earlier: BTreeSet<_> = versions
        .iter()
        .filter(|record| record.superseded_by.is_some())
        .flat_map(|record| record.found_by.iter().map(|found| found.run))
        .collect();
    assert_eq!(earlier, BTreeSet::from([runs[0], runs[1]]));
}

#[test]
#[ignore = "runs uv"]
fn mentions_no_extractor_finds_any_more_are_taken_back_and_come_back() {
    let news = News::new();
    let finding = |names: &[&str]| {
        format!(
            r#"
from spool import extractor

@extractor
def find(document):
    for name in {names:?}:
        for start, end in document.find(name):
            yield document.mention("company", start, end, name=name)
"#
        )
    };
    news.extractor("names.py", &finding(&["Acme Inc", "Globex Ltd"]));
    news.extractor("others.py", &finding(&["Acme Inc"]));
    news.add(&[("a", FIRST)]);
    news.extract("names").unwrap();
    news.extract("others").unwrap();
    news.engine.resolve(&ResolveOptions::default()).unwrap();
    let document = id("a", FIRST);
    let acme = news.identity(document, 2, 10).unwrap();
    let globex = news.identity(document, 29, 39).unwrap();
    assert_ne!(acme, globex);

    // The changed extractor finds Acme alone. Globex, which no extractor
    // finds now, is taken back; its identity keeps its ID, with no records.
    news.extractor("names.py", &finding(&["Acme Inc"]));
    let again = news
        .engine
        .extract(&ExtractOptions {
            again: true,
            ..ExtractOptions::with("names")
        })
        .unwrap();
    assert_eq!(again.taken_back, 1);
    let [retract_run] = again.retract_runs[..] else {
        panic!("one retract run: {again:?}");
    };
    let taken = news
        .engine
        .records(&RecordFilter::keyed(
            "news",
            locator(document, "company", 29, 39),
        ))
        .unwrap();
    assert_eq!(taken[0].retracted, Some(retract_run));
    assert_eq!(taken[0].identity, None);
    assert!(members(&news.engine, globex).is_empty());
    let view = news.engine.identity(globex).unwrap();
    assert_eq!((view.current, view.members.len()), (globex, 0));
    assert_eq!(
        news.engine.run(retract_run).unwrap().started_by,
        Actor::Engine
    );
    // Acme stays, found by both versions of `names` and by `others`.
    let found = &news
        .engine
        .records(&RecordFilter::keyed(
            "news",
            locator(document, "company", 2, 10),
        ))
        .unwrap()[0];
    let versions: BTreeSet<(&str, &str)> = found
        .found_by
        .iter()
        .map(|found| (found.extractor.as_str(), found.version.as_str()))
        .collect();
    assert_eq!(versions.len(), 3, "{:?}", found.found_by);
    assert_eq!(found.identity, Some(acme));

    // `others`, read again, still finds Acme, and nothing is taken back.
    let others = news
        .engine
        .extract(&ExtractOptions {
            again: true,
            ..ExtractOptions::with("others")
        })
        .unwrap();
    assert_eq!((others.taken_back, others.retract_runs.len()), (0, 0));

    // Found again, Globex comes back to the identity it left.
    news.extractor("names.py", &finding(&["Acme Inc", "Globex Ltd"]));
    news.engine
        .extract(&ExtractOptions {
            again: true,
            ..ExtractOptions::with("names")
        })
        .unwrap();
    assert_eq!(news.identity(document, 29, 39), Some(globex));
    news.engine.verify().unwrap();
}

#[test]
#[ignore = "runs uv"]
fn opening_a_format_11_store_preserves_mentions_and_reading_counts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.duckdb");
    let open = || Engine::open(&path, Options::seeded(1)).unwrap();
    let mut news = News::with_engine(dir, open());
    news.extractor("names.py", NAMES);
    news.extractor("others.py", NAMES);
    news.add(&[("a", FIRST)]);
    let names = news.extract("names").unwrap();
    let others = news.extract("others").unwrap();
    assert_eq!((names.inserted, others.duplicates), (2, 2));
    news.engine.resolve(&ResolveOptions::default()).unwrap();
    let document = id("a", FIRST);
    let mut expected = news.mentions(document);
    for mention in &mut expected {
        mention.found_by.clear();
    }
    let log = news.engine.log().unwrap();
    drop(news.engine);

    // Format 11 stored the same readings and records, with no report of
    // per-document failures and no links from a reading to the mentions.
    duckdb::Connection::open(&path)
        .unwrap()
        .execute_batch(
            "ALTER TABLE spool.extraction DROP COLUMN failure;
             DROP TABLE spool.extraction_found;
             UPDATE spool.meta SET value = '11' WHERE key = 'format_version';",
        )
        .unwrap();

    for _ in 0..2 {
        news.engine = Arc::new(open());
        assert_eq!(news.engine.info().unwrap().format_version, "12");
        assert_eq!(news.engine.log().unwrap(), log);
        let readings = news.engine.document(document).unwrap().extractions;
        assert_eq!(readings.len(), 2);
        assert_eq!(readings[0].run, names.run);
        assert_eq!(readings[1].run, others.run);
        assert!(
            readings
                .iter()
                .all(|read| read.mentions == 2 && read.failure.is_none())
        );

        assert_eq!(news.mentions(document), expected);
        drop(news.engine);
    }

    news.engine = Arc::new(open());
    news.engine.set_user_code(UserCode::new(news.project()));
    let reread = || {
        news.engine
            .extract(&ExtractOptions {
                again: true,
                ..ExtractOptions::with("names")
            })
            .unwrap()
    };

    // Count-only readings remain in the history. Per-mention tracking
    // starts with readings made after the upgrade.
    news.extractor(
        "names.py",
        "from spool import extractor\n\n@extractor\ndef find(document):\n    return []\n",
    );
    assert_eq!(reread().taken_back, 0);
    assert_eq!(news.mentions(document), expected);

    news.extractor("names.py", NAMES);
    let reading = reread();
    assert_eq!((reading.inserted, reading.duplicates), (0, 2));
    for mention in news.mentions(document) {
        assert_eq!(mention.found_by.len(), 1);
        assert_eq!(mention.found_by[0].run, reading.run);
    }
    news.engine.verify().unwrap();
}

#[test]
#[ignore = "runs bun"]
fn a_typescript_extractor_finds_what_a_python_one_finds() {
    let python = News::new();
    python.extractor("names.py", NAMES);
    let typescript = News::new();
    typescript.extractor(
        "names.ts",
        r#"
import { extractor } from "spool-strategy";

await extractor(async (task) => {
  const documents = await task.documents();
  if (task.entityTypes.join() !== "company") throw new Error("the types are not there");
  return documents.flatMap((document) =>
    [...document.text.matchAll(/[A-Z]\p{L}+ (?:Inc|Ltd)/gu)].map((found) =>
      document.mention("company", found.index, found.index + found[0].length, { name: found[0] }),
    ),
  );
});
"#,
    );

    let mut found = Vec::new();
    for news in [&python, &typescript] {
        news.add(&[("a", FIRST), ("b", SECOND)]);
        let report = news.extract("names").unwrap();
        assert_eq!(
            (report.documents, report.mentions, report.inserted),
            (2, 4, 4)
        );
        // The rocket is two of JavaScript's characters and one of the
        // engine's: the helper counts as the engine does.
        assert_eq!(
            news.mentions(id("a", FIRST))[0].evidence.as_ref().unwrap()["text"],
            "Acme Inc"
        );
        let ids: BTreeSet<_> = news
            .engine
            .records(&RecordFilter::default())
            .unwrap()
            .iter()
            .map(|record| record.id)
            .collect();
        found.push(ids);
        assert!(news.tasks().is_empty(), "{:?}", news.tasks());
    }
    // The same mentions with the same attributes are the same records,
    // whichever language found them.
    assert_eq!(found[0], found[1]);

    // An extractor that throws fails its run.
    typescript.extractor(
        "names.ts",
        "import { extractor } from \"spool-strategy\";\n\
         await extractor(() => { throw new Error(\"cannot read\"); });\n",
    );
    typescript.add(&[("c", "Initech Ltd.")]);
    let message = extractor_error(typescript.extract("names"));
    assert!(message.contains("cannot read"), "{message}");
}
