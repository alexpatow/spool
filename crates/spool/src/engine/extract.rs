//! Documents and extraction: storing text, running an extractor over it,
//! and storing the mentions it finds as records.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use duckdb::params;
use duckdb::vtab::arrow::ArrowBatchRegistration;
use serde_json::json;
use spool_core::{DocumentId, EntityType, RunId};

use super::{Engine, Inner, lock};
use crate::batch::{Column, batch};
use crate::documents::{
    self, DocumentEntry, DocumentFilter, ExtractOptions, ExtractReport, ExtractorRef, Mention,
    NewDocument, RawMention, TaskDocument,
};
use crate::error::{Error, Result};
use crate::features;
use crate::ingest::{self, IngestReport};
use crate::project::{Declaration, Extractor};
use crate::runs::RunStatus;
use crate::store::parse;
use crate::usercode::{self, ExtractType, Task, UserCode};

/// The documents of the extraction in progress. One extraction runs at a
/// time, so one table is enough.
const TASK: &str = "spool_extract_task";

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
    pub fn add_documents(&self, source: &str, documents: &[NewDocument]) -> Result<IngestReport> {
        self.lock().add_documents(source, documents)
    }

    /// A document with its text and the extractors that have read it.
    pub fn document(&self, id: DocumentId) -> Result<DocumentEntry> {
        self.read(|conn| documents::get(conn, id))?
            .ok_or(Error::UnknownDocument(id))
    }

    /// Documents without their text, ordered by ID.
    pub fn documents(&self, filter: &DocumentFilter) -> Result<Vec<DocumentEntry>> {
        self.read(|conn| documents::list(conn, filter))
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
    /// rows that were ingested.
    ///
    /// The extractor is user code and runs as a process of its own. While
    /// it runs the engine is free. An extractor that fails, does not finish
    /// in time, or writes a mention the engine refuses fails its run, and
    /// nothing it found is stored.
    pub fn extract(&self, options: &ExtractOptions) -> Result<ExtractReport> {
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
            None,
            &json!({
                "extractor": ExtractorRef::from(&extractor),
                "source": options.source,
                "again": options.again,
                "limit": options.limit,
            }),
        )?;
        let task = inner.in_run(run, |inner| {
            inner.write_extract_task(run, &extractor, &user_code, options)
        })?;
        let Some(mut task) = task else {
            // There is nothing to read, so nothing is started.
            let report = ExtractReport::empty(run);
            inner.finish_run(run, RunStatus::Succeeded, Some(&json!(report)))?;
            return Ok(report);
        };

        // The extractor runs with the writer let go. The run stays
        // `running`, and it reads the documents as they were written.
        drop(inner);
        let executed = task.execute();
        let mut inner = self.lock();
        let report = inner.in_run(run, |inner| {
            executed?;
            inner
                .take_mentions(run, &task.path(usercode::MENTIONS), &extractor)
                .map_err(|error| match error {
                    Error::Mentions(_) => task.rejected(error),
                    other => other,
                })
        })?;
        task.remove();
        Ok(report)
    }
}

