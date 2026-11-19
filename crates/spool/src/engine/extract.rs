//! Documents and extraction: storing text, running an extractor over it,
//! and storing the mentions it finds as records.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;

use duckdb::params;
use duckdb::vtab::arrow::ArrowBatchRegistration;
use serde_json::json;
use spool_core::{Actor, DocumentId, EntityType, RunId};

use super::{Engine, Inner, Reading, lock};
use crate::batch::{Column, batch};
use crate::documents::{
    self, DocumentEntry, DocumentFilter, DocumentReading, ExtractOptions, ExtractReport,
    ExtractorRef, FailedDocument, Mention, NewDocument, RawMention, TaskDocument,
};
use crate::error::{Error, Result};
use crate::features;
use crate::load::{self, LoadReport};
use crate::project::{Declaration, Extractor};
use crate::retract::NOT_RETRACTED;
use crate::runs::RunStatus;
use crate::store::parse;
use crate::usercode::{self, ExtractType, Task, UserCode};

/// The documents of the extraction in progress. One extraction runs at a
/// time, so one table is enough.
const TASK: &str = "spool_extract_task";

/// The most characters of why an extractor could not read a document that
/// are kept.
const MOST_OF_A_REASON: usize = 1_000;

/// The column types a mention's `start` and `end` may have.
const WHOLE_NUMBERS: [&str; 9] = [
    "TINYINT",
    "SMALLINT",
    "INTEGER",
    "BIGINT",
    "HUGEINT",
    "UTINYINT",
    "USMALLINT",
    "UINTEGER",
    "UBIGINT",
];

impl Engine {
    /// Stores documents from one source. A document that is already known,
    /// by its source, key and text, creates nothing. A new text under a key
    /// the source has used before is the next version of that document.
    ///
    /// Nothing is extracted yet: [`extract`](Self::extract) does that.
    pub fn add_documents(&self, source: &str, documents: &[NewDocument]) -> Result<LoadReport> {
        self.lock().add_documents(&Actor::Engine, source, documents)
    }

    /// A document with its text, the extractors that have read it and the
    /// one that is reading it.
    pub fn document(&self, id: DocumentId) -> Result<DocumentEntry> {
        let mut document = self
            .read(|conn| documents::get(conn, id))?
            .ok_or(Error::UnknownDocument(id))?;
        self.mark_reading(std::slice::from_mut(&mut document));
        Ok(document)
    }

    /// Documents without their text, ordered by ID.
    pub fn documents(&self, filter: &DocumentFilter) -> Result<Vec<DocumentEntry>> {
        let mut documents = self.read(|conn| documents::list(conn, filter))?;
        self.mark_reading(&mut documents);
        Ok(documents)
    }

    /// Says of each of the documents the extraction in progress reads that
    /// it is reading them.
    fn mark_reading(&self, documents: &mut [DocumentEntry]) {
        let reading = lock(&self.reading);
        let Some(reading) = reading.as_ref() else {
            return;
        };
        for document in documents {
            if reading.documents.contains(&document.id) {
                document.reading = Some(DocumentReading {
                    extractor: reading.extractor.clone(),
                    run: reading.run,
                });
            }
        }
    }

    /// Runs one of the project's extractors over the documents it has not
    /// read, and stores each mention it finds as a record of the
    /// entity type the mention names. Only the latest version of a document
    /// is read.
    ///
    /// A record's source is its document's, its key is the mention's
    /// [locator](documents::locator), its payload is the attributes the
    /// extractor gave, and its evidence says where in which document it
    /// was found and quotes the text there. Nothing is matched: the
    /// records are new to the next [`resolve`](Self::resolve) like
    /// rows that were loaded.
    ///
    /// The extractor is user code and runs as a process of its own. While
    /// it runs the engine is free. An extractor that fails, does not finish
    /// in time, or writes a mention the engine refuses fails its run, and
    /// nothing it found is stored.
    pub fn extract(&self, options: &ExtractOptions) -> Result<ExtractReport> {
        self.extract_by(&Actor::Engine, options)
    }

