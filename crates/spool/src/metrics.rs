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

/// How much identity state changed between two log positions, against how
/// much arrived. The counts compare the two positions: an identity that was
/// merged and reinstated in between is not counted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Churn {
    /// Observations ingested between the two positions.
    pub new_observations: u64,
    pub identities_created: u64,
    pub identities_merged: u64,
    /// Observations that had an identity at the earlier position and have
    /// another, or none, at the later one. A merge reassigns the members it
    /// moves.
    pub observations_reassigned: u64,
    /// The three counts per thousand new observations. `None` when nothing
    /// was ingested.
    pub created_per_thousand: Option<f64>,
    pub merged_per_thousand: Option<f64>,
    pub reassigned_per_thousand: Option<f64>,
}

/// Whether the splits a scenario made still hold: no identity has members
/// from both sides of the split.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SplitRecovery {
    pub splits: u64,
    pub kept_apart: u64,
    pub share: Option<f64>,
}

/// Whether the constraints a scenario recorded are still honoured. A
/// constraint is honoured while it is in force and state agrees with it: the
/// two observations of a `must_link` share an identity, and those of a
/// `cannot_link` do not.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OverridePersistence {
    pub constraints: u64,
    pub honoured: u64,
    pub share: Option<f64>,
}

/// How far the result depends on the order the batches arrived in: the
/// steps as written against the same steps in reverse, without the
/// scenario's corrections. An observation without an identity is a group of
/// its own.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrderSensitivity {
    pub observations: u64,
    /// Observations grouped with the same observations in both orders.
    pub same_group: u64,
    pub agreement: Option<f64>,
    /// Pairs of observations that share an identity in both orders, and in
    /// at least one.
    pub pairs_in_both: u64,
    pub pairs_in_either: u64,
    pub pair_agreement: Option<f64>,
}

impl SplitRecovery {
    pub(crate) fn new(splits: u64, kept_apart: u64) -> Self {
        Self {
            splits,
            kept_apart,
            share: ratio(kept_apart as f64, splits as f64),
        }
    }
}

impl OverridePersistence {
    pub(crate) fn new(constraints: u64, honoured: u64) -> Self {
        Self {
            constraints,
            honoured,
            share: ratio(honoured as f64, constraints as f64),
        }
    }
}