/// A SQL string literal.
fn sql_text(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

impl Inner {
    fn add_documents(&mut self, source: &str, documents: &[NewDocument]) -> Result<IngestReport> {
        let staged = documents::stage(source, documents)?;
        let received = documents.len() as u64;
        let run = self.start_run(
            "ingest",
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
                 WHERE ingest_run = ? AND supersedes IS NOT NULL",
                [run.0 as i64],
                |row| row.get(0),
            )?;
            let report = IngestReport {
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
    /// extractor finds them. `None` when there is nothing to read.
    fn write_extract_task(
        &mut self,
        run: RunId,
        extractor: &Extractor,
        user_code: &UserCode,
        options: &ExtractOptions,
    ) -> Result<Option<Task>> {
        let manifest = self.manifest.clone().ok_or(Error::NoManifest)?;
        let conn = &self.store.conn;
        let of_source = match &options.source {
            Some(source) => format!("AND d.source_id = {}", sql_text(source)),
            None => String::new(),
        };
        let unread = match options.again {
            true => String::new(),
            false => format!(
                "AND NOT EXISTS (
                   SELECT 1 FROM spool.extraction e
                   WHERE e.document_id = d.document_id AND e.extractor = {})",
                sql_text(&extractor.name)
            ),
        };
        let limit = match options.limit {
            Some(limit) => format!("LIMIT {limit}"),
            None => String::new(),
        };
        conn.execute_batch(&format!(
            "CREATE OR REPLACE TEMP TABLE {TASK} AS
             SELECT d.document_id,
                    row_number() OVER (ORDER BY d.ingest_run, d.source_id, d.source_key) AS ord
             FROM spool.document d
             WHERE NOT EXISTS (
                 SELECT 1 FROM spool.document later WHERE later.supersedes = d.document_id)
               {of_source}
               {unread}
             ORDER BY d.ingest_run, d.source_id, d.source_key
             {limit}"
        ))?;
        let chosen: i64 = conn.query_row(&format!("SELECT count(*) FROM {TASK}"), [], |row| {
            row.get(0)
        })?;
        if chosen == 0 {
            return Ok(None);
        }

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
        Ok(Some(task))
    }

    /// Reads the mentions an extractor wrote, checks them against the
    /// documents it was given, and stores them as records.
    fn take_mentions(
        &mut self,
        run: RunId,
        mentions: &Path,
        extractor: &Extractor,
    ) -> Result<ExtractReport> {
        let manifest = self.manifest.clone().ok_or(Error::NoManifest)?;
        let given = self.task_documents()?;
        let raw = self.read_mentions(mentions)?;
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
        for mention in &staged {
            *found.entry(mention.document).or_default() += 1;
        }

        // The records are stored under a run of their own, which
        // begins now. A match run that began while the extractor worked
        // has a later ID than the extract run, and takes as new only what
        // was ingested before it: under the extract run's ID these
        // records would be new to no run at all.
        let reference = ExtractorRef::from(extractor);
        let ingest_run = match staged.is_empty() {
            true => None,
            false => Some(self.start_run(
                "ingest",
                None,
                &json!({ "extract_run": run, "extractor": reference }),
            )?),
        };
        let store = |inner: &mut Self| -> Result<(ExtractReport, Option<IngestReport>)> {
            let now = inner.store.clock.now_micros();
            let tx = inner.store.conn.transaction()?;
            let (mut inserted, mut restored) = (0, 0);
            if let Some(ingest_run) = ingest_run {
                for ((entity_type, source), staged) in grouped(&staged)? {
                    let stored = ingest::insert(&tx, entity_type, source, now, ingest_run, staged)?;
                    inserted += stored.inserted;
                    restored += stored.restored;
                }
                for (name, declaration, version) in declared.values() {
                    let problem = features::fill(
                        &tx,
                        name,
                        declaration,
                        *version,
                        &features::of_run(name, ingest_run),
                    )?;
                    if let Some(problem) = problem {
                        return Err(Error::Mentions(problem));
                    }
                }
            }

            let (ids, counts): (Vec<_>, Vec<_>) = found
                .iter()
                .map(|(id, count)| (Some(id.to_string()), *count))
                .unzip();
            tx.execute(
                "INSERT INTO spool.extraction
                 SELECT s.document_id::UUID, $1, $2, $3, s.mentions FROM arrow($4) s",
                params![
                    extractor.name,
                    extractor.hash,
                    run.0 as i64,
                    &ArrowBatchRegistration::new(batch(vec![
                        ("document_id", Column::Text(ids)),
                        ("mentions", Column::Integer(counts)),
                    ])?)
                ],
            )?;

            let superseded: i64 = match ingest_run {
                Some(ingest_run) => tx.query_row(
                    "SELECT count(*) FROM spool.record
                     WHERE ingest_run = ? AND supersedes IS NOT NULL",
                    [ingest_run.0 as i64],
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
                ingest_run,
            };
            let finished = inner.store.clock.now_micros();
            let finish = "UPDATE spool.run
                 SET status = 'succeeded', finished_at = make_timestamptz(?), result = ?::JSON
                 WHERE run_id = ?";
            let stored = ingest_run.map(|ingest_run| IngestReport {
                run: ingest_run,
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
        let (report, stored) = match ingest_run {
            Some(ingest_run) => self.in_run(ingest_run, store),
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

/// The mentions of each entity type and source, as the batch an ingest
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
