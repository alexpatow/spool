//! Why two records share an identity, or do not.
//!
//! An explanation gathers what the store holds about a pair: where each
//! record is and which changeset put it there, what every run that
//! looked at the pair concluded, the constraints and review items that name
//! them, and what the project's rules say about the pair as it stands. It
//! decides nothing and changes nothing.
//!
//! Two things in it are computed for the question rather than read back:
//! the reading of the rules, which uses the rules and the feature rows as
//! they are now and so can differ from what a run recorded, and the chain
//! of links between two members of one identity that were never compared
//! with each other.

use std::collections::BTreeMap;

use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};
use spool_core::{
    Actor, ChangesetId, ConstraintEntry, ConstraintKind, EntityType, IdentityId, RecordId, RunId,
    Seq, StateError, Verdict,
};

use crate::drafts::DraftStatus;
use crate::engine::StrategyRef;
use crate::error::{Error, Result};
use crate::features;
use crate::links::{Graph, MOST_CUT, Parting};
use crate::lookup::{self, RecordEntry};
use crate::project::{Manifest, Rule, RuleSet};
use crate::review::{self, ReviewEntry, ReviewFilter, ReviewKind, ReviewStatus};
use crate::runs::RunStatus;
use crate::store::{self, parse};
use crate::time::rfc3339;

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

/// One link between two members of an identity: what joins them directly.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Link {
    pub from: RecordId,
    pub to: RecordId,
    /// `from` and `to` as a person knows them.
    pub from_record: Named,
    pub to_record: Named,
    pub kind: LinkKind,
    /// With `match`: the latest run that judged so.
    pub run: Option<RunId>,
    /// With `match`: the strategy of that run.
    #[serde(default)]
    pub strategy: Option<String>,
    /// With `match`: the `match` rules that held, as the run recorded them.
    /// Empty for a strategy that keeps no rule names.
    #[serde(default)]
    pub rules: Vec<String>,
    /// With `match`: the score the run gave, on its strategy's own scale.
    #[serde(default)]
    pub score: Option<f64>,
    /// The features those rules read, with the values `from` and `to` have
    /// now: what satisfied the rules, unless the features changed since.
    #[serde(default)]
    pub values: Vec<LinkValue>,
}

/// A record as a person knows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Named {
    /// Its value of the feature its type names as the label.
    pub label: Option<String>,
    /// `source:key`, or for a mention `source:document[start-end]`, as
    /// [`RecordEntry::name`] says it.
    pub name: String,
}

/// One feature a rule of a link reads, at both ends of the link.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LinkValue {
    pub feature: String,
    pub from: serde_json::Value,
    pub to: serde_json::Value,
}

/// The fewest links whose removal would part an identity in two, and the
/// two sides.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Split {
    pub links: Vec<Link>,
    /// The two sides: for a pair, `left`'s first; for an identity, the
    /// smaller first. Members no link holds to either are on neither.
    pub sides: Vec<Side>,
}

/// One side of a [`Split`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Side {
    pub records: u64,
    /// Its most common labels, the most common first, at most five.
    pub names: Vec<NameCount>,
}

/// How many records have a label.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct NameCount {
    pub name: String,
    pub records: u64,
}

/// What one rule holds of an identity together.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RuleHold {
    /// A `match` rule, as the runs that judged the links recorded it.
    pub rule: String,
    /// The links it held for, alone or with other rules.
    pub links: u64,
    /// Of those, the links no other rule held for.
    pub alone: u64,
    /// How many pieces the identity would be in without the links only
    /// this rule made: 1 when the other links hold it together.
    pub pieces: u64,
    /// The largest of those pieces, the largest first, at most five.
    pub largest: Vec<Side>,
}

/// What holds an identity together, and where it holds least.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct IdentityExplanation {
    /// The ID that was asked for.
    pub id: IdentityId,
    /// The active identity it stands for: itself, or the one it was merged
    /// into. Everything below is of this one.
    pub current: IdentityId,
    pub entity_type: EntityType,
    pub label: Option<String>,
    pub records: u64,
    /// Different labels its records have, whatever their case. Records
    /// without a label are not counted.
    pub names: u64,
    /// Its most common labels, the most common first, at most five.
    pub top_names: Vec<NameCount>,
    /// Pairs of its records, `records × (records - 1) / 2`.
    pub pairs: u64,
    /// Pairs a link joins directly.
    pub linked_pairs: u64,
    /// Pairs that are together only through a chain of other records: no
    /// link joins them, and a chain does.
    pub chained_pairs: u64,
    /// Groups of records that links hold together. More than one when
    /// what joined them has been removed since, or a changeset placed a
    /// record directly.
    pub parts: u64,
    /// The most links a chain has between two records of the largest part,
    /// at the least: found from two of its records far apart, not from
    /// every pair.
    pub longest_chain: u64,
    /// The places where fewest links hold it together, where a wrong merge
    /// entered: each a set of links whose removal would part it, with the
    /// two sides, the smaller first. They are the links that alone hold a
    /// part on, and the fewest that part the records of its most common
    /// name from those of each of the next eight (or, without names, its
    /// two records farthest apart). Fewest links for each record they part
    /// off first; at most five, and none of more than twenty links.
    pub weakest: Vec<Split>,
    /// For each `match` rule its links were made by, what it holds
    /// together: the pieces the identity would be in without the links only
    /// that rule made. A rule that holds many pieces together is the one
    /// that chains different things. The rule that holds most pieces
    /// together first.
    pub rules: Vec<RuleHold>,
    /// The explanation in sentences.
    pub summary: Vec<String>,
}

