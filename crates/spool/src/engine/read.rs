//! Reading: runs, review items, identities, records, explanations, the
//! log, and the measures of a store.

use duckdb::Connection;
use duckdb::arrow::record_batch::{RecordBatch, RecordBatchReader};
use duckdb::vtab::arrow::ArrowBatchRegistration;
use spool_core::{Actor, IdentityId, IdentityStatus, LogEntry, Op, RecordId, RunId, Seq, Snapshot};

use super::Engine;
use crate::attributes::{AttributeFilter, AttributeGroup};
use crate::error::{Error, Result};
use crate::explain::{self, Explanation, IdentityExplanation};
use crate::lookup::{
    self, ChangesetSummary, HistoryEntry, IdentityFilter, IdentitySnapshot, IdentitySummary,
    IdentityView, LogSort, LoggedChangeset, RecordEntry, RecordFilter, StoreInfo,
};
use crate::metrics::{self, Churn, IdStability, Metrics};
use crate::order::Sort;
use crate::project::Manifest;
use crate::review::{self, ReviewCase, ReviewEntry, ReviewFilter};
use crate::runs::{self, RunEntry, RunFilter, RunStatus};
use crate::schema;
use crate::store;

/// How often an explanation begins again when the declaration changed
/// under it, and how long it waits in between.
const EXPLAIN_ATTEMPTS: usize = 20;
const EXPLAIN_PAUSE: std::time::Duration = std::time::Duration::from_millis(5);

impl Engine {
    /// A run as the store records it: the task record of whatever started
    /// it.
    pub fn run(&self, run: RunId) -> Result<RunEntry> {
        let mut entry = self
            .read(|conn| runs::get(conn, run))?
            .ok_or(Error::UnknownRun(run))?;
        if entry.status == RunStatus::Running {
            entry.said = self.said(run);
        }
        Ok(entry)
    }

    /// Runs, newest first.
    pub fn runs(&self, filter: &RunFilter) -> Result<Vec<RunEntry>> {
        self.read(|conn| runs::list(conn, filter))
    }

    /// Review items, oldest first.
    pub fn review_items(&self, filter: &ReviewFilter) -> Result<Vec<ReviewEntry>> {
        self.read(|conn| review::list(conn, filter))
    }

    /// A review item with what a reviewer reads to decide it: the records
    /// it holds back, the identities they matched with their members, and
    /// what was judged between them.
    pub fn review_case(&self, id: u64) -> Result<ReviewCase> {
        self.read(|conn| review::case(conn, id))
    }

    pub fn identity_of(&self, record: RecordId) -> Result<Option<IdentityId>> {
        Ok(self.identities_of(&[record])?[0])
    }

    /// The identity of each record, in the order asked. `None` for an
    /// record that has no identity or that the store does not hold.
    pub fn identities_of(&self, records: &[RecordId]) -> Result<Vec<Option<IdentityId>>> {
        self.read(|conn| store::read_assignments(conn, records))
    }

    /// The active identity an ID stands for: itself, or the identity it was
    /// merged into.
    pub fn current_id(&self, id: IdentityId) -> Result<IdentityId> {
        match self
            .read(|conn| store::read_identities(conn, &[id]))?
            .remove(0)
        {
            None => Err(Error::UnknownIdentity(id)),
            Some(record) => Ok(match record.status {
                IdentityStatus::Active => id,
                IdentityStatus::MergedInto(survivor) => survivor,
            }),
        }
    }

    /// An identity with the records that belong to it now.
    pub fn identity(&self, id: IdentityId) -> Result<IdentityView> {
        self.read(|conn| lookup::identity(conn, id))?
            .ok_or(Error::UnknownIdentity(id))
    }

    /// The identity and its members immediately after changeset `seq`.
    pub fn identity_at(&self, id: IdentityId, seq: Seq) -> Result<IdentitySnapshot> {
        self.read(|conn| lookup::identity_at(conn, id, seq))
    }

