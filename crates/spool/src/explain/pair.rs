//! Explaining a pair of records: where each of the two is, what the runs
//! that compared them concluded, and what the rules say of them now.

use std::collections::BTreeMap;

use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};
use spool_core::{Actor, ChangesetId, ConstraintEntry, RecordId, RunId, Seq, StateError, Verdict};

use super::identity::{Describe, Link, Split, read_member_labels};
use super::prose::pair_summary;
use super::{feature_row, has_table};
use crate::drafts::DraftStatus;
use crate::engine::StrategyRef;
use crate::error::{Error, Result};
use crate::features;
use crate::links::Graph;
use crate::lookup::{self, RecordEntry};
use crate::project::{Manifest, Rule, RuleSet};
use crate::review::{self, ReviewEntry, ReviewFilter};
use crate::runs::RunStatus;
use crate::store::{self, parse};
use crate::time::format_rfc3339;

/// What the store knows about a pair of records, and what follows
/// from it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Explanation {
    /// The record with the smaller ID. It is `l` in a rule.
    pub left: Explained,
    /// The record with the larger ID. It is `r` in a rule.
    pub right: Explained,
    pub outcome: Outcome,
    /// The explanation in sentences, the outcome first.
    pub summary: Vec<String>,
    /// What the project's rules say about the pair now. `null` when the two
    /// are of different entity types, when the engine runs without a
    /// manifest, and when either has no feature row.
    pub rules: Option<RuleReading>,
    /// The match runs that looked at the pair, oldest first.
    pub comparisons: Vec<Comparison>,
    /// The constraints between the two, oldest first, with those that were
    /// removed.
    pub constraints: Vec<ConstraintEntry>,
    /// The review items that name either of the two, oldest first.
    pub review_items: Vec<ReviewEntry>,
    /// When the two share an identity: the shortest chain of links from
    /// `left` to `right` through members of that identity. Empty when they
    /// do not share one, and when no chain joins them as things are now:
    /// what joined them was removed since, or a changeset placed one of
    /// them directly.
    pub path: Vec<Link>,
    /// When they share an identity: the fewest links whose removal would
    /// part them, and the two sides that would leave. `null` when they do
    /// not share one, when no chain joins them, and when more than twenty
    /// links would have to go.
    #[serde(default)]
    pub cut: Option<Split>,
    /// When they share an identity: the `match` rules without whose links
    /// they would not be together, each of which alone made a link that
    /// every chain between them goes through.
    #[serde(default)]
    pub held_by: Vec<String>,
}

/// Where the two records are in relation to each other.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    SameIdentity,
    DifferentIdentities,
    /// At least one of the two has no identity.
    Unassigned,
    /// The two are of different entity types, and are never compared.
    DifferentTypes,
}

/// One side of an explanation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Explained {
    pub record: RecordEntry,
    /// Its row in the feature table of its entity type, by column name:
    /// what rules and strategies see of it. `null` when it has no row.
    pub features: Option<serde_json::Map<String, serde_json::Value>>,
    /// The changeset that put it in the identity it has. `null` while it has
    /// none.
    pub placed: Option<Placement>,
}

/// The changeset that put a record where it is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Placement {
    pub seq: Seq,
    pub changeset: ChangesetId,
    pub actor: Actor,
    pub reason: Option<String>,
    /// The reconcile run that drafted the changeset, if one did.
    pub run: Option<RunId>,
    pub committed_at: String,
}

/// The project's rules applied to a pair as it is now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RuleReading {
    pub block: Vec<RuleResult>,
    pub no_match: Vec<RuleResult>,
    #[serde(rename = "match")]
    pub match_: Vec<RuleResult>,
    /// Whether a block rule holds, so that a run of the rules would compare
    /// the two.
    pub candidate: bool,
    /// What a run of the rules would judge. `null` when the pair is not a
    /// candidate, and when no `no_match` or `match` rule holds.
    pub verdict: Option<Verdict>,
    /// What SQL said when it refused to apply the `no_match` and `match`
    /// rules to the pair, as it refuses the cast of a value that does not
    /// convert. Their `holds` is then `null`. A run applies those rules
    /// only to pairs a block rule found, so a rule may rely on what the
    /// block rules let through and fail on a pair they did not. On a
    /// candidate pair, a run of the rules fails the same way.
    pub failed: Option<String>,
}

