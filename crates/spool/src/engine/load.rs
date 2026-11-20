//! Loading: storing rows as records, placing one record as it is added,
//! and taking rows back.

use std::collections::{BTreeSet, HashSet};

use duckdb::arrow::record_batch::RecordBatchReader;
use duckdb::params;
use duckdb::vtab::arrow::ArrowBatchRegistration;
use serde::{Deserialize, Serialize};
use serde_json::json;
use spool_core::{
    Actor, Changeset, EntityType, IdentityId, IdentityState, IdentityStore, Op, Policy, RecordId,
    RunId, ValidationError, validate,
};

use super::{Disposition, Engine, Inner, Scope, StrategyRef, lock_ignoring_poison};
use crate::error::{Error, Result};
use crate::features;
use crate::load::{self, LoadReport};
use crate::possible_split;
use crate::retract::{self, RetractReport};
use crate::review::{self, ReviewEntry, ReviewFilter, ReviewKind, ReviewStatus};
use crate::rows::Rows;
use crate::runs::RunStatus;
use crate::store;

/// The result of storing one row and placing its record, or leaving it for review.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AddRecordReport {
    pub record: RecordId,
    pub identity: Option<IdentityId>,
    pub review: Option<ReviewEntry>,
    pub load_run: RunId,
    /// A retry of a placed record or one already waiting for review starts no match.
    pub match_run: Option<RunId>,
}

pub(super) struct RecordResolution {
    pub(super) identity: Option<IdentityId>,
    pub(super) review: Option<ReviewEntry>,
    match_run: Option<RunId>,
}

impl Engine {
    /// Stores rows from one source as records of one entity type and
    /// computes their feature rows. A row that is already known, by its
    /// source, key and content, creates nothing.
    pub fn load(
        &self,
        source: &str,
        entity_type: &str,
        rows: impl RecordBatchReader,
    ) -> Result<LoadReport> {
        self.acting(Actor::Engine).load(source, entity_type, rows)
    }

    /// As [`Engine::load`], for a caller that means to add rows and not to
    /// change any. A row under a key the store holds with other content
    /// would become the next version of that row. Here it is refused, and
    /// the whole call with it: nothing is stored, and the error names the
    /// keys. Content the key held before, and not now, is other content:
    /// a row that would change back is refused too. A row the store holds
    /// as it is creates nothing, as in any load, and so does one a source
    /// took back and sends again unchanged.
    pub fn load_without_replacing(
        &self,
        source: &str,
        entity_type: &str,
        rows: impl RecordBatchReader,
    ) -> Result<LoadReport> {
        self.acting(Actor::Engine)
            .load_without_replacing(source, entity_type, rows)
    }

    /// Stores a row and matches its record against existing identities with
    /// the project's SQL rules. An ambiguous record stays unassigned.
    pub fn add_record(
        &self,
        source: &str,
        entity_type: &EntityType,
        row: &serde_json::Map<String, serde_json::Value>,
        replace: bool,
    ) -> Result<AddRecordReport> {
        self.acting(Actor::Engine)
            .add_record(source, entity_type, row, replace)
    }

    /// Takes back rows a source no longer holds, named by their keys. Every
    /// version of each row stays in the store as evidence and leaves its
    /// identity, by one changeset under `actor`'s name. From then on no run
    /// examines the records, and a changeset that would give one an
    /// identity is refused, until the source sends the row again: an
    /// load of the key, with the same content or other, brings every
    /// version back to the identity it left.
    ///
    /// A key the source has no row under is counted and changes nothing, so
    /// the same retraction can be sent twice. A row without a key cannot be
    /// taken back.
    pub fn retract(
        &self,
        source: &str,
        entity_type: &str,
        keys: &[String],
        actor: Actor,
        reason: Option<String>,
    ) -> Result<RetractReport> {
        self.lock()
            .retract(source, entity_type, keys, actor, reason)
    }
}

impl Inner {
    pub(super) fn load(
        &mut self,
        by: &Actor,
        source: &str,
        entity_type: &str,
        rows: impl RecordBatchReader,
        replace: bool,
    ) -> Result<LoadReport> {
        self.load_rows(by, source, entity_type, rows, replace, false)
            .map(|(report, _)| report)
    }

