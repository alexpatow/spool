//! The log: the changesets that touched an identity, and the committed
//! changesets in the order a list asks for.

use std::collections::BTreeMap;

use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};
use spool_core::{Actor, ChangesetId, ContentHash, IdentityId, LogEntry, Op, RunId, Seq};

use super::clamp_limit;
use crate::error::Result;
use crate::order::{Key, Keyed, Sort, called, names_table};
use crate::store::{OpRow, log_entry, op_from_row, parse};
use crate::time::rfc3339;

/// A committed changeset as far as it concerns one identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct HistoryEntry {
    pub seq: Seq,
    pub changeset: ChangesetId,
    pub actor: Actor,
    pub reason: Option<String>,
    /// The reconcile run that drafted it, if one did.
    pub run: Option<RunId>,
    pub committed_at: String,
    /// The operations that name the identity, in the order they were applied.
    pub ops: Vec<Op>,
}

/// A committed changeset as the log holds it, with who asked for the work
/// it came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "schema",
    derive(schemars::JsonSchema),
    schemars(rename = "LogEntry")
)]
pub struct LoggedChangeset {
    #[serde(flatten)]
    pub entry: LogEntry,
    /// Who started the run that committed the changeset, if a run did: a
    /// match run, a retraction, or a load of rows that had been taken back.
    /// The changeset's `actor` is who decided its operations, which for a
    /// match run and for rows that come back is the engine.
    #[serde(default)]
    pub started_by: Option<Actor>,
}

/// A committed changeset's metadata and operation count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangesetSummary {
    pub seq: Seq,
    pub id: ChangesetId,
    pub base: Seq,
    pub actor: Actor,
    pub reason: Option<String>,
    pub run: Option<RunId>,
    pub content_hash: ContentHash,
    pub started_by: Option<Actor>,
    pub operation_count: u64,
}

/// The changesets that name an identity, oldest first, each with only the
/// operations that name it.
pub(crate) fn history(conn: &Connection, id: IdentityId) -> Result<Vec<HistoryEntry>> {
    let mut entries: Vec<HistoryEntry> = Vec::new();
    let mut statement = conn.prepare(
        "SELECT c.seq, c.changeset_id::VARCHAR, c.actor, c.reason, c.run_id,
                epoch_us(c.committed_at),
                o.kind, o.identity_id::VARCHAR, o.from_identity_id::VARCHAR,
                o.record_id::VARCHAR, o.other_record_id::VARCHAR,
                o.entity_type, o.constraint_kind
         FROM spool.changeset_op o
         JOIN spool.changeset c ON c.seq = o.seq
         WHERE o.identity_id = $1::UUID OR o.from_identity_id = $1::UUID
         ORDER BY o.seq, o.op_index",
    )?;
    let mut rows = statement.query([id.to_string()])?;
    while let Some(row) = rows.next()? {
        let seq = Seq(row.get::<_, i64>(0)? as u64);
        let op = op_from_row(OpRow {
            kind: row.get(6)?,
            identity: row.get(7)?,
            from_identity: row.get(8)?,
            record: row.get(9)?,
            other_record: row.get(10)?,
            entity_type: row.get(11)?,
            constraint_kind: row.get(12)?,
        })?;
        match entries.last_mut() {
            Some(entry) if entry.seq == seq => entry.ops.push(op),
            _ => entries.push(HistoryEntry {
                seq,
                changeset: parse(&row.get::<_, String>(1)?)?,
                actor: parse(&row.get::<_, String>(2)?)?,
                reason: row.get(3)?,
                run: row.get::<_, Option<i64>>(4)?.map(|run| RunId(run as u64)),
                committed_at: rfc3339(row.get(5)?),
                ops: vec![op],
            }),
        }
    }
    Ok(entries)
}

/// What the log can be listed by. Each ends with the log position.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogSort {
    /// The log position: the order of the log.
    Seq,
    /// Who decided it: a person by their name, whatever its case.
    Actor,
    /// How many operations the changeset has.
    Operations,
    /// The reconcile run that drafted it. Those that name none last.
    Run,
    /// Its reason, whatever the case. Those without one last.
    Reason,
}

impl LogSort {
    fn keys(self) -> Vec<Key> {
        let mut keys = match self {
            Self::Seq => Vec::new(),
            Self::Actor => Key::text("coalesce(called.name, c.actor)").into(),
            Self::Operations => vec![Key::of(
                "(SELECT count(*) FROM spool.changeset_op o WHERE o.seq = c.seq)",
            )],
            Self::Run => vec![
                Key::missing_last("c.run_id"),
                Key::of("coalesce(c.run_id, 0)"),
            ],
            Self::Reason => Key::optional_text("c.reason").into(),
        };
        keys.push(Key::of("c.seq"));
        keys
    }
}

