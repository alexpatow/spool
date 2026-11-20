//! Resolution quality, measured on committed state against a truth relation
//! of `(record_id, true_entity)`.
//!
//! Only records named in the truth relation are measured. A record
//! without an identity counts as a group of its own, so abstaining costs
//! recall and never precision.

use duckdb::Connection;
use serde::{Deserialize, Serialize};
use spool_core::Seq;

use crate::error::Result;

/// The name of the temporary table that holds the truth relation.
pub(crate) const TRUTH: &str = "spool_truth";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Metrics {
    /// Records that have a true entity.
    pub records: u64,
    pub true_entities: u64,
    /// Identities that hold at least one of those records.
    pub identities: u64,
    /// Records left without an identity, and their share of the total.
    pub unassigned: u64,
    pub unassigned_share: Option<f64>,
    pub pairs: PairMetrics,
    pub b_cubed: BCubed,
    /// Identities that contain more than one true entity.
    pub false_merges: u64,
    /// True entities spread over more than one identity.
    pub false_splits: u64,
}

/// Pairs of records placed in the same identity, against pairs that
/// share a true entity. A ratio is `None` when its denominator is zero:
/// precision when no pair was predicted, recall when the truth has no pair,
/// and F1, which is `2 * true_positives / (predicted + actual)`, only when
/// both hold.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PairMetrics {
    pub true_positives: u64,
    pub predicted: u64,
    pub actual: u64,
    pub precision: Option<f64>,
    pub recall: Option<f64>,
    pub f1: Option<f64>,
}

/// B-cubed precision and recall: for each record, the share of its
/// identity that shares its true entity, and the share of its true entity
/// that shares its identity, averaged over records.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BCubed {
    pub precision: Option<f64>,
    pub recall: Option<f64>,
}

/// How many records kept their identity between two log positions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IdStability {
    /// Records that had an identity at the earlier position.
    pub records: u64,
    /// Those whose identity ID is the same at the later position.
    pub unchanged: u64,
    /// Those whose identity is the same once merged IDs are followed to the
    /// identities that stood for them at the later position.
    pub unchanged_following_redirects: u64,
    pub strict: Option<f64>,
    pub following_redirects: Option<f64>,
}

/// How much identity state changed between two log positions, against how
/// much arrived. The counts compare the two positions, and do not depend on
/// what was committed after the later one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Churn {
    /// Records loaded between the two positions.
    pub new_records: u64,
    pub identities_created: u64,
    /// Identities that are merged at the later position by a merge made after
    /// the earlier one. One that was merged and reinstated in between is not
    /// counted.
    pub identities_merged: u64,
    /// Records that had an identity at the earlier position and have
    /// another, or none, at the later one. A merge reassigns the members it
    /// moves.
    pub records_reassigned: u64,
    /// The three counts per thousand new records. `None` when nothing
    /// was loaded.
    pub created_per_thousand: Option<f64>,
    pub merged_per_thousand: Option<f64>,
    pub reassigned_per_thousand: Option<f64>,
}

fn ratio(numerator: f64, denominator: f64) -> Option<f64> {
    (denominator > 0.0).then(|| numerator / denominator)
}

