//! Why two observations share an identity, or do not.
//!
//! An explanation gathers what the store holds about a pair: where each
//! observation is and which changeset put it there, what every run that
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
    Actor, ChangesetId, ConstraintKind, ConstraintRecord, IdentityId, ObservationId, RunId, Seq,
    StateError, Verdict,
};

use crate::drafts::DraftStatus;
use crate::engine::StrategyRef;
use crate::error::{Error, Result};
use crate::features;
use crate::lookup::{self, ObservationRecord};
use crate::project::{Manifest, Rule, RuleSet};
use crate::review::{self, ReviewFilter, ReviewKind, ReviewRecord, ReviewStatus};
use crate::runs::RunStatus;
use crate::store::{self, parse};
use crate::time::rfc3339;

/// What the store knows about a pair of observations, and what follows
/// from it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Explanation {
    /// The observation with the smaller ID. It is `l` in a rule.
    pub left: Explained,
    /// The observation with the larger ID. It is `r` in a rule.
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
    pub constraints: Vec<ConstraintRecord>,
    /// The review items that name either of the two, oldest first.
    pub review_items: Vec<ReviewRecord>,
    /// When the two share an identity: the shortest chain of links from
    /// `left` to `right` through members of that identity. Empty when they
    /// do not share one, and when no chain joins them, which means a
    /// changeset written by hand placed at least one of them.
    pub path: Vec<Link>,
}

/// Where the two observations are in relation to each other.
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
    pub observation: ObservationRecord,
    /// Its row in the feature table of its entity type, by column name:
    /// what rules and strategies see of it. `null` when it has no row.
    pub features: Option<serde_json::Map<String, serde_json::Value>>,
    /// The changeset that put it in the identity it has. `null` while it has
    /// none.
    pub placed: Option<Placement>,
}

/// The changeset that put an observation where it is.
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
}

/// One rule applied to a pair.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RuleResult {
    pub name: String,
    /// The rule's SQL, over the feature rows `l` and `r`.
    pub predicate: String,
    /// Whether the predicate holds. `null` when it cannot be decided, which
    /// is what SQL makes of a comparison with a feature that is null. A rule
    /// that cannot be decided does not hold.
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
    /// When the run's draft was left for review: what became of it.
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
    pub from: ObservationId,
    pub to: ObservationId,
    pub kind: LinkKind,
    /// With `match`: the latest run that judged so.
    pub run: Option<RunId>,
}

/// What joins two observations of one identity directly.
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
    a: ObservationId,
    b: ObservationId,
) -> Result<Explanation> {
    if a == b {
        return Err(Error::SameObservation(a));
    }
    let (left, right) = if a < b { (a, b) } else { (b, a) };
    let find = |id| lookup::observation(conn, id)?.ok_or(Error::UnknownObservation(id));
    let (left, right) = (find(left)?, find(right)?);

    let same_type = left.entity_type == right.entity_type;
    let outcome = match (left.identity, right.identity) {
        _ if !same_type => Outcome::DifferentTypes,
        (Some(one), Some(other)) if one == other => Outcome::SameIdentity,
        (Some(_), Some(_)) => Outcome::DifferentIdentities,
        _ => Outcome::Unassigned,
    };
    let rules = match manifest.and_then(|manifest| manifest.entity_type(left.entity_type.as_str()))
    {
        Some(declared) if same_type => read_rules(
            conn,
            &features::table(&left.entity_type),
            &declared.rules,
            left.id,
            right.id,
        )?,
        _ => None,
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
                observation: Some(id),
                ..ReviewFilter::default()
            },
        )?);
    }
    review_items.sort_by_key(|item| item.id);
    review_items.dedup_by_key(|item| item.id);

    let mut explanation = Explanation {
        comparisons: comparisons(conn, left.id, right.id)?,
        constraints: constraints(conn, left.id, right.id)?,
        left: explained(conn, left)?,
        right: explained(conn, right)?,
        outcome,
        summary: Vec::new(),
        rules,
        review_items,
        path,
    };
    explanation.summary = summary(&explanation);
    Ok(explanation)
}