/// One rule applied to a pair.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RuleResult {
    pub name: String,
    /// The rule's SQL, over the feature rows `l` and `r`.
    pub predicate: String,
    /// Whether the predicate holds. `null` when it cannot be decided, which
    /// is what SQL makes of a comparison with a feature that is null, and
    /// for a `no_match` or `match` rule when those `failed`. A rule that
    /// cannot be decided does not hold.
    pub holds: Option<bool>,
}

/// What one match run made of a pair.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Comparison {
    pub run: RunId,
    pub status: RunStatus,
    pub strategy: Option<StrategyRef>,
    pub started_at: String,
    /// The block rules that made the pair a candidate in this run. Empty
    /// when a strategy judged a pair no block rule had found.
    pub blocked_by: Vec<String>,
    /// What the strategy judged. Empty when the pair was a candidate and
    /// got no judgement.
    pub judgements: Vec<RecordedJudgement>,
    /// When the run's draft was left for review: what became of it. Of a
    /// run that was reconciled more than once, the latest draft.
    pub draft: Option<DraftStatus>,
}

/// A judgement as a run stored it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RecordedJudgement {
    pub verdict: Verdict,
    /// On the strategy's own scale.
    pub score: Option<f64>,
    /// What the strategy kept with the judgement. The built-in `rules`
    /// keeps the names of the rules that held.
    pub detail: Option<serde_json::Value>,
}

pub(crate) fn explain_pair(
    conn: &Connection,
    manifest: Option<&Manifest>,
    a: RecordId,
    b: RecordId,
) -> Result<Explanation> {
    if a == b {
        return Err(Error::SameRecord(a));
    }
    let (left, right) = if a < b { (a, b) } else { (b, a) };
    let find = |id| lookup::record(conn, id)?.ok_or(Error::UnknownRecord(id));
    let (left, right) = (find(left)?, find(right)?);

    let same_type = left.entity_type == right.entity_type;
    let outcome = match (left.identity, right.identity) {
        _ if !same_type => Outcome::DifferentTypes,
        (Some(one), Some(other)) if one == other => Outcome::SameIdentity,
        (Some(_), Some(_)) => Outcome::DifferentIdentities,
        _ => Outcome::Unassigned,
    };
    let (path, cut, held_by) = match (outcome, left.identity) {
        (Outcome::SameIdentity, Some(identity)) => {
            let graph = Graph::read(conn, identity)?;
            let mut describe = Describe::new(conn, manifest, &left.entity_type, &graph);
            match (graph.number(left.id), graph.number(right.id)) {
                (Some(one), Some(other)) => {
                    let mut path = Vec::new();
                    let mut at = one;
                    for edge in graph.chain(one, other) {
                        let link = describe.link(edge, at)?;
                        at = graph.number(link.to).expect("a member");
                        path.push(link);
                    }
                    // Between the records named as each of the two is, so
                    // that the cut parts two groups rather than one record
                    // from its own.
                    let labels = read_member_labels(conn, identity, &graph)?;
                    let named_as = |member: u32| -> Vec<u32> {
                        let Some(label) = &labels[member as usize] else {
                            return vec![member];
                        };
                        let label = label.to_lowercase();
                        (0..graph.len() as u32)
                            .filter(|&at| {
                                at == member
                                    || labels[at as usize]
                                        .as_ref()
                                        .is_some_and(|other| other.to_lowercase() == label)
                            })
                            .collect()
                    };
                    let (ones, others) = (named_as(one), named_as(other));
                    let parting = graph.cut_between_names(one, other, &ones, &others);
                    let cut = match parting {
                        Some(parting) => Some(describe.split(&parting, &labels)?),
                        None => None,
                    };
                    let mut held_by: Vec<String> = match path.is_empty() {
                        true => Vec::new(),
                        false => (0..graph.rule_names.len() as u32)
                            .filter(|&rule| !graph.together_without(rule, one, other))
                            .map(|rule| graph.rule_names[rule as usize].clone())
                            .collect(),
                    };
                    held_by.sort();
                    (path, cut, held_by)
                }
                _ => (Vec::new(), None, Vec::new()),
            }
        }
        _ => (Vec::new(), None, Vec::new()),
    };
    let mut review_items = Vec::new();
    for id in [left.id, right.id] {
        review_items.extend(review::list(
            conn,
            &ReviewFilter {
                record: Some(id),
                ..ReviewFilter::default()
            },
        )?);
    }
    review_items.sort_by_key(|item| item.id);
    review_items.dedup_by_key(|item| item.id);

    let comparisons = read_comparisons(conn, left.id, right.id)?;
    let constraints = read_pair_constraints(conn, left.id, right.id)?;
    let declared = manifest.and_then(|manifest| manifest.entity_type(left.entity_type.as_str()));
    let table = features::table(&left.entity_type);
    let (left, right) = (explain_record(conn, left)?, explain_record(conn, right)?);
    // Last, because a rule SQL refuses ends the read: see `read_rules`.
    let rules = match declared {
        Some(declared) if same_type => read_rules(
            conn,
            &table,
            &declared.rules,
            left.record.id,
            right.record.id,
        )?,
        _ => None,
    };

    let mut explanation = Explanation {
        comparisons,
        constraints,
        left,
        right,
        outcome,
        summary: Vec::new(),
        rules,
        review_items,
        path,
        cut,
        held_by,
    };
    explanation.summary = pair_summary(&explanation);
    Ok(explanation)
}