/// What joins two records of one identity directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    /// A `must_link` constraint in force.
    MustLink,
    /// One is a later version of the other: the same row of the same
    /// source, sent again with other content.
    Version,
    /// A run that finished judged them `match`.
    Match,
}

pub(crate) fn explain(
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
                    let labels = labels(conn, identity, &graph)?;
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

    let comparisons = comparisons(conn, left.id, right.id)?;
    let constraints = constraints(conn, left.id, right.id)?;
    let declared = manifest.and_then(|manifest| manifest.entity_type(left.entity_type.as_str()));
    let table = features::table(&left.entity_type);
    let (left, right) = (explained(conn, left)?, explained(conn, right)?);
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
    explanation.summary = summary(&explanation);
    Ok(explanation)
}

fn explained(conn: &Connection, record: RecordEntry) -> Result<Explained> {
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
            committed_at: rfc3339(row.get(5)?),
        }),
        None => None,
    };
    Ok(Explained {
        record,
        features,
        placed,
    })
}

/// Whether a feature table exists. A store read without its manifest can
/// hold records of a type whose table is not there.
fn has_table(conn: &Connection, table: &str) -> Result<bool> {
    let name = table.rsplit('.').next().unwrap_or(table);
    let found: i64 = conn.query_row(
        "SELECT count(*) FROM information_schema.tables
         WHERE table_schema = 'spool' AND table_name = ?",
        [name],
        |row| row.get(0),
    )?;
    Ok(found > 0)
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

fn comparisons(conn: &Connection, left: RecordId, right: RecordId) -> Result<Vec<Comparison>> {
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
                started_at: rfc3339(row.get(4)?),
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

fn constraints(conn: &Connection, left: RecordId, right: RecordId) -> Result<Vec<ConstraintEntry>> {
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
            constraint: store::constraint(row, 0)?,
            from: Seq(row.get::<_, i64>(3)? as u64),
            to: row.get::<_, Option<i64>>(4)?.map(|seq| Seq(seq as u64)),
        });
    }
    Ok(found)
}

/// The most labels a side or an identity is described by.
const TOP_NAMES: usize = 5;

/// The most splits an identity's explanation names.
const WEAKEST: usize = 5;

/// How many names after its most common an identity is cut from.
const NAME_CUTS: usize = 8;

/// What the links of one identity are, in words a person reads: each
/// record by its label and name, and each `match` by the rules that held
/// and the values they read. What it reads is kept for the next link.
struct Describe<'a> {
    conn: &'a Connection,
    graph: &'a Graph,
    table: String,
    /// The `match` rules of the type, by name, when the store was applied
    /// with the manifest the engine has: only then are the feature rows
    /// those the rules read.
    rules: BTreeMap<&'a str, &'a str>,
    named: BTreeMap<RecordId, Named>,
    rows: BTreeMap<RecordId, Option<serde_json::Map<String, serde_json::Value>>>,
}

