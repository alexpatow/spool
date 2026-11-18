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

use std::collections::{BTreeMap, VecDeque};

use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};
use spool_core::{
    Actor, ChangesetId, ConstraintEntry, ConstraintKind, IdentityId, RecordId, RunId, Seq,
    StateError, Verdict,
};

use crate::drafts::DraftStatus;
use crate::engine::StrategyRef;
use crate::error::{Error, Result};
use crate::features;
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

/// One step in the chain that joins two members of an identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Link {
    pub from: RecordId,
    pub to: RecordId,
    pub kind: LinkKind,
    /// With `match`: the latest run that judged so.
    pub run: Option<RunId>,
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
    let path = match (outcome, left.identity) {
        (Outcome::SameIdentity, Some(identity)) => path(conn, identity, left.id, right.id)?,
        _ => Vec::new(),
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
    };
    explanation.summary = summary(&explanation);
    Ok(explanation)
}

fn explained(conn: &Connection, record: RecordEntry) -> Result<Explained> {
    let table = features::table(&record.entity_type);
    let features = if has_table(conn, &table)? {
        // The whole row, without the two columns the engine adds: a table
        // may have no others, and a query has to select something.
        let mut statement = conn.prepare(&format!(
            "SELECT to_json(f)::VARCHAR FROM {table} f WHERE f.record_id = ?::UUID"
        ))?;
        let mut rows = statement.query([record.id.to_string()])?;
        match rows.next()? {
            Some(row) => {
                let mut features: serde_json::Map<String, serde_json::Value> =
                    serde_json::from_str(&row.get::<_, String>(0)?).map_err(StateError::new)?;
                features.remove("record_id");
                features.remove("declaration_version");
                Some(features)
            }
            None => None,
        }
    } else {
        None
    };

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

/// The shortest chain of links between two members of an identity. Between
/// two records, a constraint is preferred to a version and a version
/// to a judgement, which is the order in which a reconcile run listens to
/// them.
fn path(
    conn: &Connection,
    identity: IdentityId,
    left: RecordId,
    right: RecordId,
) -> Result<Vec<Link>> {
    let mut statement = conn.prepare(
        "WITH member AS (
           SELECT record_id FROM spool.membership
           WHERE to_seq IS NULL AND identity_id = $1::UUID)
         SELECT c.record_a::VARCHAR, c.record_b::VARCHAR, 0 AS kind, NULL AS run
         FROM spool.constraint_link c
         WHERE c.to_seq IS NULL AND c.kind = 'must_link'
           AND c.record_a IN (SELECT record_id FROM member)
           AND c.record_b IN (SELECT record_id FROM member)
         UNION ALL
         SELECT o.supersedes::VARCHAR, o.record_id::VARCHAR, 1, NULL
         FROM spool.record o
         WHERE o.supersedes IN (SELECT record_id FROM member)
           AND o.record_id IN (SELECT record_id FROM member)
         UNION ALL
         SELECT j.left_id::VARCHAR, j.right_id::VARCHAR, 2, max(j.run_id)
         FROM spool.judgement j
         JOIN spool.run r ON r.run_id = j.run_id AND r.status = 'succeeded'
         WHERE j.verdict = 'match'
           AND j.left_id IN (SELECT record_id FROM member)
           AND j.right_id IN (SELECT record_id FROM member)
         GROUP BY j.left_id, j.right_id
         ORDER BY 3, 1, 2",
    )?;
    let mut rows = statement.query([identity.to_string()])?;
    // For each record, what it is linked to, the strongest link of a
    // pair first and only that one.
    let mut links: BTreeMap<RecordId, Vec<(RecordId, LinkKind, Option<RunId>)>> = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let one: RecordId = parse(&row.get::<_, String>(0)?)?;
        let other: RecordId = parse(&row.get::<_, String>(1)?)?;
        let kind = match row.get::<_, i32>(2)? {
            0 => LinkKind::MustLink,
            1 => LinkKind::Version,
            _ => LinkKind::Match,
        };
        let run = row.get::<_, Option<i64>>(3)?.map(|run| RunId(run as u64));
        for (from, to) in [(one, other), (other, one)] {
            let linked = links.entry(from).or_default();
            if !linked.iter().any(|(known, _, _)| *known == to) {
                linked.push((to, kind, run));
            }
        }
    }

    let mut reached: BTreeMap<RecordId, Link> = BTreeMap::new();
    let mut queue = VecDeque::from([left]);
    while let Some(from) = queue.pop_front() {
        if from == right {
            break;
        }
        for (to, kind, run) in links.get(&from).map(Vec::as_slice).unwrap_or_default() {
            if *to != left && !reached.contains_key(to) {
                reached.insert(
                    *to,
                    Link {
                        from,
                        to: *to,
                        kind: *kind,
                        run: *run,
                    },
                );
                queue.push_back(*to);
            }
        }
    }
    let mut chain = Vec::new();
    let mut at = right;
    while let Some(link) = reached.remove(&at) {
        at = link.from;
        chain.push(link);
    }
    chain.reverse();
    Ok(chain)
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
                        id => format!("{id}"),
                    };
                    let how = match (link.kind, link.run) {
                        (LinkKind::MustLink, _) => "a `must_link` constraint".to_owned(),
                        (LinkKind::Version, _) => "a version of the same row".to_owned(),
                        (LinkKind::Match, Some(run)) => format!("`match` in run {run}"),
                        (LinkKind::Match, None) => "`match`".to_owned(),
                    };
                    steps.push_str(&format!(" → {to} ({how})"));
                }
                let between = chain.len() - 1;
                said.push(format!(
                    "Nothing links the two directly. They are joined through {between} other {}: {steps}.",
                    if between == 1 { "member" } else { "members" }
                ));
            }
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