fn explain_record(conn: &Connection, record: RecordEntry) -> Result<Explained> {
    let features = feature_row(conn, &features::table(&record.entity_type), record.id)?;

    let mut statement = conn.prepare(
        "SELECT c.seq, c.changeset_id::VARCHAR, c.actor, c.reason, c.run_id,
                epoch_us(c.committed_at)
         FROM spool.membership m
         JOIN spool.changeset c ON c.seq = m.from_seq
         WHERE m.to_seq IS NULL AND m.record_id = ?::UUID",
    )?;
    let mut rows = statement.query([record.id.to_string()])?;
    let placed = match rows.next()? {
        Some(row) => Some(Placement {
            seq: Seq(row.get::<_, i64>(0)? as u64),
            changeset: parse(&row.get::<_, String>(1)?)?,
            actor: parse(&row.get::<_, String>(2)?)?,
            reason: row.get(3)?,
            run: row.get::<_, Option<i64>>(4)?.map(|run| RunId(run as u64)),
            committed_at: format_rfc3339(row.get(5)?),
        }),
        None => None,
    };
    Ok(Explained {
        record,
        features,
        placed,
    })
}

/// Applies each rule to the pair's feature rows. `None` when either row is
/// missing.
///
/// This has to be the last thing a read asks of its connection. A run
/// applies `no_match` and `match` rules only to the pairs a block rule
/// found, so such a rule may rely on what the block rules let through, and
/// SQL may refuse it on a pair they did not. A statement SQL refuses ends
/// the transaction it is in.
fn read_rules(
    conn: &Connection,
    table: &str,
    rules: &RuleSet,
    left: RecordId,
    right: RecordId,
) -> Result<Option<RuleReading>> {
    if !has_table(conn, table)? {
        return Ok(None);
    }
    let apply = |rules: &[&Rule]| -> Result<Option<Vec<RuleResult>>> {
        let predicates: String = rules
            .iter()
            .map(|rule| format!(", (\n{}\n)::BOOLEAN", rule.predicate))
            .collect();
        let mut statement = conn.prepare(&format!(
            "SELECT 1{predicates}
             FROM {table} l, {table} r
             WHERE l.record_id = ?::UUID AND r.record_id = ?::UUID"
        ))?;
        let mut rows = statement.query(params![left.to_string(), right.to_string()])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        let mut results = Vec::with_capacity(rules.len());
        for (index, rule) in rules.iter().enumerate() {
            results.push(RuleResult {
                name: rule.name.clone(),
                predicate: rule.predicate.clone(),
                holds: row.get(index + 1)?,
            });
        }
        Ok(Some(results))
    };

    let Some(block) = apply(&rules.block.iter().collect::<Vec<_>>())? else {
        return Ok(None);
    };
    let judging: Vec<&Rule> = rules.no_match.iter().chain(&rules.match_).collect();
    let (mut no_match, failed) = match apply(&judging) {
        Ok(Some(results)) => (results, None),
        Ok(None) => return Ok(None),
        Err(Error::Duckdb(error)) => {
            let undecided = judging
                .iter()
                .map(|rule| RuleResult {
                    name: rule.name.clone(),
                    predicate: rule.predicate.clone(),
                    holds: None,
                })
                .collect();
            // The first line says what was wrong. The rest points into a
            // statement the caller never saw.
            let said = error.to_string();
            (
                undecided,
                Some(said.lines().next().unwrap_or_default().to_owned()),
            )
        }
        Err(error) => return Err(error),
    };
    let match_ = no_match.split_off(rules.no_match.len());

    let any = |results: &[RuleResult]| results.iter().any(|result| result.holds == Some(true));
    let candidate = any(&block);
    let verdict = match candidate {
        true if any(&no_match) => Some(Verdict::NoMatch),
        true if any(&match_) => Some(Verdict::Match),
        _ => None,
    };
    Ok(Some(RuleReading {
        block,
        no_match,
        match_,
        candidate,
        verdict,
        failed,
    }))
}