impl OrderSensitivity {
    /// Compares two groupings of the same observations, each a list of
    /// `(observation, group)`.
    pub(crate) fn between(first: &[(String, String)], second: &[(String, String)]) -> Self {
        use std::collections::{BTreeSet, HashMap};

        fn groups(partition: &[(String, String)]) -> HashMap<&str, BTreeSet<&str>> {
            let mut groups: HashMap<&str, BTreeSet<&str>> = HashMap::new();
            for (observation, group) in partition {
                groups.entry(group).or_default().insert(observation);
            }
            groups
        }
        let pairs = |size: usize| (size * size.saturating_sub(1) / 2) as u64;

        let (first_groups, second_groups) = (groups(first), groups(second));
        let second_of: HashMap<&str, &str> = second
            .iter()
            .map(|(observation, group)| (observation.as_str(), group.as_str()))
            .collect();

        let mut same_group = 0;
        let mut cells: HashMap<(&str, &str), usize> = HashMap::new();
        for (observation, group) in first {
            let Some(other) = second_of.get(observation.as_str()) else {
                continue;
            };
            *cells.entry((group, other)).or_default() += 1;
            if first_groups[group.as_str()] == second_groups[other] {
                same_group += 1;
            }
        }
        let in_both: u64 = cells.values().map(|size| pairs(*size)).sum();
        let in_first: u64 = first_groups.values().map(|group| pairs(group.len())).sum();
        let in_second: u64 = second_groups.values().map(|group| pairs(group.len())).sum();
        let in_either = in_first + in_second - in_both;
        let observations = first.len() as u64;
        Self {
            observations,
            same_group,
            agreement: ratio(same_group as f64, observations as f64),
            pairs_in_both: in_both,
            pairs_in_either: in_either,
            pair_agreement: ratio(in_both as f64, in_either as f64),
        }
    }
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
        // From the counts, not from precision and recall: when no predicted
        // pair is right both of those are zero, and F1 is zero, not undefined.
        let f1 = ratio(2.0 * true_positives as f64, (predicted + actual) as f64);
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

/// Counts what changed between two log positions.
pub(crate) fn churn(
    conn: &Connection,
    before: Seq,
    after: Seq,
    new_observations: u64,
) -> Result<Churn> {
    let at = |seq: Seq| {
        format!(
            "SELECT observation_id, identity_id FROM spool.membership
             WHERE from_seq <= {seq} AND (to_seq IS NULL OR to_seq > {seq})"
        )
    };
    let sql = format!(
        "SELECT
           (SELECT count(*) FROM spool.identity
            WHERE created_seq > {before} AND created_seq <= {after})::BIGINT,
           (SELECT count(*) FROM spool.identity
            WHERE merged_seq > {before} AND merged_seq <= {after})::BIGINT,
           (SELECT count(*) FROM ({}) b LEFT JOIN ({}) a ON a.observation_id = b.observation_id
            WHERE a.identity_id IS DISTINCT FROM b.identity_id)::BIGINT",
        at(before),
        at(after)
    );
    conn.query_row(&sql, [], |row| {
        let count = |index| row.get::<_, i64>(index).map(|value| value as u64);
        let (created, merged, reassigned) = (count(0)?, count(1)?, count(2)?);
        let per_thousand = |count: u64| ratio(count as f64 * 1000.0, new_observations as f64);
        Ok(Churn {
            new_observations,
            identities_created: created,
            identities_merged: merged,
            observations_reassigned: reassigned,
            created_per_thousand: per_thousand(created),
            merged_per_thousand: per_thousand(merged),
            reassigned_per_thousand: per_thousand(reassigned),
        })
    })
    .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::OrderSensitivity;

    fn grouping(groups: &[&[&str]]) -> Vec<(String, String)> {
        groups
            .iter()
            .enumerate()
            .flat_map(|(group, members)| {
                members
                    .iter()
                    .map(move |member| ((*member).to_owned(), format!("group-{group}")))
            })
            .collect()
    }

    #[test]
    fn the_same_groups_under_other_ids_agree_completely() {
        let first = grouping(&[&["a", "b", "c"], &["d"], &["e", "f"]]);
        let second = grouping(&[&["f", "e"], &["c", "a", "b"], &["d"]]);
        let order = OrderSensitivity::between(&first, &second);
        assert_eq!((order.observations, order.same_group), (6, 6));
        assert_eq!((order.pairs_in_both, order.pairs_in_either), (4, 4));
        assert_eq!(order.agreement, Some(1.0));
        assert_eq!(order.pair_agreement, Some(1.0));
    }

    #[test]
    fn an_observation_that_moved_changes_both_groups_it_touched() {
        // `c` is with `a` and `b` in one order and with `d` in the other.
        let first = grouping(&[&["a", "b", "c"], &["d"], &["e", "f"]]);
        let second = grouping(&[&["a", "b"], &["c", "d"], &["e", "f"]]);
        let order = OrderSensitivity::between(&first, &second);
        assert_eq!((order.observations, order.same_group), (6, 2));
        // Together in both: a-b and e-f. In either: those, a-c, b-c and c-d.
        assert_eq!((order.pairs_in_both, order.pairs_in_either), (2, 5));
        assert_eq!(order.pair_agreement, Some(0.4));
    }

    #[test]
    fn nothing_grouped_has_no_pairs_to_agree_on() {
        let alone = grouping(&[&["a"], &["b"]]);
        let order = OrderSensitivity::between(&alone, &alone);
        assert_eq!(order.agreement, Some(1.0));
        assert_eq!(order.pair_agreement, None);
        let empty = OrderSensitivity::between(&[], &[]);
        assert_eq!((empty.agreement, empty.pair_agreement), (None, None));
    }
}