/// Scores current state against the truth in the [`TRUTH`] table.
pub(crate) fn score(conn: &Connection) -> Result<Metrics> {
    let tables = format!(
        "WITH labelled AS (
           SELECT t.true_entity,
                  m.identity_id IS NULL AS unassigned,
                  coalesce(m.identity_id::VARCHAR, 'unassigned:' || t.record_id::VARCHAR) AS cluster
           FROM {TRUTH} t
           LEFT JOIN spool.membership m ON m.record_id = t.record_id AND m.to_seq IS NULL
         ),
         cell AS (
           SELECT cluster, true_entity, any_value(unassigned) AS unassigned, count(*) AS n
           FROM labelled GROUP BY cluster, true_entity
         ),
         by_cluster AS (
           SELECT cluster, any_value(unassigned) AS unassigned, sum(n) AS size, count(*) AS entities
           FROM cell GROUP BY cluster
         ),
         by_entity AS (
           SELECT true_entity, sum(n) AS size, count(*) FILTER (WHERE NOT unassigned) AS identities
           FROM cell GROUP BY true_entity
         )"
    );

    // B-cubed sums n²/size over cells. Adding fractions in whatever order the
    // database visits them gives results that differ in the last digit from
    // run to run, so the database adds whole numbers per size and the
    // fractions are added here, in size order.
    let b_cubed = |group: &str, key: &str| -> Result<f64> {
        let mut statement = conn.prepare(&format!(
            "{tables}
             SELECT g.size::BIGINT, sum(c.n * c.n)::BIGINT
             FROM cell c JOIN {group} g ON g.{key} = c.{key}
             GROUP BY g.size ORDER BY g.size"
        ))?;
        let mut rows = statement.query([])?;
        let mut total = 0.0;
        while let Some(row) = rows.next()? {
            let (size, squares): (i64, i64) = (row.get(0)?, row.get(1)?);
            total += squares as f64 / size as f64;
        }
        Ok(total)
    };
    let b_cubed_precision = b_cubed("by_cluster", "cluster")?;
    let b_cubed_recall = b_cubed("by_entity", "true_entity")?;

    let sql = format!(
        "{tables}
         SELECT
           (SELECT coalesce(sum(n), 0) FROM cell)::BIGINT,
           (SELECT count(*) FROM by_entity)::BIGINT,
           (SELECT count(*) FROM by_cluster WHERE NOT unassigned)::BIGINT,
           (SELECT coalesce(sum(size), 0) FROM by_cluster WHERE unassigned)::BIGINT,
           (SELECT coalesce(sum(n * (n - 1) // 2), 0) FROM cell)::BIGINT,
           (SELECT coalesce(sum(size * (size - 1) // 2), 0) FROM by_cluster)::BIGINT,
           (SELECT coalesce(sum(size * (size - 1) // 2), 0) FROM by_entity)::BIGINT,
           (SELECT count(*) FROM by_cluster WHERE NOT unassigned AND entities > 1)::BIGINT,
           (SELECT count(*) FROM by_entity WHERE identities > 1)::BIGINT"
    );
    conn.query_row(&sql, [], |row| {
        let count = |index| row.get::<_, i64>(index).map(|value| value as u64);
        let records = count(0)?;
        let unassigned = count(3)?;
        let (true_positives, predicted, actual) = (count(4)?, count(5)?, count(6)?);

        let total = records as f64;
        let precision = ratio(true_positives as f64, predicted as f64);
        let recall = ratio(true_positives as f64, actual as f64);
        // From the counts, not from precision and recall: when no predicted
        // pair is right both of those are zero, and F1 is zero, not undefined.
        let f1 = ratio(2.0 * true_positives as f64, (predicted + actual) as f64);
        Ok(Metrics {
            records,
            true_entities: count(1)?,
            identities: count(2)?,
            unassigned,
            unassigned_share: ratio(unassigned as f64, total),
            pairs: PairMetrics {
                true_positives,
                predicted,
                actual,
                precision,
                recall,
                f1,
            },
            b_cubed: BCubed {
                precision: ratio(b_cubed_precision, total),
                recall: ratio(b_cubed_recall, total),
            },
            false_merges: count(7)?,
            false_splits: count(8)?,
        })
    })
    .map_err(Into::into)
}

/// The identities that were merged at a log position, each with the identity
/// its merge named and the position of that merge.
///
/// This is read from the log. The identity projection holds only what is
/// merged now: reinstating an identity clears its merge, and merging it again
/// replaces the position, so the projection cannot answer for an earlier
/// position. A changeset merges or reinstates an identity at most once, so
/// the last such operation at or before the position decides.
fn merged_at(seq: Seq) -> String {
    format!(
        "SELECT identity_id, arg_max(target, seq) AS target, max(seq) AS seq
         FROM (
           SELECT CASE kind WHEN 'merge' THEN from_identity_id ELSE identity_id END AS identity_id,
                  kind,
                  identity_id AS target,
                  seq
           FROM spool.changeset_op
           WHERE kind IN ('merge', 'reinstate') AND seq <= {seq}
         )
         GROUP BY identity_id
         HAVING arg_max(kind, seq) = 'merge'"
    )
}

/// Compares membership at two log positions using the membership intervals.
/// Redirects are those of the later position.
pub(crate) fn id_stability(conn: &Connection, before: Seq, after: Seq) -> Result<IdStability> {
    let at = |seq: Seq| {
        format!(
            "SELECT m.record_id, m.identity_id, coalesce(s.survivor, m.identity_id) AS survivor
             FROM spool.membership m
             LEFT JOIN survivor s ON s.identity_id = m.identity_id
             WHERE m.from_seq <= {seq} AND (m.to_seq IS NULL OR m.to_seq > {seq})"
        )
    };
    let sql = format!(
        "WITH RECURSIVE merged AS ({}),
         follow(identity_id, stands_for) AS (
           SELECT identity_id, target FROM merged
           UNION ALL
           SELECT f.identity_id, m.target
           FROM follow f JOIN merged m ON m.identity_id = f.stands_for
         ),
         survivor AS (
           SELECT identity_id, stands_for AS survivor FROM follow
           WHERE stands_for NOT IN (SELECT identity_id FROM merged)
         )
         SELECT count(*)::BIGINT,
                count(*) FILTER (WHERE a.identity_id = b.identity_id)::BIGINT,
                count(*) FILTER (WHERE a.survivor = b.survivor)::BIGINT
         FROM ({}) b LEFT JOIN ({}) a ON a.record_id = b.record_id",
        merged_at(after),
        at(before),
        at(after)
    );
    conn.query_row(&sql, [], |row| {
        let records = row.get::<_, i64>(0)? as u64;
        let unchanged = row.get::<_, i64>(1)? as u64;
        let following = row.get::<_, i64>(2)? as u64;
        Ok(IdStability {
            records,
            unchanged,
            unchanged_following_redirects: following,
            strict: ratio(unchanged as f64, records as f64),
            following_redirects: ratio(following as f64, records as f64),
        })
    })
    .map_err(Into::into)
}

/// Counts what changed between two log positions.
pub(crate) fn churn(conn: &Connection, before: Seq, after: Seq, new_records: u64) -> Result<Churn> {
    let at = |seq: Seq| {
        format!(
            "SELECT record_id, identity_id FROM spool.membership
             WHERE from_seq <= {seq} AND (to_seq IS NULL OR to_seq > {seq})"
        )
    };
    let sql = format!(
        "SELECT
           (SELECT count(*) FROM spool.identity
            WHERE created_seq > {before} AND created_seq <= {after})::BIGINT,
           (SELECT count(*) FROM ({}) WHERE seq > {before})::BIGINT,
           (SELECT count(*) FROM ({}) b LEFT JOIN ({}) a ON a.record_id = b.record_id
            WHERE a.identity_id IS DISTINCT FROM b.identity_id)::BIGINT",
        merged_at(after),
        at(before),
        at(after)
    );
    conn.query_row(&sql, [], |row| {
        let count = |index| row.get::<_, i64>(index).map(|value| value as u64);
        let (created, merged, reassigned) = (count(0)?, count(1)?, count(2)?);
        let per_thousand = |count: u64| ratio(count as f64 * 1000.0, new_records as f64);
        Ok(Churn {
            new_records,
            identities_created: created,
            identities_merged: merged,
            records_reassigned: reassigned,
            created_per_thousand: per_thousand(created),
            merged_per_thousand: per_thousand(merged),
            reassigned_per_thousand: per_thousand(reassigned),
        })
    })
    .map_err(Into::into)
}