fn read_comparisons(conn: &Connection, left: RecordId, right: RecordId) -> Result<Vec<Comparison>> {
    let pair = params![left.to_string(), right.to_string()];
    let mut found: BTreeMap<RunId, Comparison> = BTreeMap::new();

    let mut statement = conn.prepare(
        "SELECT r.run_id, r.status, r.strategy, r.strategy_version, epoch_us(r.started_at),
                (SELECT d.status FROM spool.draft d WHERE d.match_run = r.run_id
                 ORDER BY d.run_id DESC LIMIT 1)
         FROM spool.run r
         WHERE r.run_id IN (
           SELECT run_id FROM spool.candidate WHERE left_id = $1::UUID AND right_id = $2::UUID
           UNION
           SELECT run_id FROM spool.judgement WHERE left_id = $1::UUID AND right_id = $2::UUID)",
    )?;
    let mut rows = statement.query(pair)?;
    while let Some(row) = rows.next()? {
        let run = RunId(row.get::<_, i64>(0)? as u64);
        found.insert(
            run,
            Comparison {
                run,
                status: RunStatus::from_stored(&row.get::<_, String>(1)?)?,
                strategy: row.get::<_, Option<String>>(2)?.map(|name| StrategyRef {
                    name,
                    version: row.get(3).unwrap_or(None),
                }),
                started_at: format_rfc3339(row.get(4)?),
                blocked_by: Vec::new(),
                judgements: Vec::new(),
                draft: row
                    .get::<_, Option<String>>(5)?
                    .as_deref()
                    .map(DraftStatus::from_stored)
                    .transpose()?,
            },
        );
    }

    let mut statement = conn.prepare(
        "SELECT DISTINCT run_id, rule FROM spool.candidate
         WHERE left_id = $1::UUID AND right_id = $2::UUID AND rule IS NOT NULL
         ORDER BY run_id, rule",
    )?;
    let mut rows = statement.query(pair)?;
    while let Some(row) = rows.next()? {
        if let Some(comparison) = found.get_mut(&RunId(row.get::<_, i64>(0)? as u64)) {
            comparison.blocked_by.push(row.get(1)?);
        }
    }

    let mut statement = conn.prepare(
        "SELECT run_id, verdict, score, detail::VARCHAR FROM spool.judgement
         WHERE left_id = $1::UUID AND right_id = $2::UUID
         ORDER BY run_id, verdict, score",
    )?;
    let mut rows = statement.query(pair)?;
    while let Some(row) = rows.next()? {
        if let Some(comparison) = found.get_mut(&RunId(row.get::<_, i64>(0)? as u64)) {
            comparison.judgements.push(RecordedJudgement {
                verdict: row
                    .get::<_, String>(1)?
                    .parse()
                    .map_err(|message: String| StateError::new(message))?,
                score: row.get(2)?,
                detail: row
                    .get::<_, Option<String>>(3)?
                    .map(|detail| serde_json::from_str(&detail).map_err(StateError::new))
                    .transpose()?,
            });
        }
    }
    Ok(found.into_values().collect())
}

fn read_pair_constraints(
    conn: &Connection,
    left: RecordId,
    right: RecordId,
) -> Result<Vec<ConstraintEntry>> {
    let mut statement = conn.prepare(
        "SELECT record_a::VARCHAR, record_b::VARCHAR, kind, from_seq, to_seq
         FROM spool.constraint_link
         WHERE record_a = ?::UUID AND record_b = ?::UUID
         ORDER BY from_seq",
    )?;
    let mut rows = statement.query(params![left.to_string(), right.to_string()])?;
    let mut found = Vec::new();
    while let Some(row) = rows.next()? {
        found.push(ConstraintEntry {
            constraint: store::constraint_from_row(row, 0)?,
            from: Seq(row.get::<_, i64>(3)? as u64),
            to: row.get::<_, Option<i64>>(4)?.map(|seq| Seq(seq as u64)),
        });
    }
    Ok(found)
}