impl<'a> Describe<'a> {
    fn new(
        conn: &'a Connection,
        manifest: Option<&'a Manifest>,
        entity_type: &EntityType,
        graph: &'a Graph,
    ) -> Self {
        let rules = manifest
            .and_then(|manifest| manifest.entity_type(entity_type.as_str()))
            .map(|declared| {
                declared
                    .rules
                    .match_
                    .iter()
                    .map(|rule| (rule.name.as_str(), rule.predicate.as_str()))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            conn,
            graph,
            table: features::table(entity_type),
            rules,
            named: BTreeMap::new(),
            rows: BTreeMap::new(),
        }
    }

    fn named(&mut self, id: RecordId) -> Result<Named> {
        if let Some(named) = self.named.get(&id) {
            return Ok(named.clone());
        }
        let record = lookup::record(self.conn, id)?.ok_or(Error::UnknownRecord(id))?;
        let named = Named {
            name: record.name(),
            label: record.label,
        };
        self.named.insert(id, named.clone());
        Ok(named)
    }

    fn row(&mut self, id: RecordId) -> Result<Option<serde_json::Map<String, serde_json::Value>>> {
        if !self.rows.contains_key(&id) {
            let row = feature_row(self.conn, &self.table, id)?;
            self.rows.insert(id, row);
        }
        Ok(self.rows[&id].clone())
    }

    /// The link of an edge, from the member numbered `from` to the other.
    fn link(&mut self, edge: u32, from: u32) -> Result<Link> {
        let found = self.graph.edges[edge as usize];
        let to = if found.a == from { found.b } else { found.a };
        let (from, to) = (
            self.graph.members[from as usize],
            self.graph.members[to as usize],
        );
        let mut link = Link {
            from,
            to,
            from_record: self.named(from)?,
            to_record: self.named(to)?,
            kind: found.kind,
            run: found.run,
            strategy: None,
            rules: Vec::new(),
            score: None,
            values: Vec::new(),
        };
        if let (LinkKind::Match, Some(run)) = (found.kind, found.run) {
            let (left, right) = (from.min(to), from.max(to));
            let mut statement = self.conn.prepare(
                "SELECT r.strategy, j.score, j.detail::VARCHAR
                 FROM spool.judgement j
                 JOIN spool.run r ON r.run_id = j.run_id
                 WHERE j.run_id = ? AND j.left_id = ?::UUID AND j.right_id = ?::UUID
                   AND j.verdict = 'match'
                 LIMIT 1",
            )?;
            let mut rows =
                statement.query(params![run.0 as i64, left.to_string(), right.to_string()])?;
            if let Some(row) = rows.next()? {
                link.strategy = row.get(0)?;
                link.score = row.get(1)?;
                let detail: Option<serde_json::Value> = row
                    .get::<_, Option<String>>(2)?
                    .map(|detail| serde_json::from_str(&detail).map_err(StateError::new))
                    .transpose()?;
                link.rules = detail
                    .as_ref()
                    .and_then(|detail| detail.get("match"))
                    .and_then(serde_json::Value::as_array)
                    .map(|names| {
                        names
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default();
            }
        }
        let mut read: Vec<String> = Vec::new();
        for rule in &link.rules {
            if let Some(predicate) = self.rules.get(rule.as_str()) {
                for feature in features_read(predicate) {
                    if !read.contains(&feature) {
                        read.push(feature);
                    }
                }
            }
        }
        if !read.is_empty() {
            let (one, other) = (self.row(from)?, self.row(to)?);
            if let (Some(one), Some(other)) = (one, other) {
                for feature in read {
                    if let (Some(from), Some(to)) = (one.get(&feature), other.get(&feature)) {
                        link.values.push(LinkValue {
                            feature,
                            from: from.clone(),
                            to: to.clone(),
                        });
                    }
                }
            }
        }
        Ok(link)
    }

    /// A parting in words. Each of its links runs from the first side to
    /// the second.
    fn split(&mut self, parting: &Parting, labels: &[Option<String>]) -> Result<Split> {
        let one: std::collections::HashSet<u32> = parting.one.iter().copied().collect();
        let mut links = Vec::new();
        for &edge in &parting.edges {
            let found = self.graph.edges[edge as usize];
            let from = if one.contains(&found.a) {
                found.a
            } else {
                found.b
            };
            links.push(self.link(edge, from)?);
        }
        Ok(Split {
            links,
            sides: vec![side(&parting.one, labels), side(&parting.other, labels)],
        })
    }
}

/// An entity type's feature row of a record, by column name, without the
/// two columns the engine adds. `None` when it has none.
fn feature_row(
    conn: &Connection,
    table: &str,
    id: RecordId,
) -> Result<Option<serde_json::Map<String, serde_json::Value>>> {
    if !has_table(conn, table)? {
        return Ok(None);
    }
    // The whole row: a table may have no columns but the two the engine
    // adds, and a query has to select something.
    let mut statement = conn.prepare(&format!(
        "SELECT to_json(f)::VARCHAR FROM {table} f WHERE f.record_id = ?::UUID"
    ))?;
    let mut rows = statement.query([id.to_string()])?;
    match rows.next()? {
        Some(row) => {
            let mut features: serde_json::Map<String, serde_json::Value> =
                serde_json::from_str(&row.get::<_, String>(0)?).map_err(StateError::new)?;
            features.remove("record_id");
            features.remove("declaration_version");
            Ok(Some(features))
        }
        None => Ok(None),
    }
}

/// The features a rule reads: each `l.<name>` and `r.<name>` in its SQL,
/// in the order they first appear. A name in quotes is read too. What else
/// the SQL does with them is not looked at, so a rule that compares a
/// feature only in a function still names it.
pub(crate) fn features_read(predicate: &str) -> Vec<String> {
    let bytes = predicate.as_bytes();
    let word = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    let mut found: Vec<String> = Vec::new();
    let mut at = 0;
    while at + 2 < bytes.len() {
        let starts = matches!(bytes[at], b'l' | b'r' | b'L' | b'R')
            && bytes[at + 1] == b'.'
            && (at == 0 || !word(bytes[at - 1]));
        if !starts {
            at += 1;
            continue;
        }
        let begin = at + 2;
        let (name, end) = if bytes[begin] == b'"' {
            match predicate[begin + 1..].find('"') {
                Some(length) => (
                    &predicate[begin + 1..begin + 1 + length],
                    begin + 2 + length,
                ),
                None => break,
            }
        } else {
            let length = bytes[begin..]
                .iter()
                .take_while(|&&byte| word(byte))
                .count();
            (&predicate[begin..begin + length], begin + length)
        };
        if !name.is_empty() && !found.iter().any(|known| known == name) {
            found.push(name.to_owned());
        }
        at = end.max(at + 1);
    }
    found
}

/// The label of each member of an identity, by its number in the graph.
fn labels(conn: &Connection, identity: IdentityId, graph: &Graph) -> Result<Vec<Option<String>>> {
    let mut labels = vec![None; graph.len()];
    let mut statement = conn.prepare(
        "SELECT m.record_id::VARCHAR, named.label
         FROM spool.membership m
         JOIN spool.record_label named ON named.record_id = m.record_id
         WHERE m.to_seq IS NULL AND m.identity_id = ?::UUID AND named.label IS NOT NULL",
    )?;
    let mut rows = statement.query([identity.to_string()])?;
    while let Some(row) = rows.next()? {
        let id: RecordId = parse(&row.get::<_, String>(0)?)?;
        if let Some(at) = graph.number(id) {
            labels[at as usize] = row.get(1)?;
        }
    }
    Ok(labels)
}

/// The labels of some members, the most common first, as many as `most`.
/// Labels that differ only in case are one, written as the most common
/// spelling has it.
fn common_names(members: &[u32], labels: &[Option<String>], most: usize) -> (u64, Vec<NameCount>) {
    let mut counted: BTreeMap<String, BTreeMap<&str, u64>> = BTreeMap::new();
    for &member in members {
        if let Some(label) = &labels[member as usize] {
            *counted
                .entry(label.to_lowercase())
                .or_default()
                .entry(label.as_str())
                .or_default() += 1;
        }
    }
    let distinct = counted.len() as u64;
    let mut names: Vec<NameCount> = counted
        .into_values()
        .map(|spellings| {
            let records = spellings.values().sum();
            let (name, _) = spellings
                .iter()
                .max_by_key(|(spelling, count)| (**count, std::cmp::Reverse(**spelling)))
                .expect("a label is counted");
            NameCount {
                name: (*name).to_owned(),
                records,
            }
        })
        .collect();
    names.sort_by(|one, other| {
        other
            .records
            .cmp(&one.records)
            .then_with(|| one.name.cmp(&other.name))
    });
    names.truncate(most);
    (distinct, names)
}

/// What each rule holds together. See [`IdentityExplanation::rules`].
fn rule_holds(graph: &Graph, labels: &[Option<String>]) -> Vec<RuleHold> {
    let mut holds = Vec::new();
    for (rule, name) in graph.rule_names.iter().enumerate() {
        let rule = rule as u32;
        let (mut links, mut alone) = (0, 0);
        for edge in &graph.edges {
            let rules = &graph.rule_sets[edge.rules as usize];
            if rules.contains(&rule) {
                links += 1;
                if rules.len() == 1 {
                    alone += 1;
                }
            }
        }
        let mut pieces = graph.pieces_without(rule);
        let count = pieces.len() as u64;
        pieces.truncate(TOP_NAMES);
        holds.push(RuleHold {
            rule: name.clone(),
            links,
            alone,
            pieces: count,
            largest: pieces.iter().map(|piece| side(piece, labels)).collect(),
        });
    }
    holds.sort_by(|one, other| {
        other
            .pieces
            .cmp(&one.pieces)
            .then_with(|| other.links.cmp(&one.links))
            .then_with(|| one.rule.cmp(&other.rule))
    });
    holds
}

/// The members of each label, whatever its case, the most numerous first.
fn name_groups(labels: &[Option<String>]) -> Vec<Vec<u32>> {
    let mut groups: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    for (at, label) in labels.iter().enumerate() {
        if let Some(label) = label {
            groups
                .entry(label.to_lowercase())
                .or_default()
                .push(at as u32);
        }
    }
    let mut groups: Vec<Vec<u32>> = groups.into_values().collect();
    groups.sort_by_key(|members| std::cmp::Reverse(members.len()));
    groups
}

fn side(members: &[u32], labels: &[Option<String>]) -> Side {
    Side {
        records: members.len() as u64,
        names: common_names(members, labels, TOP_NAMES).1,
    }
}

pub(crate) fn explain_identity(
    conn: &Connection,
    manifest: Option<&Manifest>,
    id: IdentityId,
) -> Result<IdentityExplanation> {
    let view = lookup::identity(conn, id)?.ok_or(Error::UnknownIdentity(id))?;
    let graph = Graph::read(conn, view.current)?;
    let labels = labels(conn, view.current, &graph)?;
    let everyone: Vec<u32> = (0..graph.len() as u32).collect();
    let (names, top_names) = common_names(&everyone, &labels, TOP_NAMES);
    let records = graph.len() as u64;
    let parts = graph.parts();
    let together: u64 = parts
        .iter()
        .map(|part| {
            let size = part.len() as u64;
            size * size.saturating_sub(1) / 2
        })
        .sum();
    let linked_pairs = graph.linked_pairs();
    let far = parts.first().and_then(|part| graph.far_apart(part));

    // Where a wrong merge entered: the links that alone hold a part on,
    // and the fewest that part the records of its most common name from
    // those of each of the next. Without names, the fewest that part the
    // two records of the largest part farthest apart.
    let mut partings = graph.bridges(WEAKEST);
    let groups = name_groups(&labels);
    let mut cuts = Vec::new();
    match groups.split_first() {
        Some((most, next)) if !next.is_empty() => {
            for group in next.iter().take(NAME_CUTS) {
                cuts.extend(graph.cut(most, group));
            }
        }
        _ => {
            if let Some((one, other, _)) = far {
                cuts.extend(graph.cut(&[one], &[other]));
            }
        }
    }
    for cut in cuts {
        let (one, other) = match cut.one.len() <= cut.other.len() {
            true => (cut.one, cut.other),
            false => (cut.other, cut.one),
        };
        partings.push(Parting {
            edges: cut.edges,
            one,
            other,
        });
    }
    for parting in &mut partings {
        parting.edges.sort();
    }
    // Fewest links for each record they part off first: two links that
    // part five records are weaker than sixteen that part seven, and than
    // one that parts one.
    partings.sort_by(|one, other| {
        (one.edges.len() * other.one.len())
            .cmp(&(other.edges.len() * one.one.len()))
            .then_with(|| other.one.len().cmp(&one.one.len()))
            .then_with(|| one.edges.cmp(&other.edges))
    });
    partings.dedup_by(|one, other| one.edges == other.edges);
    partings.truncate(WEAKEST);

    let mut describe = Describe::new(conn, manifest, &view.entity_type, &graph);
    let mut weakest = Vec::new();
    for parting in &partings {
        weakest.push(describe.split(parting, &labels)?);
    }
    let rules = rule_holds(&graph, &labels);

    let mut explanation = IdentityExplanation {
        id,
        current: view.current,
        entity_type: view.entity_type,
        label: view.label,
        records,
        names,
        top_names,
        pairs: records * records.saturating_sub(1) / 2,
        linked_pairs,
        chained_pairs: together.saturating_sub(linked_pairs),
        parts: parts.len() as u64,
        longest_chain: far.map_or(0, |(_, _, steps)| u64::from(steps)),
        weakest,
        rules,
        summary: Vec::new(),
    };
    explanation.summary = identity_summary(&explanation);
    Ok(explanation)
}

/// A record in a sentence: its label and its name, or its name alone.
fn called(named: &Named) -> String {
    match &named.label {
        Some(label) => format!("`{label}` ({})", named.name),
        None => format!("`{}`", named.name),
    }
}

/// How a link joins its two records, in words.
pub fn how_linked(link: &Link) -> String {
    let mut how = match (link.kind, link.run) {
        (LinkKind::MustLink, _) => "a `must_link` constraint".to_owned(),
        (LinkKind::Version, _) => "a version of the same row".to_owned(),
        (LinkKind::Match, Some(run)) => format!("`match` in run {run}"),
        (LinkKind::Match, None) => "`match`".to_owned(),
    };
    if !link.rules.is_empty() {
        how.push_str(&format!(
            ", {} {}",
            rule_or_rules(&link.rules.iter().map(String::as_str).collect::<Vec<_>>()),
            listed(link.rules.iter().map(String::as_str))
        ));
    } else if let Some(score) = link.score {
        how.push_str(&format!(", score {score}"));
    }
    let values: Vec<String> = link.values.iter().map(value_said).collect();
    if !values.is_empty() {
        how.push_str(&format!(": {}", values.join(", ")));
    }
    how
}

/// A feature's values at the two ends of a link: once when they are the
/// same, and both when they are not.
fn value_said(value: &LinkValue) -> String {
    let plain = |value: &serde_json::Value| match value {
        serde_json::Value::Null => "null".to_owned(),
        other => other.to_string(),
    };
    match value.from == value.to {
        true => format!("{} {}", value.feature, plain(&value.from)),
        false => format!(
            "{} {} and {}",
            value.feature,
            plain(&value.from),
            plain(&value.to)
        ),
    }
}

fn names_said(side: &Side) -> String {
    let names: Vec<String> = side
        .names
        .iter()
        .map(|name| format!("{} × `{}`", name.records, name.name))
        .collect();
    match names.is_empty() {
        true => String::new(),
        false => format!(" ({})", names.join(", ")),
    }
}

fn links_count(count: usize) -> String {
    match count {
        1 => "1 link".to_owned(),
        count => format!("{count} links"),
    }
}

fn records_count(count: u64) -> String {
    match count {
        1 => "1 record".to_owned(),
        count => format!("{count} records"),
    }
}

fn identity_summary(explanation: &IdentityExplanation) -> Vec<String> {
    let mut said = Vec::new();
    let called = match &explanation.label {
        Some(label) => format!("`{label}`"),
        None => format!("Identity {}", explanation.current),
    };
    if explanation.current != explanation.id {
        said.push(format!(
            "Identity {} was merged into {}, which is explained here.",
            explanation.id, explanation.current
        ));
    }
    said.push(format!(
        "{called} is a `{}` identity of {} with {} different {}.",
        explanation.entity_type,
        records_count(explanation.records),
        explanation.names,
        if explanation.names == 1 {
            "name"
        } else {
            "names"
        },
    ));
    if explanation.pairs > 0 {
        said.push(format!(
            "Of its {} pairs of records, {} are linked directly and {} are together only through a chain of others{}.",
            explanation.pairs,
            explanation.linked_pairs,
            explanation.chained_pairs,
            match explanation.longest_chain {
                0 | 1 => String::new(),
                steps => format!(", the longest of at least {steps} links"),
            }
        ));
    }
    if explanation.parts > 1 {
        said.push(format!(
            "No chain of matches, constraints or versions holds all of it together: it is in {} parts. What joined them has since been removed, or a changeset placed records directly.",
            explanation.parts
        ));
    }
    for hold in &explanation.rules {
        if hold.pieces < 2 {
            continue;
        }
        let pieces: Vec<String> = hold
            .largest
            .iter()
            .map(|piece| format!("{}{}", piece.records, names_said(piece)))
            .collect();
        let more = match hold.pieces as usize - hold.largest.len() {
            0 => String::new(),
            1 => " and 1 more".to_owned(),
            count => format!(" and {count} more"),
        };
        said.push(format!(
            "Without the {} only rule `{}` made, it would fall into {} pieces: {}{more}.",
            links_count(hold.alone as usize),
            hold.rule,
            hold.pieces,
            pieces.join(", "),
        ));
    }
    match explanation.weakest.first() {
        None if explanation.records > 1 => said.push(format!(
            "No single link holds a part of it on, and more than {MOST_CUT} links hold its two records farthest apart together."
        )),
        None => {}
        Some(_) => {
            for split in &explanation.weakest {
                let [small, large] = &split.sides[..] else {
                    continue;
                };
                said.push(format!(
                    "Removing {} would part {}{} from the other {}{}.",
                    links_count(split.links.len()),
                    records_count(small.records),
                    names_said(small),
                    large.records,
                    names_said(large),
                ));
            }
        }
    }
    said
}

fn listed<'a>(names: impl IntoIterator<Item = &'a str>) -> String {
    names
        .into_iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn holding(results: &[RuleResult]) -> Vec<&str> {
    results
        .iter()
        .filter(|result| result.holds == Some(true))
        .map(|result| result.name.as_str())
        .collect()
}

fn rule_or_rules(names: &[&str]) -> &'static str {
    if names.len() == 1 { "rule" } else { "rules" }
}

#[expect(clippy::too_many_lines)]
fn summary(explanation: &Explanation) -> Vec<String> {
    let (left, right) = (&explanation.left.record, &explanation.right.record);
    // Two versions of one row have the same source and key.
    let (l, r) = match (left.name(), right.name()) {
        (l, r) if l == r => (left.id.to_string(), right.id.to_string()),
        names => names,
    };
    let name_of = |id: RecordId| if id == left.id { &l } else { &r };
    let mut said = Vec::new();

    match (explanation.outcome, left.identity, right.identity) {
        (Outcome::DifferentTypes, _, _) => {
            said.push(format!(
                "`{l}` is a `{}` and `{r}` is a `{}`. Records of different entity types are never compared.",
                left.entity_type, right.entity_type
            ));
        }
        (Outcome::SameIdentity, Some(identity), _) => {
            said.push(format!(
                "`{l}` and `{r}` belong to the same identity, {identity}."
            ));
        }
        (_, Some(one), Some(other)) => {
            said.push(format!(
                "`{l}` belongs to identity {one} and `{r}` to identity {other}."
            ));
        }
        (_, None, None) => said.push(format!("Neither `{l}` nor `{r}` has an identity.")),
        (_, Some(identity), None) => said.push(format!(
            "`{l}` belongs to identity {identity}. `{r}` has no identity."
        )),
        (_, None, Some(identity)) => said.push(format!(
            "`{l}` has no identity. `{r}` belongs to identity {identity}."
        )),
    }

    for record in [left, right] {
        if let Some(run) = record.retracted {
            said.push(format!(
                "`{}` was taken back by its source in run {run}: it was taken out of its identity, and no run examines it until the source sends the row again.",
                name_of(record.id)
            ));
        }
    }

    for (later, earlier) in [(right, left), (left, right)] {
        if later.supersedes == Some(earlier.id) {
            said.push(format!(
                "`{}` is a later version of `{}`: the source sent the row again under the same key with other content. Versions of one row are kept together unless a constraint says otherwise.",
                name_of(later.id),
                name_of(earlier.id)
            ));
        }
    }

    for record in &explanation.constraints {
        let kind = record.constraint.kind.as_str();
        said.push(match (record.to, record.constraint.kind) {
            (Some(to), _) => format!(
                "A `{kind}` constraint between them was in force from log position {} until position {to}.",
                record.from
            ),
            (None, ConstraintKind::MustLink) => format!(
                "A `must_link` constraint committed at log position {} says they belong together, whatever a strategy judges.",
                record.from
            ),
            (None, ConstraintKind::CannotLink) => format!(
                "A `cannot_link` constraint committed at log position {} keeps them apart, whatever a strategy judges.",
                record.from
            ),
        });
    }

    match explanation.comparisons.split_last() {
        None if explanation.outcome != Outcome::DifferentTypes => {
            said.push("No run has compared them.".to_owned());
        }
        None => {}
        Some((latest, earlier)) => {
            said.push(compared(latest));
            match earlier.len() {
                0 => {}
                1 => said.push("One earlier run compared them as well.".to_owned()),
                count => said.push(format!("{count} earlier runs compared them as well.")),
            }
        }
    }

    if explanation.outcome == Outcome::SameIdentity {
        match explanation.path.as_slice() {
            [] => said.push(
                "No chain of matches, constraints or versions joins them within the identity as it is now. Either what joined them has since been removed, or a changeset placed one of them directly."
                    .to_owned(),
            ),
            [_] => {}
            chain => {
                let mut steps = format!("`{}`", name_of(left.id));
                for link in chain {
                    let to = match link.to {
                        id if id == right.id => format!("`{r}`"),
                        _ => called(&link.to_record),
                    };
                    steps.push_str(&format!(" → {to} ({})", how_linked(link)));
                }
                let between = chain.len() - 1;
                said.push(format!(
                    "Nothing links the two directly. They are joined through {between} other {}: {steps}.",
                    if between == 1 { "member" } else { "members" }
                ));
            }
        }
        for rule in explanation
            .held_by
            .iter()
            .filter(|_| explanation.path.len() > 1)
        {
            said.push(format!(
                "Without the links only rule `{rule}` made, they would not be together."
            ));
        }
        // A link of their own parts nothing that the sentence above has
        // not said.
        match &explanation.cut {
            _ if explanation.path.len() < 2 => {}
            Some(cut) => {
                if let [with_left, with_right] = &cut.sides[..] {
                    said.push(format!(
                        "Removing {} would part them, leaving {} with `{l}`{} and {} with `{r}`{}.",
                        links_count(cut.links.len()),
                        records_count(with_left.records),
                        names_said(with_left),
                        records_count(with_right.records),
                        names_said(with_right),
                    ));
                }
            }
            None => said.push(format!(
                "More than {MOST_CUT} links hold them together, so no few links part them."
            )),
        }
    }

    match (&explanation.left.placed, &explanation.right.placed) {
        (Some(one), Some(other)) if one.seq == other.seq => {
            said.push(format!("Both were placed {}.", placed(one)));
        }
        (one, other) => {
            for (name, placement) in [(&l, one), (&r, other)] {
                if let Some(placement) = placement {
                    said.push(format!("`{name}` was placed {}.", placed(placement)));
                }
            }
        }
    }

    for item in &explanation.review_items {
        let named: Vec<&str> = [left.id, right.id]
            .into_iter()
            .filter(|id| item.records.contains(id))
            .map(|id| name_of(id).as_str())
            .collect();
        let named = listed(named);
        let kind = item.kind.as_str();
        // In a `possible_split`, whether the two are in different groups:
        // then the item asks about this very pair.
        let group_of = |id| item.groups.iter().position(|group| group.contains(&id));
        let asked_about = matches!(
            (group_of(left.id), group_of(right.id)),
            (Some(one), Some(other)) if one != other
        );
        match (item.status, &item.dismissed) {
            (ReviewStatus::Open, _) => said.push(format!(
                "Review item {} (`{kind}`) is open and names {named}: {}",
                item.id,
                match item.kind {
                    ReviewKind::PossibleMerge =>
                        "what it matched belongs to more than one identity, and merging identities is left to a reviewer.",
                    ReviewKind::Conflict =>
                        "the evidence about it contradicts itself, which is left to a reviewer.",
                    ReviewKind::PossibleSplit if asked_about =>
                        "they share an identity, and what held them together went through a record that its source took back. Nothing the store knows says they are one thing now. Whether to split the identity is left to a reviewer.",
                    ReviewKind::PossibleSplit =>
                        "the identity lost a record that its source took back, and nothing the store knows holds all of the records that are left together. Whether to split the identity is left to a reviewer.",
                }
            )),
            (ReviewStatus::Dismissed, Some(dismissal))
                if item.kind == ReviewKind::PossibleSplit =>
            {
                said.push(format!(
                    "Review item {} (`{kind}`), which names {named}, was dismissed by `{}`: it asked whether to split their identity, and the reviewer left it as it is.",
                    item.id, dismissal.by
                ));
            }
            (ReviewStatus::Dismissed, Some(dismissal)) => said.push(format!(
                "Review item {} (`{kind}`), which names {named}, was dismissed by `{}`. No run looks at a dismissed item's records again.",
                item.id, dismissal.by
            )),
            _ => {}
        }
    }

    if let Some(rules) = &explanation.rules {
        said.push(read(rules));
        if let Some(run) = read_otherwise(&explanation.comparisons, rules) {
            said.push(format!(
                "That is not what run {run} recorded, so the rules or the features have changed since."
            ));
        }
        match (explanation.outcome, rules.verdict) {
            (Outcome::DifferentIdentities, Some(Verdict::Match)) => said.push(
                "Both already have an identity, and a run compares a pair only when one of the two is new to it. A rematch compares every pair and lists a pair like this one as a suggested merge."
                    .to_owned(),
            ),
            (Outcome::SameIdentity, Some(Verdict::NoMatch)) => said.push(
                "No run takes an identity apart. A rematch lists a pair like this one as a suggested split."
                    .to_owned(),
            ),
            _ => {}
        }
    }
    said
}

fn placed(placement: &Placement) -> String {
    let mut details = Vec::new();
    if let Some(run) = placement.run {
        details.push(format!("reconcile run {run}"));
    }
    if let Some(reason) = &placement.reason {
        details.push(format!("\"{reason}\""));
    }
    let details = match details.is_empty() {
        true => String::new(),
        false => format!(" ({})", details.join(", ")),
    };
    format!(
        "at log position {} by `{}`{details}",
        placement.seq, placement.actor
    )
}

fn compared(comparison: &Comparison) -> String {
    let strategy = match &comparison.strategy {
        Some(strategy) => format!(" (strategy `{}`)", strategy.name),
        None => String::new(),
    };
    let mut text = match comparison.judgements.as_slice() {
        [] => format!(
            "Run {}{strategy} made them a candidate pair (block {} {}) and gave no judgement.",
            comparison.run,
            rule_or_rules(
                &comparison
                    .blocked_by
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
            ),
            listed(comparison.blocked_by.iter().map(String::as_str)),
        ),
        judgements => {
            let verdicts: Vec<String> = judgements
                .iter()
                .map(|judgement| {
                    let mut verdict = format!("`{}`", judgement.verdict.as_str());
                    if let Some(score) = judgement.score {
                        verdict.push_str(&format!(" with score {score}"));
                    }
                    let held = |group: &str| -> Vec<&str> {
                        judgement
                            .detail
                            .as_ref()
                            .and_then(|detail| detail.get(group))
                            .and_then(serde_json::Value::as_array)
                            .map(|names| {
                                names.iter().filter_map(serde_json::Value::as_str).collect()
                            })
                            .unwrap_or_default()
                    };
                    let group = match judgement.verdict {
                        Verdict::Match => "match",
                        Verdict::NoMatch => "no_match",
                        Verdict::Unsure => "",
                    };
                    let names = held(group);
                    if !names.is_empty() {
                        verdict.push_str(&format!(
                            " ({group} {} {} held)",
                            rule_or_rules(&names),
                            listed(names.iter().copied())
                        ));
                    }
                    verdict
                })
                .collect();
            format!(
                "Run {}{strategy} judged them {}.",
                comparison.run,
                verdicts.join(" and ")
            )
        }
    };
    match (comparison.status, comparison.draft) {
        (RunStatus::Failed, _) => text.push_str(" The run failed, so nothing came of it."),
        (RunStatus::Open | RunStatus::Running, _) => {
            text.push_str(" The run has not been reconciled.");
        }
        (_, Some(DraftStatus::Open)) => text.push_str(" Its draft waits for a decision."),
        (_, Some(DraftStatus::Rejected)) => text.push_str(" Its draft was rejected."),
        (_, Some(DraftStatus::Superseded)) => {
            text.push_str(" Its draft was superseded by a later run before it was committed.");
        }
        _ => {}
    }
    text
}

/// The latest run of the rules whose record of the pair is not what the
/// rules say of it now.
fn read_otherwise(comparisons: &[Comparison], rules: &RuleReading) -> Option<RunId> {
    let recorded = comparisons.iter().rev().find(|comparison| {
        matches!(comparison.status, RunStatus::Succeeded | RunStatus::Drafted)
            && comparison
                .strategy
                .as_ref()
                .is_some_and(|strategy| strategy.name == StrategyRef::RULES)
    })?;
    let judged = recorded
        .judgements
        .last()
        .map(|judgement| judgement.verdict);
    (!rules.candidate || judged != rules.verdict).then_some(recorded.run)
}

fn read(rules: &RuleReading) -> String {
    let block = holding(&rules.block);
    if rules.block.is_empty() {
        return "The entity type has no block rule, so a run of the rules would not compare them."
            .to_owned();
    }
    if block.is_empty() {
        let undecided: Vec<&str> = rules
            .block
            .iter()
            .filter(|result| result.holds.is_none())
            .map(|result| result.name.as_str())
            .collect();
        let mut text =
            "By the rules as they are now, no block rule holds for the pair, so a run would not compare them."
                .to_owned();
        if !undecided.is_empty() {
            text.push_str(&format!(
                " Block {} {} cannot be decided, because a feature it reads is null.",
                rule_or_rules(&undecided),
                listed(undecided.iter().copied())
            ));
        }
        return text;
    }
    let found = format!(
        "By the rules as they are now, block {} {} {} them a candidate pair",
        rule_or_rules(&block),
        listed(block.iter().copied()),
        if block.len() == 1 { "makes" } else { "make" }
    );
    let decided = |group: &str, results: &[RuleResult]| {
        let names = holding(results);
        format!(
            "{found} and {group} {} {} {}, so they would be judged `{group}`.",
            rule_or_rules(&names),
            listed(names.iter().copied()),
            if names.len() == 1 { "holds" } else { "hold" }
        )
    };
    if let Some(failed) = &rules.failed {
        return format!(
            "{found}, and the `no_match` and `match` rules cannot be applied to the pair's values, so a run of the rules that compared them would fail ({failed})."
        );
    }
    match rules.verdict {
        Some(Verdict::NoMatch) => decided("no_match", &rules.no_match),
        Some(Verdict::Match) => decided("match", &rules.match_),
        _ => format!(
            "{found} and no `no_match` or `match` rule holds, so they would get no judgement."
        ),
    }
}