    /// Active identities, ordered by ID.
    pub fn identities(&self, filter: &IdentityFilter) -> Result<Vec<IdentitySummary>> {
        self.read(|conn| lookup::identities(conn, filter))
    }

    /// The changesets that name an identity, oldest first.
    pub fn history(&self, id: IdentityId) -> Result<Vec<HistoryEntry>> {
        self.read(|conn| {
            if store::read_identities(conn, &[id])?.remove(0).is_none() {
                return Err(Error::UnknownIdentity(id));
            }
            lookup::history(conn, id)
        })
    }

    /// A record as it was loaded, or `None` if the store does not
    /// hold it.
    pub fn record(&self, id: RecordId) -> Result<Option<RecordEntry>> {
        self.read(|conn| lookup::record(conn, id))
    }

    /// Records, ordered by source and key. A caller that knows a row by its
    /// source and key finds its record IDs here: only the engine computes
    /// them.
    pub fn records(&self, filter: &RecordFilter) -> Result<Vec<RecordEntry>> {
        self.read(|conn| lookup::records(conn, filter))
    }

    /// What the attributes of the records that stand hold, for each
    /// entity type and source the filter takes: see [`AttributeGroup`].
    pub fn attributes(&self, filter: &AttributeFilter) -> Result<Vec<AttributeGroup>> {
        let manifest = self.manifest();
        self.read(|conn| crate::attributes::attributes(conn, manifest.as_deref(), filter))
    }

    /// Why two records share an identity, or do not: where each is,
    /// what every run that looked at the pair concluded, and what the
    /// project's rules say about it now.
    pub fn explain(&self, a: RecordId, b: RecordId) -> Result<Explanation> {
        self.with_rules(|conn, manifest| explain::explain_pair(conn, manifest, a, b))
    }

    /// What holds an identity together: how many of its pairs of records
    /// are linked directly and how many only through chains, and the
    /// fewest links whose removal would part it, where a wrong merge
    /// entered.
    pub fn explain_identity(&self, id: IdentityId) -> Result<IdentityExplanation> {
        self.with_rules(|conn, manifest| explain::explain_identity(conn, manifest, id))
    }

    /// A read that needs the rules the feature tables were built for. The
    /// manifest is read before the store is, and `apply` can commit
    /// another declaration in between, so the read checks that the store
    /// was applied with the manifest it was given, and begins again with
    /// the engine's present manifest if not. When the declaration keeps
    /// changing, the read is done without one.
    fn with_rules<T>(
        &self,
        read: impl Fn(&Connection, Option<&Manifest>) -> Result<T>,
    ) -> Result<T> {
        for _ in 0..EXPLAIN_ATTEMPTS {
            let manifest = self.manifest();
            let done = self.read(|conn| {
                if let Some(manifest) = &manifest {
                    let applied = schema::meta(conn, "manifest_hash")?;
                    if applied.as_deref() != Some(manifest.hash.as_str()) {
                        return Ok(None);
                    }
                }
                read(conn, manifest.as_deref()).map(Some)
            })?;
            if let Some(done) = done {
                return Ok(done);
            }
            std::thread::sleep(EXPLAIN_PAUSE);
        }
        self.read(|conn| read(conn, None))
    }

    /// Runs one `SELECT`. The stable things to read are the views
    /// `spool.identity_current`, `spool.membership_current` and
    /// `spool.entity_<type>`. UUID columns come back as strings.
    ///
    /// The statement is run as a subquery, so anything that is not a query
    /// is a syntax error. It can still read files the process can read, so
    /// this is not for SQL from untrusted callers.
    pub fn query(&self, sql: &str) -> Result<Vec<RecordBatch>> {
        let sql = sql.trim().trim_end_matches(';');
        if sql.is_empty() {
            return Err(Error::Query("the query is empty".to_owned()));
        }
        self.read(|conn| {
            let mut statement = conn
                .prepare(&format!("SELECT * FROM (\n{sql}\n)"))
                .map_err(|error| Error::Query(error.to_string()))?;
            Ok(statement.query_arrow([])?.collect())
        })
    }