    pub(super) fn extract_by(&self, by: &Actor, options: &ExtractOptions) -> Result<ExtractReport> {
        let _turn = lock(&self.resolving);
        let mut inner = self.lock();
        let manifest = inner.manifest.clone().ok_or(Error::NoManifest)?;
        let extractor = manifest
            .extractor(&options.extractor)
            .cloned()
            .ok_or_else(|| Error::UnknownExtractor {
                extractor: options.extractor.clone(),
                known: manifest
                    .extractors
                    .iter()
                    .map(|extractor| extractor.name.clone())
                    .collect(),
            })?;
        let user_code = self.user_code().ok_or_else(|| Error::Extractor {
            extractor: extractor.name.clone(),
            message: "it is user code, and this engine was not told where the project directory is"
                .to_owned(),
        })?;
        // A file that is not the one the manifest was compiled from is
        // never run, and neither is one that nothing here can run.
        user_code.check(&(&extractor).into())?;

        let run = inner.start_run(
            "extract",
            by,
            None,
            &json!({
                "extractor": ExtractorRef::from(&extractor),
                "source": options.source,
                "again": options.again,
                "since": options.since,
                "limit": options.limit,
            }),
        )?;
        let task = inner.in_run(run, |inner| {
            inner.write_extract_task(run, &extractor, &user_code, options)
        })?;
        let Some((mut task, left)) = task else {
            // There is nothing to read, so nothing is started.
            let report = ExtractReport::empty(run);
            inner.finish_run(run, RunStatus::Succeeded, Some(&json!(report)))?;
            return Ok(report);
        };

        let given = inner.task_document_ids()?;
        *lock(&self.reading) = Some(Reading {
            run,
            extractor: extractor.name.clone(),
            documents: given,
        });
        let _reading = ReadingEnds(&self.reading, &user_code.running, run);

        // The extractor runs with the writer let go. The run stays
        // `running`, and it reads the documents as they were written.
        drop(inner);
        let executed = task.execute();
        let mut inner = self.lock();
        let report = inner.in_run(run, |inner| {
            executed?;
            if !user_code.running.begin_storing(run) {
                return Err(task.cancelled());
            }
            inner
                .take_mentions(by, run, &task, &extractor, left)
                .map_err(|error| match error {
                    Error::Mentions(_) => task.rejected(error),
                    other => other,
                })
        })?;
        task.remove();
        let mut report = report;
        if report.documents > report.failed.len() as u64 {
            let (taken_back, runs) = inner.take_back(by, run, &extractor)?;
            if taken_back > 0 || !runs.is_empty() {
                report.taken_back = taken_back;
                report.retract_runs = runs;
                inner.store.conn.execute(
                    "UPDATE spool.run SET result = ?::JSON WHERE run_id = ?",
                    params![json!(report).to_string(), run.0 as i64],
                )?;
            }
        }
        Ok(report)
    }
}

/// Forgets what the extraction in progress reads, and whether it was
/// cancelled, when it ends, however it ends.
struct ReadingEnds<'a>(&'a Mutex<Option<Reading>>, &'a usercode::Running, RunId);

impl Drop for ReadingEnds<'_> {
    fn drop(&mut self) {
        *lock(self.0) = None;
        self.1.forget(self.2);
    }
}

