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

/// The mentions an extractor found, against the labelled mentions of the
/// same documents. A mention is right when a label has the same document,
/// entity type, start and end: half a name is a wrong mention and a missed
/// one. This says nothing about which entity a mention is, which is what
/// resolution is scored on.
///
/// A ratio is `None` when its denominator is zero, as for [`PairMetrics`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExtractionMetrics {
    /// Labelled mentions.
    pub labelled: u64,
    /// Mentions the extractor found.
    pub extracted: u64,
    /// Those of them that are labelled.
    pub correct: u64,
    pub precision: Option<f64>,
    pub recall: Option<f64>,
    pub f1: Option<f64>,
}

impl ExtractionMetrics {
    pub(crate) fn new(labelled: u64, extracted: u64, correct: u64) -> Self {
        let ratio = |numerator: u64, denominator: u64| {
            (denominator > 0).then(|| numerator as f64 / denominator as f64)
        };
        Self {
            labelled,
            extracted,
            correct,
            precision: ratio(correct, extracted),
            recall: ratio(correct, labelled),
            f1: ratio(2 * correct, extracted + labelled),
        }
    }
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
    /// Records ingested between the two positions.
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
/// two records of a `must_link` share an identity, and those of a
/// `cannot_link` do not.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OverridePersistence {
    pub constraints: u64,
    pub honoured: u64,
    pub share: Option<f64>,
}

/// How far the result depends on the order the batches arrived in: the
/// steps as written against the same steps in reverse, without the
/// scenario's corrections. A record without an identity is a group of
/// its own.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrderSensitivity {
    pub records: u64,
    /// Records grouped with the same records in both orders.
    pub same_group: u64,
    pub agreement: Option<f64>,
    /// Pairs of records that share an identity in both orders, and in
    /// at least one.
    pub pairs_in_both: u64,
    pub pairs_in_either: u64,
    pub pair_agreement: Option<f64>,
}

/// How the identities of a store that rows were taken back from compare
/// with those of a store that was never sent them: the same steps in the
/// same order, without the rows the sources have taken back as things
/// stand, and without the scenario's corrections. The records compared are
/// those the sources still hold, and one without an identity is a group of
/// its own.
///
/// The two differ where a row that is gone was what joined two that are
/// left: the store keeps them together, as it never parts an identity on
/// its own, and the rebuild never puts them together.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rebuild {
    pub records: u64,
    /// Records grouped with the same records in both.
    pub same_group: u64,
    pub agreement: Option<f64>,
    /// Pairs of records that share an identity in both.
    pub pairs_in_both: u64,
    /// Pairs that share an identity in the store and not in the rebuild.
    pub pairs_only_kept: u64,
    /// Pairs that share an identity in the rebuild and not in the store.
    pub pairs_only_rebuilt: u64,
    /// Pairs in both, of those in either.
    pub pair_agreement: Option<f64>,
    /// The rebuild against the truth, to set beside the step's own.
    pub metrics: Metrics,
}

impl Rebuild {
    /// Compares the grouping a store kept with the one a rebuild gives the
    /// same records, each a list of `(record, group)`.
    pub(crate) fn between(
        kept: &[(String, String)],
        rebuilt: &[(String, String)],
        metrics: Metrics,
    ) -> Self {
        let compared = Compared::between(kept, rebuilt);
        let in_either = compared.in_first + compared.in_second - compared.in_both;
        Self {
            records: compared.records,
            same_group: compared.same_group,
            agreement: ratio(compared.same_group as f64, compared.records as f64),
            pairs_in_both: compared.in_both,
            pairs_only_kept: compared.in_first - compared.in_both,
            pairs_only_rebuilt: compared.in_second - compared.in_both,
            pair_agreement: ratio(compared.in_both as f64, in_either as f64),
            metrics,
        }
    }
}