/// At most `most` committed changesets in the order `sort` says, after
/// the one at `after` in that order. `names` says what people are called.
pub(crate) fn changesets(
    conn: &Connection,
    sort: Sort<LogSort>,
    after: Option<Seq>,
    most: u64,
    names: &[(Actor, String)],
) -> Result<Vec<LogEntry>> {
    let named = sort.by == LogSort::Actor;
    let keyed = Keyed {
        before: named.then(|| called("$3")),
        from: if named {
            "spool.changeset c LEFT JOIN called ON called.actor = c.actor"
        } else {
            "spool.changeset c"
        }
        .to_owned(),
        id: "c.seq".to_owned(),
        keys: sort.by.keys(),
        direction: sort.direction,
    };
    let mut statement = conn.prepare(&format!(
        "{} SELECT c.seq FROM spool.changeset c {}
         WHERE {}
         ORDER BY {}
         LIMIT $2",
        keyed.with("$1::BIGINT"),
        keyed.join("c.seq"),
        keyed.after("$1::BIGINT IS NOT NULL"),
        keyed.order_by(),
    ))?;
    let after = after.and_then(|seq| i64::try_from(seq.0).ok());
    let most = self::clamp_limit(most);
    let table = names_table(names)?;
    let table = &table;
    let mut values: Vec<&dyn duckdb::ToSql> = vec![&after, &most];
    if named {
        values.push(&table);
    }
    let mut rows = statement.query(values.as_slice())?;
    let mut order = Vec::new();
    while let Some(row) = rows.next()? {
        order.push(Seq(row.get::<_, i64>(0)? as u64));
    }
    // Positions that follow each other are read together, which is the
    // whole page when the log is listed by position.
    let mut sorted = order.clone();
    sorted.sort();
    let mut read: BTreeMap<Seq, LogEntry> = BTreeMap::new();
    let mut at = 0;
    while let Some(&first) = sorted.get(at) {
        let mut end = at + 1;
        while sorted
            .get(end)
            .is_some_and(|seq| seq.0 == first.0 + (end - at) as u64)
        {
            end += 1;
        }
        let entries = crate::store::read_log_after(conn, Seq(first.0 - 1), (end - at) as u64)?;
        read.extend(entries.into_iter().map(|entry| (entry.seq, entry)));
        at = end;
    }
    Ok(order.iter().filter_map(|seq| read.remove(seq)).collect())
}

/// The changesets with who started the run each came from, in the order given.
pub(crate) fn logged_changesets(
    conn: &Connection,
    entries: Vec<LogEntry>,
) -> Result<Vec<LoggedChangeset>> {
    let (Some(first), Some(last)) = (
        entries.iter().map(|entry| entry.seq).min(),
        entries.iter().map(|entry| entry.seq).max(),
    ) else {
        return Ok(Vec::new());
    };
    let mut started = started_by(conn, first, last)?;
    Ok(entries
        .into_iter()
        .map(|entry| LoggedChangeset {
            started_by: started.remove(&entry.seq),
            entry,
        })
        .collect())
}

pub(crate) fn changeset_summaries(
    conn: &Connection,
    after: Seq,
    head: Seq,
    most: u64,
) -> Result<Vec<ChangesetSummary>> {
    if after >= head {
        return Ok(Vec::new());
    }
    let mut statement = conn.prepare(
        "WITH page AS (
           SELECT * FROM spool.changeset WHERE seq > $1 AND seq <= $2 ORDER BY seq LIMIT $3
         )
         SELECT c.seq, c.changeset_id::VARCHAR, c.base_seq, c.actor, c.reason, c.run_id,
                c.content_hash, (SELECT count(*) FROM spool.changeset_op o WHERE o.seq = c.seq)
         FROM page c ORDER BY c.seq",
    )?;
    let mut rows = statement.query(params![after.0 as i64, head.0 as i64, clamp_limit(most)])?;
    let mut summaries = Vec::new();
    while let Some(row) = rows.next()? {
        let entry = log_entry(row)?;
        summaries.push(ChangesetSummary {
            seq: entry.seq,
            id: entry.changeset.id,
            base: entry.changeset.base,
            actor: entry.changeset.actor,
            reason: entry.changeset.reason,
            run: entry.changeset.run,
            content_hash: entry.content_hash,
            started_by: None,
            operation_count: row.get::<_, i64>(7)? as u64,
        });
    }
    if let (Some(first), Some(last)) = (summaries.first(), summaries.last()) {
        let mut started = started_by(conn, first.seq, last.seq)?;
        for summary in &mut summaries {
            summary.started_by = started.remove(&summary.seq);
        }
    }
    Ok(summaries)
}

fn started_by(conn: &Connection, first: Seq, last: Seq) -> Result<BTreeMap<Seq, Actor>> {
    // A changeset names the reconcile run that drafted it. A retraction and
    // a load of rows that came back commit one that names no run, and the
    // run's report names the changeset instead. Those runs are the ones
    // `spool.retraction` names, so no other run's report is read.
    let mut statement = conn.prepare(
        "SELECT c.seq, r.started_by
         FROM spool.changeset c
         JOIN spool.run r ON r.run_id = c.run_id
         WHERE c.seq BETWEEN $1 AND $2
         UNION ALL
         SELECT c.seq, r.started_by
         FROM spool.changeset c
         JOIN spool.run r
           ON TRY_CAST(json_extract_string(r.result, '$.committed.seq') AS BIGINT) = c.seq
         WHERE c.seq BETWEEN $1 AND $2 AND c.run_id IS NULL
           AND r.kind IN ('load', 'retract')
           AND r.run_id IN (
             SELECT retract_run FROM spool.retraction
             UNION ALL
             SELECT restored_run FROM spool.retraction WHERE restored_run IS NOT NULL)",
    )?;
    let mut rows = statement.query(params![first.0 as i64, last.0 as i64])?;
    let mut started: BTreeMap<Seq, Actor> = BTreeMap::new();
    while let Some(row) = rows.next()? {
        started.insert(
            Seq(row.get::<_, i64>(0)? as u64),
            parse(&row.get::<_, String>(1)?)?,
        );
    }
    Ok(started)
}