/// A SQL string literal.
fn sql_text(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

impl Inner {
    pub(super) fn add_documents(
        &mut self,
        by: &Actor,
        source: &str,
        documents: &[NewDocument],
    ) -> Result<LoadReport> {
        let staged = documents::stage(source, documents)?;
        let received = documents.len() as u64;
        let run = self.start_run(
            "load",
            by,
            None,
            &json!({ "source": source, "documents": received }),
        )?;
        self.in_run(run, |inner| {
            let now = inner.store.clock.now_micros();
            let tx = inner.store.conn.transaction()?;
            let inserted = match staged {
                Some(staged) => documents::insert(&tx, source, now, run, staged)?,
                None => 0,
            };
            let superseded: i64 = tx.query_row(
                "SELECT count(*) FROM spool.document
                 WHERE load_run = ? AND supersedes IS NOT NULL",
                [run.0 as i64],
                |row| row.get(0),
            )?;
            let report = LoadReport {
                run,
                received,
                inserted,
                duplicates: received - inserted,
                superseded: superseded as u64,
                restored: 0,
                returned: 0,
                committed: None,
                possible_splits: 0,
            };
            let finished = inner.store.clock.now_micros();
            tx.execute(
                "UPDATE spool.run
                 SET status = 'succeeded', finished_at = make_timestamptz(?), result = ?::JSON
                 WHERE run_id = ?",
                params![finished, json!(report).to_string(), run.0 as i64],
            )?;
            tx.commit()?;
            Ok(report)
        })
    }

    /// Chooses the documents an extraction reads and writes them where the
    /// extractor finds them, with how many its limit left out. `None` when
    /// there is nothing to read.
    fn write_extract_task(
        &mut self,
        run: RunId,
        extractor: &Extractor,
        user_code: &UserCode,
        options: &ExtractOptions,
    ) -> Result<Option<(Task, u64)>> {
        let manifest = self.manifest.clone().ok_or(Error::NoManifest)?;
        let conn = &self.store.conn;
        let of_source = match &options.source {
            Some(source) => format!("AND d.source_id = {}", sql_text(source)),
            None => String::new(),
        };
        let extractor_is = format!("e.extractor = {}", sql_text(&extractor.name));
        // Read: a reading that did not fail, by whatever version.
        let read = match options.again {
            false => format!(
                "AND NOT EXISTS (
                   SELECT 1 FROM spool.extraction e
                   WHERE e.document_id = d.document_id AND {extractor_is} AND e.failure IS NULL)"
            ),
            true => String::new(),
        };
        // Tried since: a reading, failed or not, in this run or a later one.
        let tried = match options.since {
            Some(since) => format!(
                "AND NOT EXISTS (
                   SELECT 1 FROM spool.extraction e
                   WHERE e.document_id = d.document_id AND {extractor_is}
                     AND e.run_id >= {})",
                since.0
            ),
            None => String::new(),
        };
        let limit = match options.limit {
            Some(limit) => format!("LIMIT {limit}"),
            None => String::new(),
        };
        conn.execute_batch(&format!(
            "CREATE OR REPLACE TEMP TABLE {TASK} AS
             SELECT d.document_id,
                    row_number() OVER (ORDER BY d.load_run, d.source_id, d.source_key) AS ord,
                    count(*) OVER () AS matching
             FROM spool.document d
             WHERE NOT EXISTS (
                 SELECT 1 FROM spool.document later WHERE later.supersedes = d.document_id)
               {of_source}
               {read}
               {tried}
             ORDER BY d.load_run, d.source_id, d.source_key
             {limit}"
        ))?;
        let (chosen, matching): (i64, i64) = conn.query_row(
            &format!("SELECT count(*), coalesce(max(matching), 0) FROM {TASK}"),
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if chosen == 0 {
            return Ok(None);
        }
        let left = (matching - chosen) as u64;

        let entity_types: Vec<ExtractType> = manifest
            .entity_types
            .iter()
            .map(|entity_type| ExtractType {
                name: entity_type.name.as_str(),
                instructions: entity_type.extract.as_deref(),
                features: &entity_type.declaration.columns,
            })
            .collect();
        let task = Task::extract(user_code, extractor, run, &entity_types)?;
        conn.execute_batch(&format!(
            "COPY (
               SELECT d.document_id::VARCHAR AS document_id, d.source_id AS source,
                      d.source_key AS key, d.uri, d.text
               FROM {TASK} t JOIN spool.document d ON d.document_id = t.document_id
               ORDER BY t.ord
             ) TO {} (FORMAT parquet)",
            usercode::sql_path(&task.path(usercode::DOCUMENTS))
        ))?;
        Ok(Some((task, left)))
    }

    /// Reads the mentions an extractor wrote, checks them against the
    /// documents it was given, and stores them as records.
    fn take_mentions(
        &mut self,
        by: &Actor,
        run: RunId,
        task: &Task,
        extractor: &Extractor,
        left: u64,
    ) -> Result<ExtractReport> {
        let manifest = self.manifest.clone().ok_or(Error::NoManifest)?;
        let given = self.task_documents()?;
        let failures = self.read_failures(&task.path(usercode::FAILED), &given)?;
        // Nothing found in a document the extractor could not read is kept.
        let raw: Vec<RawMention> = self
            .read_mentions(&task.path(usercode::MENTIONS))?
            .into_iter()
            .filter(|mention| {
                mention
                    .document
                    .as_deref()
                    .and_then(|document| document.parse::<DocumentId>().ok())
                    .is_none_or(|document| !failures.contains_key(&document))
            })
            .collect();
        let staged = documents::stage_mentions(
            &raw,
            &given,
            &|name| manifest.entity_type(name).is_some(),
            extractor,
        )?;
        // A mention is a record of its document's source, and may say only
        // what that source's file lists.
        for (index, mention) in staged.iter().enumerate() {
            let fail = |message: String| Error::Mentions(format!("row {}: {message}", index + 1));
            let entry = manifest
                .entity_type(&mention.entity_type)
                .expect("a staged mention is of a declared type");
            let source = manifest.source_of(&mention.source, entry).map_err(&fail)?;
            let attributes: serde_json::Map<String, serde_json::Value> =
                serde_json::from_str(&mention.payload).unwrap_or_default();
            if let Some(unknown) = attributes
                .keys()
                .find(|name| !source.columns.contains(name))
            {
                return Err(fail(format!(
                    "it has an attribute `{unknown}`, which sources/{}.yaml does not list",
                    source.name
                )));
            }
        }

        // Each type's declaration, for the feature rows of what is new.
        let mut declared: BTreeMap<&str, (EntityType, Declaration, u32)> = BTreeMap::new();
        for mention in &staged {
            if !declared.contains_key(mention.entity_type.as_str()) {
                let (_, entry, version) = self.declared(&mention.entity_type)?;
                declared.insert(
                    &mention.entity_type,
                    (entry.name.clone(), entry.declaration.clone(), version),
                );
            }
        }
        let mut found: HashMap<DocumentId, i64> = given.keys().map(|id| (*id, 0)).collect();
        let mut failed: Vec<FailedDocument> = failures
            .iter()
            .map(|(id, reason)| FailedDocument {
                document: *id,
                source: given[id].source.clone(),
                key: given[id].key.clone(),
                reason: reason.clone(),
            })
            .collect();
        failed.sort_by(|a, b| (&a.source, &a.key).cmp(&(&b.source, &b.key)));
        for mention in &staged {
            *found.entry(mention.document).or_default() += 1;
        }

        // The records are stored under a run of their own, which
        // begins now. A match run that began while the extractor worked
        // has a later ID than the extract run, and takes as new only what
        // was loaded before it: under the extract run's ID these
        // records would be new to no run at all.
        let reference = ExtractorRef::from(extractor);
        let load_run = match staged.is_empty() {
            true => None,
            false => Some(self.start_run(
                "load",
                by,
                None,
                &json!({ "extract_run": run, "extractor": reference }),
            )?),
        };
        let store = |inner: &mut Self| -> Result<(ExtractReport, Option<LoadReport>)> {
            let now = inner.store.clock.now_micros();
            let tx = inner.store.conn.transaction()?;
            let (mut inserted, mut restored) = (0, 0);
            if let Some(load_run) = load_run {
                for ((entity_type, source), staged) in grouped(&staged)? {
                    let stored = load::insert(&tx, entity_type, source, now, load_run, staged)?;
                    inserted += stored.inserted;
                    restored += stored.restored;
                }
                for (name, declaration, version) in declared.values() {
                    let problem = features::fill(
                        &tx,
                        name,
                        declaration,
                        *version,
                        &features::of_run(name, load_run),
                    )?;
                    if let Some(problem) = problem {
                        return Err(Error::Mentions(problem));
                    }
                }
            }

            let (mut ids, mut counts, mut why) = (Vec::new(), Vec::new(), Vec::new());
            for (id, count) in &found {
                ids.push(Some(id.to_string()));
                counts.push(*count);
                why.push(failures.get(id).cloned());
            }
            tx.execute(
                "INSERT INTO spool.extraction
                 SELECT s.document_id::UUID, $1, $2, $3, s.mentions, s.failure
                 FROM arrow($4) s",
                params![
                    extractor.name,
                    extractor.hash,
                    run.0 as i64,
                    &ArrowBatchRegistration::new(batch(vec![
                        ("document_id", Column::Text(ids)),
                        ("mentions", Column::Integer(counts)),
                        ("failure", Column::Text(why)),
                    ])?)
                ],
            )?;
            // Which mentions this reading found, new or found again, and
            // the version of the extractor that found them.
            if !staged.is_empty() {
                let column = |value: fn(&Mention) -> String| {
                    Column::Text(staged.iter().map(|mention| Some(value(mention))).collect())
                };
                tx.execute(
                    "INSERT INTO spool.extraction_found
                     SELECT $1, f.document_id::UUID, $2, $3, f.entity_type, f.record_key,
                            f.record_id::UUID
                     FROM arrow($4) f",
                    params![
                        run.0 as i64,
                        extractor.name,
                        extractor.hash,
                        &ArrowBatchRegistration::new(batch(vec![
                            (
                                "document_id",
                                column(|mention| mention.document.to_string())
                            ),
                            ("entity_type", column(|mention| mention.entity_type.clone())),
                            ("record_key", column(|mention| mention.key.clone())),
                            ("record_id", column(|mention| mention.record.to_string())),
                        ])?)
                    ],
                )?;
            }

            let superseded: i64 = match load_run {
                Some(load_run) => tx.query_row(
                    "SELECT count(*) FROM spool.record
                     WHERE load_run = ? AND supersedes IS NOT NULL",
                    [load_run.0 as i64],
                    |row| row.get(0),
                )?,
                None => 0,
            };
            let mentions = staged.len() as u64;
            let report = ExtractReport {
                run,
                documents: given.len() as u64,
                mentions,
                inserted,
                duplicates: mentions - inserted,
                superseded: superseded as u64,
                load_run,
                left,
                failed: failed.clone(),
                taken_back: 0,
                retract_runs: Vec::new(),
                stderr: task.said(),
            };
            let finished = inner.store.clock.now_micros();
            let finish = "UPDATE spool.run
                 SET status = 'succeeded', finished_at = make_timestamptz(?), result = ?::JSON
                 WHERE run_id = ?";
            let stored = load_run.map(|load_run| LoadReport {
                run: load_run,
                received: mentions,
                inserted,
                duplicates: mentions - inserted,
                superseded: superseded as u64,
                restored,
                returned: 0,
                committed: None,
                possible_splits: 0,
            });
            if let Some(stored) = &stored {
                tx.execute(
                    finish,
                    params![finished, json!(stored).to_string(), stored.run.0 as i64],
                )?;
            }
            tx.execute(
                finish,
                params![finished, json!(report).to_string(), run.0 as i64],
            )?;
            tx.commit()?;
            Ok((report, stored))
        };
        let (report, stored) = match load_run {
            Some(load_run) => self.in_run(load_run, store),
            None => store(self),
        }?;
        if let Some(stored) = stored {
            self.give_back(
                stored,
                format!("extractor `{}` found the rows again", extractor.name),
            )?;
        }
        self.store
            .conn
            .execute_batch(&format!("DROP TABLE IF EXISTS {TASK}"))?;
        Ok(report)
    }

    /// The documents the extractor said it could not read, with why: the
    /// optional `failed.parquet`, of `document_id` and `reason`. A document
    /// it was not given refuses the run, as a mention in one does.
    fn read_failures(
        &self,
        path: &Path,
        given: &HashMap<DocumentId, TaskDocument>,
    ) -> Result<HashMap<DocumentId, String>> {
        let mut failures = HashMap::new();
        if !path.is_file() {
            return Ok(failures);
        }
        let unreadable = |error: duckdb::Error| {
            Error::Mentions(format!(
                "{} could not be read as `document_id` and `reason`: {error}",
                usercode::FAILED
            ))
        };
        let mut statement = self
            .store
            .conn
            .prepare(&format!(
                "SELECT document_id::VARCHAR, reason::VARCHAR FROM read_parquet({})",
                usercode::sql_path(path)
            ))
            .map_err(unreadable)?;
        let mut rows = statement.query([]).map_err(unreadable)?;
        let mut row_number = 0;
        while let Some(row) = rows.next().map_err(unreadable)? {
            row_number += 1;
            let document: Option<String> = row.get(0).map_err(unreadable)?;
            let reason: Option<String> = row.get(1).map_err(unreadable)?;
            let document = document
                .as_deref()
                .and_then(|document| document.parse::<DocumentId>().ok())
                .filter(|document| given.contains_key(document))
                .ok_or_else(|| {
                    Error::Mentions(format!(
                        "{} row {row_number} names a document the extractor was not given",
                        usercode::FAILED
                    ))
                })?;
            let reason = reason
                .map(|reason| {
                    reason
                        .trim()
                        .chars()
                        .take(MOST_OF_A_REASON)
                        .collect::<String>()
                })
                .filter(|reason| !reason.is_empty())
                .unwrap_or_else(|| "the extractor did not say why".to_owned());
            failures.entry(document).or_insert(reason);
        }
        Ok(failures)
    }

    /// Takes back the mentions of the documents a run read that no
    /// extractor finds in them any more: each was found by some reading,
    /// and the latest reading by each extractor that read the document,
    /// this run's among them, finds it no longer. A mention that another
    /// extractor still finds stays. They are taken back as a source takes
    /// back rows, by a retract run for each source and entity type, under
    /// the name of whoever started the extraction.
    fn take_back(
        &mut self,
        by: &Actor,
        run: RunId,
        extractor: &Extractor,
    ) -> Result<(u64, Vec<RunId>)> {
        let mut statement = self.store.conn.prepare(&format!(
            "WITH read_now AS (
               SELECT document_id FROM spool.extraction
               WHERE run_id = $1 AND failure IS NULL),
             latest AS (
               SELECT e.document_id, e.extractor, max(e.run_id) AS run_id
               FROM spool.extraction e JOIN read_now USING (document_id)
               WHERE e.failure IS NULL
               GROUP BY e.document_id, e.extractor),
             still AS (
               SELECT f.document_id, f.entity_type, f.record_key
               FROM spool.extraction_found f
               JOIN latest l
                 ON l.document_id = f.document_id AND l.extractor = f.extractor
                AND l.run_id = f.run_id),
             ever AS (
               SELECT DISTINCT f.document_id, f.entity_type, f.record_key
               FROM spool.extraction_found f JOIN read_now USING (document_id))
             SELECT d.source_id, ever.entity_type, ever.record_key
             FROM ever
             JOIN spool.document d ON d.document_id = ever.document_id
             WHERE NOT EXISTS (
                 SELECT 1 FROM still
                 WHERE still.document_id = ever.document_id
                   AND still.entity_type = ever.entity_type
                   AND still.record_key = ever.record_key)
               AND EXISTS (
                 SELECT 1 FROM spool.record r
                 WHERE r.source_id = d.source_id AND r.entity_type = ever.entity_type
                   AND r.source_key = ever.record_key
                   AND r.{NOT_RETRACTED})
             ORDER BY 1, 2, 3"
        ))?;
        let mut rows = statement.query([run.0 as i64])?;
        let mut stale: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
        while let Some(row) = rows.next()? {
            stale
                .entry((row.get(0)?, row.get(1)?))
                .or_default()
                .push(row.get(2)?);
        }
        drop(rows);
        drop(statement);
        let (mut taken_back, mut runs) = (0, Vec::new());
        for ((source, entity_type), keys) in stale {
            let report = self.retract(
                &source,
                &entity_type,
                &keys,
                by.clone(),
                Some(format!(
                    "extractor `{}` no longer finds it, in run {run}",
                    extractor.name
                )),
            )?;
            taken_back += report.retracted;
            runs.push(report.run);
        }
        Ok((taken_back, runs))
    }

    /// The IDs of the documents the extraction in progress was given.
    fn task_document_ids(&self) -> Result<HashSet<DocumentId>> {
        let mut statement = self
            .store
            .conn
            .prepare(&format!("SELECT document_id::VARCHAR FROM {TASK}"))?;
        let mut rows = statement.query([])?;
        let mut ids = HashSet::new();
        while let Some(row) = rows.next()? {
            ids.insert(parse(&row.get::<_, String>(0)?)?);
        }
        Ok(ids)
    }

    /// The documents the extraction in progress was given, as the store
    /// holds them. What the extractor's own copy says is not asked.
    fn task_documents(&self) -> Result<HashMap<DocumentId, TaskDocument>> {
        let mut statement = self.store.conn.prepare(&format!(
            "SELECT d.document_id::VARCHAR, d.source_id, d.source_key, d.uri, d.text
             FROM {TASK} t JOIN spool.document d ON d.document_id = t.document_id"
        ))?;
        let mut rows = statement.query([])?;
        let mut given = HashMap::new();
        while let Some(row) = rows.next()? {
            given.insert(
                parse(&row.get::<_, String>(0)?)?,
                TaskDocument {
                    source: row.get(1)?,
                    key: row.get(2)?,
                    uri: row.get(3)?,
                    text: row.get(4)?,
                },
            );
        }
        Ok(given)
    }

    fn read_mentions(&self, path: &Path) -> Result<Vec<RawMention>> {
        let unreadable = |error: duckdb::Error| {
            Error::Mentions(format!(
                "{} could not be read as `document_id`, `entity_type`, `start`, `end` and \
                 `attributes`: {error}",
                usercode::MENTIONS
            ))
        };
        // A position is read only from a column of whole numbers. A cast
        // would round 1.6 to 2, and store a mention at a place the extractor
        // did not name.
        let mut described = self
            .store
            .conn
            .prepare(&format!(
                "SELECT column_name, column_type
                 FROM (DESCRIBE SELECT \"start\", \"end\" FROM read_parquet({}))",
                usercode::sql_path(path)
            ))
            .map_err(unreadable)?;
        let mut columns = described.query([]).map_err(unreadable)?;
        while let Some(column) = columns.next().map_err(unreadable)? {
            let name: String = column.get(0).map_err(unreadable)?;
            let data_type: String = column.get(1).map_err(unreadable)?;
            if !WHOLE_NUMBERS.contains(&data_type.as_str()) {
                return Err(Error::Mentions(format!(
                    "`{name}` is a {data_type} column, and a position is a whole number"
                )));
            }
        }

        let mut statement = self
            .store
            .conn
            .prepare(&format!(
                "SELECT document_id::VARCHAR, entity_type::VARCHAR, \"start\"::BIGINT,
                        \"end\"::BIGINT, attributes::VARCHAR
                 FROM read_parquet({})",
                usercode::sql_path(path)
            ))
            .map_err(unreadable)?;
        let mut rows = statement.query([]).map_err(unreadable)?;
        let mut raw = Vec::new();
        while let Some(row) = rows.next().map_err(unreadable)? {
            let read = |error: duckdb::Error| unreadable(error);
            raw.push(RawMention {
                document: row.get(0).map_err(read)?,
                entity_type: row.get(1).map_err(read)?,
                start: row.get(2).map_err(read)?,
                end: row.get(3).map_err(read)?,
                attributes: row.get(4).map_err(read)?,
            });
        }
        Ok(raw)
    }
}