/// What matching everything again with a strategy did to the identities
/// that existed. This is the cost of changing strategy: how many
/// records kept their identity ID, and how much of what the strategy
/// would change was only suggested.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UpgradeStability {
    /// The strategy that judged every record again.
    pub strategy: String,
    /// Records that had an identity before the rematch.
    pub records: u64,
    /// Those whose identity ID is the same after it.
    pub retained: u64,
    pub share: Option<f64>,
    /// Pairs of identities the strategy's judgements would merge, and
    /// identities they would split. None of it is applied by the rematch.
    pub suggested_merges: u64,
    pub suggested_splits: u64,
    /// Changes to existing identities that were committed during the step:
    /// identities merged and records moved.
    pub applied_changes: u64,
}

impl UpgradeStability {
    pub(crate) fn new(
        strategy: &str,
        stability: &IdStability,
        churn: &Churn,
        suggested_merges: u64,
        suggested_splits: u64,
    ) -> Self {
        Self {
            strategy: strategy.to_owned(),
            records: stability.records,
            retained: stability.unchanged,
            share: stability.strict,
            suggested_merges,
            suggested_splits,
            applied_changes: churn.identities_merged + churn.records_reassigned,
        }
    }
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
    /// Compares two groupings of the same records, each a list of
    /// `(record, group)`.
    pub(crate) fn between(first: &[(String, String)], second: &[(String, String)]) -> Self {
        let compared = Compared::between(first, second);
        let in_either = compared.in_first + compared.in_second - compared.in_both;
        Self {
            records: compared.records,
            same_group: compared.same_group,
            agreement: ratio(compared.same_group as f64, compared.records as f64),
            pairs_in_both: compared.in_both,
            pairs_in_either: in_either,
            pair_agreement: ratio(compared.in_both as f64, in_either as f64),
        }
    }
}

/// Two groupings of the same records, set against each other.
struct Compared {
    records: u64,
    /// Records grouped with the same records in both.
    same_group: u64,
    /// Pairs of records that share a group in both, in the first and in
    /// the second.
    in_both: u64,
    in_first: u64,
    in_second: u64,
}

impl Compared {
    fn between(first: &[(String, String)], second: &[(String, String)]) -> Self {
        use std::collections::{BTreeSet, HashMap};

        fn groups(partition: &[(String, String)]) -> HashMap<&str, BTreeSet<&str>> {
            let mut groups: HashMap<&str, BTreeSet<&str>> = HashMap::new();
            for (record, group) in partition {
                groups.entry(group).or_default().insert(record);
            }
            groups
        }
        let pairs = |size: usize| (size * size.saturating_sub(1) / 2) as u64;

        let (first_groups, second_groups) = (groups(first), groups(second));
        let second_of: HashMap<&str, &str> = second
            .iter()
            .map(|(record, group)| (record.as_str(), group.as_str()))
            .collect();

        let mut same_group = 0;
        let mut cells: HashMap<(&str, &str), usize> = HashMap::new();
        for (record, group) in first {
            let Some(other) = second_of.get(record.as_str()) else {
                continue;
            };
            *cells.entry((group, other)).or_default() += 1;
            if first_groups[group.as_str()] == second_groups[other] {
                same_group += 1;
            }
        }
        Self {
            records: first.len() as u64,
            same_group,
            in_both: cells.values().map(|size| pairs(*size)).sum(),
            in_first: first_groups.values().map(|group| pairs(group.len())).sum(),
            in_second: second_groups.values().map(|group| pairs(group.len())).sum(),
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
        assert_eq!((order.records, order.same_group), (6, 6));
        assert_eq!((order.pairs_in_both, order.pairs_in_either), (4, 4));
        assert_eq!(order.agreement, Some(1.0));
        assert_eq!(order.pair_agreement, Some(1.0));
    }

    #[test]
    fn an_record_that_moved_changes_both_groups_it_touched() {
        // `c` is with `a` and `b` in one order and with `d` in the other.
        let first = grouping(&[&["a", "b", "c"], &["d"], &["e", "f"]]);
        let second = grouping(&[&["a", "b"], &["c", "d"], &["e", "f"]]);
        let order = OrderSensitivity::between(&first, &second);
        assert_eq!((order.records, order.same_group), (6, 2));
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