    fn load_rows(
        &mut self,
        by: &Actor,
        source: &str,
        entity_type: &str,
        rows: impl RecordBatchReader,
        replace: bool,
        single: bool,
    ) -> Result<(LoadReport, Option<RecordId>)> {
        if source.is_empty() {
            return Err(Error::Load("the source needs a name".to_owned()));
        }
        let (manifest, entry, version) = self.declared(entity_type)?;
        let name = entry.name.clone();
        let declaration = entry.declaration.clone();
        let source = &manifest
            .source_of(source, entry)
            .map_err(Error::Load)?
            .clone();

        let run = self.start_run(
            "load",
            by,
            None,
            &json!({ "source": source.name, "entity_type": name }),
        )?;
        let (report, record) = self.in_run(run, |inner| {
            let now = inner.store.clock.now_micros();
            let tx = inner.store.conn.transaction()?;
            let mut seen = HashSet::new();
            let (mut received, mut inserted, mut restored) = (0u64, 0u64, 0u64);
            let mut record = None;

            for incoming in rows {
                let incoming = incoming?;
                let staged = load::stage(source, &incoming, &mut seen, received as usize)?;
                received += incoming.num_rows() as u64;
                let Some(staged) = staged else { continue };

                // The ID does not cover the entity type, so the same row sent
                // as another type would otherwise be dropped without a word.
                let clash: Option<String> = {
                    let mut statement = tx.prepare(
                        "SELECT o.entity_type FROM arrow(?) s
                         JOIN spool.record o ON o.record_id = s.record_id::UUID
                         WHERE o.entity_type <> ? LIMIT 1",
                    )?;
                    let mut found = statement.query(params![
                        &ArrowBatchRegistration::new(staged.clone()),
                        name.as_str()
                    ])?;
                    match found.next()? {
                        Some(row) => Some(row.get(0)?),
                        None => None,
                    }
                };
                if let Some(other) = clash {
                    return Err(Error::Load(format!(
                        "a row with the same source, key and content is already stored as entity type `{other}`"
                    )));
                }

                let submitted = single.then(|| staged.clone());
                let stored = load::insert(&tx, name.as_str(), &source.name, now, run, staged)?;
                inserted += stored.inserted;
                restored += stored.restored;
                if let Some(submitted) = submitted {
                    record = Some(load::stored_record(&tx, &source.name, name.as_str(), &submitted)?);
                }
            }
            if !replace && let Some(refusal) = load::replacing(&tx, run)? {
                return Err(Error::Load(refusal));
            }

            let problem = features::fill(
                &tx,
                &name,
                &declaration,
                version,
                &features::of_run(&name, run),
            )?;
            if let Some(problem) = problem {
                return Err(Error::Load(problem));
            }
            let superseded: i64 = tx.query_row(
                "SELECT count(*) FROM spool.record
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
                restored,
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
            Ok((report, record))
        })?;
        let report = self.give_back(
            report,
            format!("source `{}` sent the rows again", source.name),
        )?;
        Ok((report, record))
    }

    pub(super) fn add_record(
        &mut self,
        by: &Actor,
        source: &str,
        entity_type: &EntityType,
        rows: Rows,
        replace: bool,
    ) -> Result<AddRecordReport> {
        let (load, record) =
            self.load_rows(by, source, entity_type.as_str(), rows, replace, true)?;
        let record = record.ok_or_else(|| Error::Load("the load returned no record".to_owned()))?;
        let placed = self.place_record(by, entity_type, record)?;
        Ok(AddRecordReport {
            record,
            identity: placed.identity,
            review: placed.review,
            load_run: load.run,
            match_run: placed.match_run,
        })
    }

    pub(super) fn place_record(
        &mut self,
        by: &Actor,
        entity_type: &EntityType,
        record: RecordId,
    ) -> Result<RecordResolution> {
        let identity = store::read_assignments(&self.store.conn, &[record])?[0];
        let review = if identity.is_none() {
            self.record_review(record)?
        } else {
            None
        };
        if identity.is_some() || review.is_some() {
            return Ok(RecordResolution {
                identity,
                review,
                match_run: None,
            });
        }
        let run = self.begin_match(
            by,
            &StrategyRef::rules(),
            &Scope::new(entity_type.clone()),
            RunStatus::Running,
            Some(record),
        )?;
        self.in_run(run, |inner| {
            inner.judge_with_rules(run)?;
            inner.conclude(by, run, &Policy::default(), Disposition::Commit)
        })?;
        let identity = store::read_assignments(&self.store.conn, &[record])?[0];
        let review = if identity.is_none() {
            self.record_review(record)?
        } else {
            None
        };
        Ok(RecordResolution {
            identity,
            review,
            match_run: Some(run),
        })
    }

    fn record_review(&self, record: RecordId) -> Result<Option<ReviewEntry>> {
        let items = review::list(
            &self.store.conn,
            &ReviewFilter {
                record: Some(record),
                ..ReviewFilter::default()
            },
        )?;
        Ok(items.into_iter().rev().find(|item| {
            item.kind != ReviewKind::PossibleSplit
                && matches!(item.status, ReviewStatus::Open | ReviewStatus::Dismissed)
        }))
    }

    /// Puts the records that came back in a load into the identity each
    /// of them left, by one changeset, and says so in the run's report.
    ///
    /// The load is already stored when this begins. Cut short in between,
    /// the store holds records that came back and have no identity, and
    /// the next match run takes them as new.
    pub(crate) fn give_back(
        &mut self,
        mut report: LoadReport,
        reason: String,
    ) -> Result<LoadReport> {
        if report.restored == 0 {
            return Ok(report);
        }
        let mut ops = retract::returning(&self.store.conn, report.run)?;
        // A record cannot go back where a constraint that was made
        // meanwhile forbids it. It is left out, and the rest go back.
        let committed = loop {
            if ops.is_empty() {
                return Ok(report);
            }
            let changeset = Changeset {
                id: lock_ignoring_poison(&self.ids).changeset_id(),
                base: self.store.head()?,
                actor: Actor::Engine,
                reason: Some(reason.clone()),
                run: None,
                ops: ops.clone(),
            };
            match validate(&self.store, changeset) {
                Ok(validated) => break self.store.apply(validated)?,
                Err(ValidationError::Rejected(rejections)) => {
                    let refused: HashSet<usize> = rejections
                        .iter()
                        .filter_map(|rejection| rejection.op_index)
                        .collect();
                    if refused.is_empty() {
                        return Ok(report);
                    }
                    let mut at = 0;
                    ops.retain(|_| {
                        at += 1;
                        !refused.contains(&(at - 1))
                    });
                }
                Err(error) => return Err(error.into()),
            }
        };
        report.returned = ops.len() as u64;
        report.committed = Some(committed);
        // A record is back where it was, and what held it there may still
        // be gone. That is put to a reviewer, as when a record leaves.
        let returned_to: Vec<IdentityId> = ops
            .iter()
            .filter_map(|op| match op {
                Op::Assign {
                    to: Some(identity), ..
                } => Some(*identity),
                _ => None,
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let tx = self.store.conn.transaction()?;
        report.possible_splits = possible_split::raise(&tx, report.run, &returned_to)?;
        tx.execute(
            "UPDATE spool.run SET result = ?::JSON WHERE run_id = ?",
            params![json!(report).to_string(), report.run.0 as i64],
        )?;
        tx.commit()?;
        Ok(report)
    }

    pub(super) fn retract(
        &mut self,
        source: &str,
        entity_type: &str,
        keys: &[String],
        actor: Actor,
        reason: Option<String>,
    ) -> Result<RetractReport> {
        if keys.is_empty() {
            return Err(Error::Retract("no keys were named".to_owned()));
        }
        let (manifest, entry, _) = self.declared(entity_type)?;
        let name = entry.name.clone();
        let source = manifest
            .source_of(source, entry)
            .map_err(Error::Retract)?
            .name
            .clone();

        let run = self.start_run(
            "retract",
            &actor,
            None,
            &json!({ "source": source, "entity_type": name, "reason": reason }),
        )?;
        self.in_run(run, |inner| {
            let named = retract::name(&inner.store.conn, &name, &source, keys)?;

            // First the records leave their identities, and only then are
            // they recorded as taken back. Cut short in between, the store
            // holds records without an identity, which is a state it
            // can be in, and the same retraction sent again finishes the
            // work. What it then no longer knows is the identity they left,
            // so when they come back they are new to the next match run.
            let members = retract::members(&inner.store.conn)?;
            let detached = members.len() as u64;
            let left: Vec<IdentityId> = members
                .iter()
                .map(|(_, identity)| *identity)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let committed = if members.is_empty() {
                None
            } else {
                let changeset = Changeset {
                    id: lock_ignoring_poison(&inner.ids).changeset_id(),
                    base: inner.store.head()?,
                    actor,
                    reason: reason
                        .or_else(|| Some(format!("source `{source}` took the rows back"))),
                    // A changeset names the reconcile run that drafted it,
                    // and none drafted this one. The run names the
                    // changeset instead.
                    run: None,
                    ops: members
                        .into_iter()
                        .map(|(record, identity)| Op::Assign {
                            record,
                            from: Some(identity),
                            to: None,
                        })
                        .collect(),
                };
                let validated = validate(&inner.store, changeset)?;
                Some(inner.store.apply(validated)?)
            };

            let finished = inner.store.clock.now_micros();
            let tx = inner.store.conn.transaction()?;
            let retracted = retract::record(
                &tx,
                &name,
                run,
                committed.as_ref().map(|committed| committed.seq),
            )?;
            // What is left of an identity may have been held together only
            // through a record that is gone now. That is put to a reviewer.
            let possible_splits = possible_split::raise(&tx, run, &left)?;
            let report = RetractReport {
                run,
                received: named.received,
                unknown: named.unknown,
                retracted,
                detached,
                committed,
                possible_splits,
            };
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
}
