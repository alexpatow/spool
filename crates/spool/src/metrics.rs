//! Resolution quality, measured on committed state against a truth relation
//! of `(observation_id, true_entity)`.
//!
//! Only observations named in the truth relation are measured. An observation
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
    /// Observations that have a true entity.
    pub observations: u64,
    pub true_entities: u64,
    /// Identities that hold at least one of those observations.
    pub identities: u64,
    /// Observations left without an identity, and their share of the total.
    pub unassigned: u64,
    pub unassigned_share: Option<f64>,
    pub pairs: PairMetrics,
    pub b_cubed: BCubed,
    /// Identities that contain more than one true entity.
    pub false_merges: u64,
    /// True entities spread over more than one identity.
    pub false_splits: u64,
}

/// Pairs of observations placed in the same identity, against pairs that
/// share a true entity. A ratio is `None` when its denominator is zero.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PairMetrics {
    pub true_positives: u64,
    pub predicted: u64,
    pub actual: u64,
    pub precision: Option<f64>,
    pub recall: Option<f64>,
    pub f1: Option<f64>,
}

/// B-cubed precision and recall: for each observation, the share of its
/// identity that shares its true entity, and the share of its true entity
/// that shares its identity, averaged over observations.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BCubed {
    pub precision: Option<f64>,
    pub recall: Option<f64>,
}

/// How many observations kept their identity between two log positions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IdStability {
    /// Observations that had an identity at the earlier position.
    pub observations: u64,
    /// Those whose identity ID is the same at the later position.
    pub unchanged: u64,
    /// Those whose identity is the same once merged IDs are followed to
    /// their survivors.
    pub unchanged_following_redirects: u64,
    pub strict: Option<f64>,
    pub following_redirects: Option<f64>,
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
                  coalesce(m.identity_id::VARCHAR, 'unassigned:' || t.observation_id::VARCHAR) AS cluster
           FROM {TRUTH} t
           LEFT JOIN spool.membership m ON m.observation_id = t.observation_id AND m.to_seq IS NULL
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
        let observations = count(0)?;
        let unassigned = count(3)?;
        let (true_positives, predicted, actual) = (count(4)?, count(5)?, count(6)?);

        let total = observations as f64;
        let precision = ratio(true_positives as f64, predicted as f64);
        let recall = ratio(true_positives as f64, actual as f64);
        let f1 = match (precision, recall) {
            (Some(p), Some(r)) => ratio(2.0 * p * r, p + r),
            _ => None,
        };
        Ok(Metrics {
            observations,
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

/// Compares membership at two log positions using the membership intervals.
pub(crate) fn id_stability(conn: &Connection, before: Seq, after: Seq) -> Result<IdStability> {
    let at = |seq: Seq| {
        format!(
            "SELECT m.observation_id, m.identity_id, coalesce(i.merged_into, m.identity_id) AS survivor
             FROM spool.membership m
             JOIN spool.identity i ON i.identity_id = m.identity_id
             WHERE m.from_seq <= {seq} AND (m.to_seq IS NULL OR m.to_seq > {seq})"
        )
    };
    let sql = format!(
        "SELECT count(*)::BIGINT,
                count(*) FILTER (WHERE a.identity_id = b.identity_id)::BIGINT,
                count(*) FILTER (WHERE a.survivor = b.survivor)::BIGINT
         FROM ({}) b LEFT JOIN ({}) a ON a.observation_id = b.observation_id",
        at(before),
        at(after)
    );
    conn.query_row(&sql, [], |row| {
        let observations = row.get::<_, i64>(0)? as u64;
        let unchanged = row.get::<_, i64>(1)? as u64;
        let following = row.get::<_, i64>(2)? as u64;
        Ok(IdStability {
            observations,
            unchanged,
            unchanged_following_redirects: following,
            strict: ratio(unchanged as f64, observations as f64),
            following_redirects: ratio(following as f64, observations as f64),
        })
    })
    .map_err(Into::into)
}