    /// The position of the last committed changeset.
    pub fn head(&self) -> Result<Seq> {
        self.read(store::read_head)
    }

    /// Every committed changeset, in log order.
    pub fn log(&self) -> Result<Vec<LogEntry>> {
        self.read(store::read_log)
    }

    /// At most `limit` committed changesets after a log position, in log
    /// order.
    pub fn log_after(&self, after: Seq, limit: u64) -> Result<Vec<LogEntry>> {
        self.read(|conn| store::read_log_after(conn, after, limit))
    }

    /// The same changesets as [`log_after`](Self::log_after), each with who
    /// started the run it came from. The log is what was changed and who
    /// decided it; this adds who asked.
    pub fn logged_after(&self, after: Seq, limit: u64) -> Result<Vec<LoggedChangeset>> {
        self.read(|conn| {
            lookup::logged_changesets(conn, store::read_log_after(conn, after, limit)?)
        })
    }

    /// The log head and up to `limit` summaries of changesets after
    /// `after`, ordered by position through that head.
    pub fn changeset_summaries(
        &self,
        after: Seq,
        limit: u64,
    ) -> Result<(Seq, Vec<ChangesetSummary>)> {
        self.read(|conn| {
            let head = store::read_head(conn)?;
            Ok((head, lookup::changeset_summaries(conn, after, head, limit)?))
        })
    }

    /// At most `limit` operations starting at the zero-based `offset` in
    /// the committed changeset at `seq`.
    pub fn changeset_ops(&self, seq: Seq, offset: u64, limit: u64) -> Result<Vec<Op>> {
        self.read(|conn| store::read_log_ops(conn, seq, offset, limit))
    }

    /// At most `limit` committed changesets in another order than the
    /// log's, each with who started the run it came from. A page continues
    /// after the changeset at `after`, wherever that stands in the order.
    /// `names` says what people are called, for sorting by who decided.
    pub fn logged_sorted(
        &self,
        sort: Sort<LogSort>,
        after: Option<Seq>,
        limit: u64,
        names: &[(Actor, String)],
    ) -> Result<Vec<LoggedChangeset>> {
        self.read(|conn| {
            lookup::logged_changesets(conn, lookup::changesets(conn, sort, after, limit, names)?)
        })
    }

    /// All of identity state, read from the projection tables.
    pub fn snapshot(&self) -> Result<Snapshot> {
        self.read(store::read_snapshot)
    }

    /// What the store holds, per entity type.
    pub fn info(&self) -> Result<StoreInfo> {
        let manifest = self.manifest();
        self.read(|conn| lookup::info(conn, manifest.as_deref()))
    }

    /// Scores current state against a truth relation with the columns
    /// `record_id` and `true_entity`.
    pub fn score(&self, truth: impl RecordBatchReader) -> Result<Metrics> {
        let inner = self.lock();
        let conn = &inner.store.conn;
        conn.execute_batch(&format!(
            "CREATE OR REPLACE TEMP TABLE {} (record_id UUID, true_entity VARCHAR)",
            metrics::TRUTH
        ))?;
        for batch in truth {
            conn.execute(
                &format!(
                    "INSERT INTO {} SELECT record_id::UUID, true_entity::VARCHAR FROM arrow(?)",
                    metrics::TRUTH
                ),
                [&ArrowBatchRegistration::new(batch?)],
            )?;
        }
        metrics::score(conn)
    }

    /// Compares each record's identity at two log positions.
    pub fn id_stability(&self, before: Seq, after: Seq) -> Result<IdStability> {
        self.read(|conn| metrics::id_stability(conn, before, after))
    }

    /// What changed between two log positions, against the number of
    /// records that arrived in between.
    pub fn churn(&self, before: Seq, after: Seq, new_records: u64) -> Result<Churn> {
        self.read(|conn| metrics::churn(conn, before, after, new_records))
    }
}
