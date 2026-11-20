//! Documents and extraction: storing text and running an extractor over
//! it. What the extractor finds is stored by `mentions`.

use std::collections::HashSet;
use std::sync::{Mutex, MutexGuard};

use duckdb::params;
use serde_json::json;
use spool_core::{Actor, DocumentId, EntityType, RecordId, RunId};

use super::{Engine, Inner, Reading, lock_ignoring_poison};
use crate::documents::{
    self, AddDocumentReport, DocumentEntry, DocumentFilter, DocumentReading, ExtractOptions,
    ExtractReport, ExtractorRef, NewDocument,
};
use crate::error::{Error, Result};
use crate::load::LoadReport;
use crate::project::Extractor;
use crate::retract::NOT_RETRACTED;
use crate::runs::RunStatus;
use crate::store::parse_stored;
use crate::usercode::{self, ExtractType, Task, UserCode};

/// The documents of the extraction in progress. One extraction runs at a
/// time, so one table is enough.
pub(super) const TASK: &str = "spool_extract_task";

impl Engine {
    /// Stores documents from one source. A document that is already known,
    /// by its source, key and text, creates nothing. A new text under a key
    /// the source has used before is the next version of that document.
    ///
    /// Nothing is extracted yet: [`extract`](Self::extract) does that.
    pub fn add_documents(&self, source: &str, documents: &[NewDocument]) -> Result<LoadReport> {
        self.acting(Actor::Engine).add_documents(source, documents)
    }

    /// Stores one document, reads it with a project extractor, and matches
    /// each mention with the rules for its entity type. The project's only
    /// extractor can be left unnamed.
    pub fn add_document(
        &self,
        source: &str,
        document: &NewDocument,
        extractor: Option<&str>,
    ) -> Result<AddDocumentReport> {
        self.acting(Actor::Engine)
            .add_document(source, document, extractor)
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
        let reading = lock_ignoring_poison(&self.reading);
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
        self.acting(Actor::Engine).extract(options)
    }

    /// Takes the writer back after user code runs. A combined call can
    /// place the mentions before another writer can change them.
    pub(super) fn extract_locked<'a>(
        &'a self,
        mut inner: MutexGuard<'a, Inner>,
        by: &Actor,
        options: &ExtractOptions,
        extractor: &Extractor,
        document: Option<DocumentId>,
    ) -> Result<(MutexGuard<'a, Inner>, ExtractReport)> {
        let user_code = self.user_code().ok_or_else(|| Error::Extractor {
            extractor: extractor.name.clone(),
            message: "it is user code, and this engine was not told where the project directory is"
                .to_owned(),
        })?;
        // A file that is not the one the manifest was compiled from is
        // never run, and neither is one that nothing here can run.
        user_code.check(&extractor.into())?;

        let mut config = json!({
            "extractor": ExtractorRef::from(extractor),
            "source": options.source,
            "again": options.again,
            "since": options.since,
            "limit": options.limit,
        });
        if let Some(document) = document {
            config["document"] = json!(document);
        }
        let run = inner.start_run("extract", by, None, &config)?;
        let task = inner.in_run(run, |inner| {
            inner.write_extract_task(run, extractor, &user_code, options, document)
        })?;
        let Some((mut task, left)) = task else {
            // There is nothing to read, so nothing is started.
            let report = ExtractReport::empty(run);
            inner.finish_run(run, RunStatus::Succeeded, Some(&json!(report)))?;
            return Ok((inner, report));
        };

        let given = inner.task_document_ids()?;
        *lock_ignoring_poison(&self.reading) = Some(Reading {
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
                .take_mentions(by, run, &task, extractor, left)
                .map_err(|error| match error {
                    Error::Mentions(_) => task.rejected(error),
                    other => other,
                })
        })?;
        task.remove();
        let mut report = report;
        if report.documents > report.failed.len() as u64 {
            let (taken_back, runs) = inner.take_back(by, run, extractor)?;
            if taken_back > 0 || !runs.is_empty() {
                report.taken_back = taken_back;
                report.retract_runs = runs;
                inner.store.conn.execute(
                    "UPDATE spool.run SET result = ?::JSON WHERE run_id = ?",
                    params![json!(report).to_string(), run.0 as i64],
                )?;
            }
        }
        Ok((inner, report))
    }
}

pub(super) fn choose_extractor(
    manifest: &crate::project::Manifest,
    name: Option<&str>,
) -> Result<Extractor> {
    let extractors = &manifest.extractors;
    let names = || {
        extractors
            .iter()
            .map(|extractor| format!("`{}`", extractor.name))
            .collect::<Vec<_>>()
            .join(", ")
    };
    match name {
        Some(name) => manifest
            .extractor(name)
            .cloned()
            .ok_or_else(|| Error::UnknownExtractor {
                extractor: name.to_owned(),
                known: extractors
                    .iter()
                    .map(|extractor| extractor.name.clone())
                    .collect(),
            }),
        None => match extractors.as_slice() {
            [only] => Ok(only.clone()),
            [] => Err(Error::Documents(
                "the project has no extractors: add one under extractors/".to_owned(),
            )),
            _ => Err(Error::Documents(format!(
                "the project has the extractors {}: say which in `extractor`",
                names()
            ))),
        },
    }
}