/// The mentions of each entity type and source, as the batch a load
/// stores. `ord` is the mention's position in what the extractor wrote.
#[allow(clippy::type_complexity)]
fn grouped(
    mentions: &[Mention],
) -> Result<Vec<((&str, &str), duckdb::arrow::record_batch::RecordBatch)>> {
    let mut groups: BTreeMap<(&str, &str), Vec<(usize, &Mention)>> = BTreeMap::new();
    for (ord, mention) in mentions.iter().enumerate() {
        groups
            .entry((&mention.entity_type, &mention.source))
            .or_default()
            .push((ord, mention));
    }
    groups
        .into_iter()
        .map(|(group, mentions)| {
            let text = |field: fn(&Mention) -> String| {
                Column::Text(
                    mentions
                        .iter()
                        .map(|(_, mention)| Some(field(mention)))
                        .collect(),
                )
            };
            let staged = batch(vec![
                (
                    "ord",
                    Column::Integer(mentions.iter().map(|(ord, _)| *ord as i64).collect()),
                ),
                ("record_id", text(|mention| mention.record.to_string())),
                ("source_key", text(|mention| mention.key.clone())),
                ("payload", text(|mention| mention.payload.clone())),
                ("evidence", text(|mention| mention.evidence.clone())),
            ])?;
            Ok((group, staged))
        })
        .collect()
}