fn explained(conn: &Connection, observation: ObservationRecord) -> Result<Explained> {
    let table = features::table(&observation.entity_type);
    let features = if has_table(conn, &table)? {
        let mut statement = conn.prepare(&format!(
            "SELECT to_json(f)::VARCHAR
             FROM (SELECT * EXCLUDE (observation_id, declaration_version) FROM {table}
                   WHERE observation_id = ?::UUID) f"
        ))?;
        let mut rows = statement.query([observation.id.to_string()])?;
        match rows.next()? {
            Some(row) => {
                Some(serde_json::from_str(&row.get::<_, String>(0)?).map_err(StateError::new)?)
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
         WHERE m.to_seq IS NULL AND m.observation_id = ?::UUID",
    )?;
    let mut rows = statement.query([observation.id.to_string()])?;
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
        observation,
        features,
        placed,
    })
}

/// Whether a feature table exists. A store read without its manifest can
/// hold observations of a type whose table is not there.
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
fn read_rules(
    conn: &Connection,
    table: &str,
    rules: &RuleSet,
    left: ObservationId,
    right: ObservationId,
) -> Result<Option<RuleReading>> {
    if !has_table(conn, table)? {
        return Ok(None);
    }
    let all: Vec<&Rule> = rules
        .block
        .iter()
        .chain(&rules.no_match)
        .chain(&rules.match_)
        .collect();
    let predicates: String = all
        .iter()
        .map(|rule| format!(", (\n{}\n)::BOOLEAN", rule.predicate))
        .collect();
    let mut statement = conn.prepare(&format!(
        "SELECT 1{predicates}
         FROM {table} l, {table} r
         WHERE l.observation_id = ?::UUID AND r.observation_id = ?::UUID"
    ))?;
    let mut rows = statement.query(params![left.to_string(), right.to_string()])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let mut results = Vec::with_capacity(all.len());
    for (index, rule) in all.iter().enumerate() {
        results.push(RuleResult {
            name: rule.name.clone(),
            predicate: rule.predicate.clone(),
            holds: row.get(index + 1)?,
        });
    }
    let match_ = results.split_off(rules.block.len() + rules.no_match.len());
    let no_match = results.split_off(rules.block.len());
    let block = results;

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
    }))
}