/// Forgets what the extraction in progress reads, and whether it was
/// cancelled, when it ends, however it ends.
struct ReadingEnds<'a>(&'a Mutex<Option<Reading>>, &'a usercode::Running, RunId);

impl Drop for ReadingEnds<'_> {
    fn drop(&mut self) {
        *lock_ignoring_poison(self.0) = None;
        self.1.forget(self.2);
    }
}

/// A SQL string literal.
fn sql_text(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

type ExtractedRecord = (RecordId, EntityType, u64, u64);

impl Inner {
    /// Reuse a successful reading by the same extractor version if all of
    /// its mentions are still present. Format 11 readings lack these links.
    pub(super) fn document_reading(
        &self,
        document: DocumentId,
        extractor: &Extractor,
    ) -> Result<Option<Vec<ExtractedRecord>>> {
        let mut statement = self.store.conn.prepare(
            "SELECT run_id, extractor_version, mentions FROM spool.extraction
             WHERE document_id = ?::UUID AND extractor = ? AND failure IS NULL
             ORDER BY run_id DESC LIMIT 1",
        )?;
        let mut rows = statement.query(params![document.to_string(), extractor.name])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        let version: String = row.get(1)?;
        if version != extractor.hash {
            return Ok(None);
        }
        let run = RunId(row.get::<_, i64>(0)? as u64);
        let count: i64 = row.get(2)?;
        drop(rows);
        drop(statement);
        let mentions = self.document_mentions(document, run)?;
        Ok((mentions.len() as i64 == count).then_some(mentions))
    }

    pub(super) fn document_mentions(
        &self,
        document: DocumentId,
        run: RunId,
    ) -> Result<Vec<ExtractedRecord>> {
        let mut statement = self.store.conn.prepare(&format!(
            "SELECT r.record_id::VARCHAR, r.entity_type,
                    (r.evidence->>'start')::UBIGINT AS start_at,
                    (r.evidence->>'end')::UBIGINT AS end_at
             FROM spool.extraction_found f
             JOIN spool.record r ON r.record_id = f.record_id
             WHERE f.document_id = ?::UUID AND f.run_id = ? AND r.{NOT_RETRACTED}
               AND NOT EXISTS (SELECT 1 FROM spool.record later WHERE later.supersedes = r.record_id)
             ORDER BY start_at, end_at, r.entity_type"
        ))?;
        let mut rows = statement.query(params![document.to_string(), run.0 as i64])?;
        let mut mentions = Vec::new();
        while let Some(row) = rows.next()? {
            mentions.push((
                parse_stored(&row.get::<_, String>(0)?)?,
                EntityType::new(row.get::<_, String>(1)?)?,
                row.get(2)?,
                row.get(3)?,
            ));
        }
        Ok(mentions)
    }

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
        document: Option<DocumentId>,
    ) -> Result<Option<(Task, u64)>> {
        let manifest = self.manifest.clone().ok_or(Error::NoManifest)?;
        let conn = &self.store.conn;
        let of_source = match &options.source {
            Some(source) => format!("AND d.source_id = {}", sql_text(source)),
            None => String::new(),
        };
        let extractor_is = format!("e.extractor = {}", sql_text(&extractor.name));
        // Read: a reading that did not fail, by whatever version.
        let read = match options.again || document.is_some() {
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
        let scope = match document {
            Some(document) => format!("d.document_id = '{document}'::UUID"),
            None => "NOT EXISTS (
                 SELECT 1 FROM spool.document later WHERE later.supersedes = d.document_id)"
                .to_owned(),
        };
        conn.execute_batch(&format!(
            "CREATE OR REPLACE TEMP TABLE {TASK} AS
             SELECT d.document_id,
                    row_number() OVER (ORDER BY d.load_run, d.source_id, d.source_key) AS ord,
                    count(*) OVER () AS matching
             FROM spool.document d
             WHERE {scope}
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

    /// The IDs of the documents the extraction in progress was given.
    fn task_document_ids(&self) -> Result<HashSet<DocumentId>> {
        let mut statement = self
            .store
            .conn
            .prepare(&format!("SELECT document_id::VARCHAR FROM {TASK}"))?;
        let mut rows = statement.query([])?;
        let mut ids = HashSet::new();
        while let Some(row) = rows.next()? {
            ids.insert(parse_stored(&row.get::<_, String>(0)?)?);
        }
        Ok(ids)
    }
}