fn comparisons(
    conn: &Connection,
    left: ObservationId,
    right: ObservationId,
) -> Result<Vec<Comparison>> {
    let pair = params![left.to_string(), right.to_string()];
    let mut found: BTreeMap<RunId, Comparison> = BTreeMap::new();

    let mut statement = conn.prepare(
        "SELECT r.run_id, r.status, r.strategy, r.strategy_version, epoch_us(r.started_at),
                (SELECT max(d.status) FROM spool.draft d WHERE d.match_run = r.run_id)
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

fn constraints(
    conn: &Connection,
    left: ObservationId,
    right: ObservationId,
) -> Result<Vec<ConstraintRecord>> {
    let mut statement = conn.prepare(
        "SELECT observation_a::VARCHAR, observation_b::VARCHAR, kind, from_seq, to_seq
         FROM spool.constraint_link
         WHERE observation_a = ?::UUID AND observation_b = ?::UUID
         ORDER BY from_seq",
    )?;
    let mut rows = statement.query(params![left.to_string(), right.to_string()])?;
    let mut found = Vec::new();
    while let Some(row) = rows.next()? {
        found.push(ConstraintRecord {
            constraint: store::constraint(row, 0)?,
            from: Seq(row.get::<_, i64>(3)? as u64),
            to: row.get::<_, Option<i64>>(4)?.map(|seq| Seq(seq as u64)),
        });
    }
    Ok(found)
}

/// The shortest chain of links between two members of an identity. Between
/// two observations, a constraint is preferred to a version and a version
/// to a judgement, which is the order in which a reconcile run listens to
/// them.
fn path(
    conn: &Connection,
    identity: IdentityId,
    left: ObservationId,
    right: ObservationId,
) -> Result<Vec<Link>> {
    let mut statement = conn.prepare(
        "WITH member AS (
           SELECT observation_id FROM spool.membership
           WHERE to_seq IS NULL AND identity_id = $1::UUID)
         SELECT c.observation_a::VARCHAR, c.observation_b::VARCHAR, 0 AS kind, NULL AS run
         FROM spool.constraint_link c
         WHERE c.to_seq IS NULL AND c.kind = 'must_link'
           AND c.observation_a IN (SELECT observation_id FROM member)
           AND c.observation_b IN (SELECT observation_id FROM member)
         UNION ALL
         SELECT o.supersedes::VARCHAR, o.observation_id::VARCHAR, 1, NULL
         FROM spool.observation o
         WHERE o.supersedes IN (SELECT observation_id FROM member)
           AND o.observation_id IN (SELECT observation_id FROM member)
         UNION ALL
         SELECT j.left_id::VARCHAR, j.right_id::VARCHAR, 2, max(j.run_id)
         FROM spool.judgement j
         JOIN spool.run r ON r.run_id = j.run_id AND r.status = 'succeeded'
         WHERE j.verdict = 'match'
           AND j.left_id IN (SELECT observation_id FROM member)
           AND j.right_id IN (SELECT observation_id FROM member)
         GROUP BY j.left_id, j.right_id
         ORDER BY 3, 1, 2",
    )?;
    let mut rows = statement.query([identity.to_string()])?;
    // For each observation, what it is linked to, the strongest link of a
    // pair first and only that one.
    let mut links: BTreeMap<ObservationId, Vec<(ObservationId, LinkKind, Option<RunId>)>> =
        BTreeMap::new();
    while let Some(row) = rows.next()? {
        let one: ObservationId = parse(&row.get::<_, String>(0)?)?;
        let other: ObservationId = parse(&row.get::<_, String>(1)?)?;
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

    let mut reached: BTreeMap<ObservationId, Link> = BTreeMap::new();
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
    let (left, right) = (
        &explanation.left.observation,
        &explanation.right.observation,
    );
    // Two versions of one row have the same source and key.
    let (l, r) = match (left.name(), right.name()) {
        (l, r) if l == r => (left.id.to_string(), right.id.to_string()),
        names => names,
    };
    let name_of = |id: ObservationId| if id == left.id { &l } else { &r };
    let mut said = Vec::new();

    match (explanation.outcome, left.identity, right.identity) {
        (Outcome::DifferentTypes, _, _) => {
            said.push(format!(
                "`{l}` is a `{}` and `{r}` is a `{}`. Observations of different entity types are never compared.",
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
                "No chain of matches, constraints or versions joins them within the identity, so a changeset written by hand placed at least one of them."
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
            .filter(|id| item.observations.contains(id))
            .map(|id| name_of(id).as_str())
            .collect();
        let named = listed(named);
        let kind = match item.kind {
            ReviewKind::PossibleMerge => "possible_merge",
            ReviewKind::Conflict => "conflict",
        };
        match (item.status, &item.dismissed) {
            (ReviewStatus::Open, _) => said.push(format!(
                "Review item {} (`{kind}`) is open and names {named}: {}",
                item.id,
                match item.kind {
                    ReviewKind::PossibleMerge =>
                        "what it matched belongs to more than one identity, and merging identities is left to a reviewer.",
                    ReviewKind::Conflict =>
                        "the evidence about it contradicts itself, which is left to a reviewer.",
                }
            )),
            (ReviewStatus::Dismissed, Some(dismissal)) => said.push(format!(
                "Review item {} (`{kind}`), which names {named}, was dismissed by `{}`. No run looks at a dismissed item's observations again.",
                item.id, dismissal.by
            )),
            _ => {}
        }
    }

    if let Some(rules) = &explanation.rules {
        said.push(read(rules));
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

fn read(rules: &RuleReading) -> String {
    let block = holding(&rules.block);
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
    match rules.verdict {
        Some(Verdict::NoMatch) => decided("no_match", &rules.no_match),
        Some(Verdict::Match) => decided("match", &rules.match_),
        _ => format!(
            "{found} and no `no_match` or `match` rule holds, so they would get no judgement."
        ),
    }
}
