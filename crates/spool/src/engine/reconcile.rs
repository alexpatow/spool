//! Reconciling: a match run's judgements become a draft changeset, which
//! is committed or left for review.

use duckdb::params;
use serde::{Deserialize, Serialize};
use serde_json::json;
use spool_core::{
    Actor, ChangesetId, Committed, Draft, EntityType, EvidenceSummary, IdentityId, IdentityLink,
    IdentityStore, NewPair, NewRecord, Policy, ReviewItem, RunId, Verdict, reconcile, validate,
};

use super::matching::MatchConfig;
use super::{Engine, Inner, id_error, lock_ignoring_poison};
use crate::drafts;
use crate::error::{Error, Result};
use crate::possible_split;
use crate::retract;
use crate::review;
use crate::rules;
use crate::runs::RunStatus;

/// What becomes of the draft a reconcile run writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disposition {
    /// Commit it at once.
    Commit,
    /// Store it. It waits until it is approved or rejected, or until a later
    /// run of the entity type drafts again.
    LeaveForReview,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TypeResolution {
    pub entity_type: EntityType,
    pub match_run: RunId,
    pub reconcile_run: RunId,
    /// Records the run examined: those loaded since the last run, and
    /// those an earlier run left for review.
    pub new_records: u64,
    /// How many of them an earlier run had left for review.
    pub reexamined: u64,
    /// Distinct candidate pairs.
    pub candidates: u64,
    pub judgements: JudgementCounts,
    pub identities_created: u64,
    pub records_assigned: u64,
    /// Earlier versions of the rows the run placed, moved along with them
    /// from another identity or from none. They are not counted in
    /// `records_assigned`.
    #[serde(default)]
    pub versions_moved: u64,
    /// Examined records left without an identity. Each is named in
    /// `review`.
    pub records_unassigned: u64,
    /// What the run would not decide. The store keeps these as review items.
    pub review: Vec<ReviewItem>,
    /// The changeset, once it is committed. `None` when there was nothing to
    /// commit, and while a draft waits.
    pub committed: Option<Committed>,
    /// The draft that was stored for review, if the run was asked to leave
    /// one. It is named by its changeset ID.
    #[serde(default)]
    pub draft: Option<ChangesetId>,
    /// Where the strategy disagrees with the identities as they are. Only a
    /// rematch has these: no other run judges pairs of records that
    /// both have an identity.
    #[serde(default)]
    pub suggestions: Option<Suggestions>,
}

/// What a rematch would change about existing identities if judgements were
/// allowed to. Nothing here was applied: a reviewer who agrees merges or
/// splits with a changeset.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Suggestions {
    /// Pairs of identities with a `match` between their members, ordered by
    /// their IDs.
    pub merges: Vec<SuggestedMerge>,
    /// Identities with a `no_match` between two of their members, ordered
    /// by ID.
    pub splits: Vec<SuggestedSplit>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SuggestedMerge {
    /// The two identities, the smaller ID first.
    pub identities: [IdentityId; 2],
    /// Pairs of their members the strategy judged `match`.
    pub matches: u64,
    /// Pairs of their members the strategy judged `no_match`. More than
    /// none means the strategy contradicts itself about these two.
    pub no_matches: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SuggestedSplit {
    pub identity: IdentityId,
    /// Pairs of its members the strategy judged `no_match`.
    pub no_matches: u64,
    /// Pairs of its members the strategy judged `match`.
    pub matches: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct JudgementCounts {
    #[serde(rename = "match")]
    pub match_: u64,
    pub no_match: u64,
    pub unsure: u64,
}

impl Engine {
    /// Turns a match run's judgements into a draft changeset and leaves it
    /// for review. Nothing is committed. What the draft leaves out is stored
    /// as review items, which stay open until their records have an
    /// identity.
    ///
    /// The same as [`reconcile_with`](Self::reconcile_with) and
    /// [`Disposition::LeaveForReview`], returning the draft itself.
    pub fn reconcile(&self, run: RunId, policy: &Policy) -> Result<Draft> {
        self.acting(Actor::Engine).reconcile(run, policy)
    }

    /// Turns a match run's judgements into a draft changeset, and commits the
    /// draft or stores it for review.
    ///
    /// A stored draft is named by its changeset ID. The run is `drafted`
    /// until the draft is committed, and only then has it succeeded: what is
    /// new to the next run is counted from the last run that succeeded, so
    /// the records of a draft nobody commits are examined again. A run
    /// whose draft is empty has succeeded at once.
    ///
    /// Drafting an entity type closes its earlier open drafts as superseded,
    /// so a type has at most one draft waiting. A drafted run can be
    /// reconciled again, for example under another policy.
    pub fn reconcile_with(
        &self,
        run: RunId,
        policy: &Policy,
        then: Disposition,
    ) -> Result<TypeResolution> {
        self.acting(Actor::Engine).reconcile_with(run, policy, then)
    }
}

/// The name of the temporary table that holds the judgements a reconcile run
/// goes by.
const JUDGED: &str = "spool_judged";

impl Inner {
    pub(super) fn reconcile(
        &mut self,
        by: &Actor,
        run: RunId,
        policy: &Policy,
        then: Disposition,
    ) -> Result<(Draft, TypeResolution)> {
        let (row, _) = self.match_run(run)?;
        if !matches!(row.status, RunStatus::Open | RunStatus::Drafted) {
            return Err(Error::Run {
                run,
                message: format!("it is {} and cannot be reconciled", row.status),
            });
        }
        self.conclude(by, run, policy, then)
    }

    /// Turns a match run's judgements into a draft, stores its review items,
    /// and commits the draft or stores it.
    ///
    /// The match run has succeeded once its draft is committed, or when there
    /// is nothing to commit, and not before: what is new to the next run is
    /// counted from the last run that succeeded. If the process stops between
    /// the commit and the line that records it, the next run examines the
    /// same records again, and those the commit placed are no longer
    /// new.
    pub(super) fn conclude(
        &mut self,
        by: &Actor,
        run: RunId,
        policy: &Policy,
        then: Disposition,
    ) -> Result<(Draft, TypeResolution)> {
        let (_, config) = self.match_run(run)?;
        let entity_type = config.entity_type.clone();

        let reconcile_run = self.start_run(
            "reconcile",
            by,
            None,
            &json!({ "match_run": run, "policy": policy }),
        )?;
        let (draft, mut report) = self.in_run(reconcile_run, |inner| {
            let evidence = inner.evidence(run, &config)?;
            let new_records = evidence.records.len() as u64;
            // A record that came back after its source had taken it back
            // was loaded long ago too, and no run left it for review.
            let reexamined = inner.count(&format!(
                "SELECT count(*) FROM {} s
                 JOIN spool.record o ON o.record_id = s.record_id
                 WHERE o.load_run <= {after_run}
                   AND s.record_id NOT IN ({restored})",
                rules::SCOPE,
                after_run = config.after_run,
                restored = retract::restored_between(config.after_run, run),
            ))?;
            let mut draft = {
                let mut ids = lock_ignoring_poison(&inner.ids);
                reconcile(&evidence, &inner.store, policy, ids.as_mut())?
            };
            draft.changeset.run = Some(reconcile_run);

            let (identities_created, records_assigned) = count_creates_and_assigns(&draft.changeset.ops);
            let along = inner.versions_along(&draft.changeset)?;
            let versions_moved = along.len() as u64;
            draft.changeset.ops.extend(along);
            let held = then == Disposition::LeaveForReview && !draft.changeset.ops.is_empty();
            let report = TypeResolution {
                entity_type: entity_type.clone(),
                match_run: run,
                reconcile_run,
                new_records,
                reexamined,
                candidates: inner.count(&format!(
                    "SELECT count(*) FROM (SELECT DISTINCT left_id, right_id FROM spool.candidate WHERE run_id = {run})"
                ))?,
                judgements: inner.judgement_counts(run)?,
                identities_created,
                records_assigned,
                versions_moved,
                records_unassigned: new_records - records_assigned,
                review: draft.review.clone(),
                committed: None,
                draft: held.then_some(draft.changeset.id),
                suggestions: match config.rematch {
                    true => Some(inner.suggestions(run)?),
                    false => None,
                },
            };

            let finished = inner.store.clock.now_micros();
            let tx = inner.store.conn.transaction()?;
            review::record(&tx, &entity_type, reconcile_run, &draft.review)?;
            // The match run may have judged a match between records that
            // nothing held together. If it commits nothing that touches
            // their identity, the question is closed where the run is
            // concluded: here when its draft is left for review, and after
            // the commit otherwise.
            if then == Disposition::LeaveForReview {
                possible_split::supersede_answered(&tx, &entity_type, reconcile_run, run)?;
            }
            // Whatever this run drafts, an earlier draft of the type was
            // written against a state this one may change.
            if config.record.is_none() {
                drafts::supersede(&tx, &entity_type, reconcile_run)?;
            }
            if then == Disposition::LeaveForReview {
                let result = json!(report).to_string();
                if held {
                    drafts::store(&tx, &entity_type, run, &draft.changeset)?;
                    tx.execute(
                        "UPDATE spool.run
                         SET status = 'drafted', finished_at = NULL, result = ?::JSON
                         WHERE run_id = ?",
                        params![result, run.0 as i64],
                    )?;
                } else {
                    tx.execute(
                        "UPDATE spool.run
                         SET status = 'succeeded', finished_at = make_timestamptz(?),
                             result = ?::JSON
                         WHERE run_id = ?",
                        params![finished, result, run.0 as i64],
                    )?;
                }
            }
            tx.execute(
                "UPDATE spool.run SET status = 'succeeded', finished_at = make_timestamptz(?)
                 WHERE run_id = ?",
                params![finished, reconcile_run.0 as i64],
            )?;
            tx.commit()?;
            Ok((draft, report))
        })?;

        if then == Disposition::Commit {
            if !draft.changeset.ops.is_empty() {
                let validated = validate(&self.store, draft.changeset.clone())?;
                report.committed = Some(self.store.apply(validated)?);
            }
            // The run has succeeded, and its judgements are evidence from
            // here on: both are recorded together.
            let finished = self.store.clock.now_micros();
            let tx = self.store.conn.transaction()?;
            tx.execute(
                "UPDATE spool.run
                 SET status = 'succeeded', finished_at = make_timestamptz(?), result = ?::JSON
                 WHERE run_id = ?",
                params![finished, json!(report).to_string(), run.0 as i64],
            )?;
            possible_split::supersede_answered(&tx, &entity_type, reconcile_run, run)?;
            tx.commit()?;
        }
        Ok((draft, report))
    }

    /// How many judgements of each verdict a match run made.
    fn judgement_counts(&self, run: RunId) -> Result<JudgementCounts> {
        let judged = |verdict: &str| {
            self.count(&format!(
                "SELECT count(*) FROM spool.judgement WHERE run_id = {run} AND verdict = '{verdict}'"
            ))
        };
        Ok(JudgementCounts {
            match_: judged("match")?,
            no_match: judged("no_match")?,
            unsure: judged("unsure")?,
        })
    }

    /// Joins a match run's judgements against current membership. This is
    /// the heavy part, and DuckDB does it; the reconciler sees a summary.
    fn evidence(&self, run: RunId, config: &MatchConfig) -> Result<EvidenceSummary> {
        let conn = &self.store.conn;
        let scope = rules::SCOPE;
        config.create_scope(conn, run)?;
        // A record that has an identity is not new: one that got it
        // since the run began, and in a rematch every one that had it.
        conn.execute_batch(&format!(
            "DELETE FROM {scope} WHERE record_id IN (
               SELECT record_id FROM spool.membership WHERE to_seq IS NULL)"
        ))?;
        // What the reconciler goes by is the run's judgements, except where
        // something stronger has spoken for a pair. A constraint is a
        // decision someone committed: `must_link` counts as a match and
        // `cannot_link` as a no-match, whatever the strategy said and whether
        // or not it looked at the pair. Next comes the source: a row sent
        // again under its key with other content is the same thing as the
        // version before it.
        conn.execute_batch(&format!(
            "CREATE OR REPLACE TEMP TABLE {JUDGED} AS
             WITH spoken AS (
               SELECT least(c.record_a, c.record_b) AS left_id,
                      greatest(c.record_a, c.record_b) AS right_id,
                      CASE c.kind WHEN 'must_link' THEN 'match' ELSE 'no_match' END AS verdict,
                      1 AS rank
               FROM spool.constraint_link c
               WHERE c.to_seq IS NULL
                 AND (c.record_a IN (SELECT record_id FROM {scope})
                   OR c.record_b IN (SELECT record_id FROM {scope}))
               UNION ALL
               SELECT least(o.record_id, o.supersedes),
                      greatest(o.record_id, o.supersedes), 'match', 2
               FROM spool.record o
               WHERE o.supersedes IS NOT NULL AND o.entity_type = '{entity_type}'
                 AND (o.record_id IN (SELECT record_id FROM {scope})
                   OR o.supersedes IN (SELECT record_id FROM {scope}))
             ), decided AS (
               SELECT left_id, right_id, arg_min(verdict, rank) AS verdict
               FROM spoken GROUP BY left_id, right_id
             )
             SELECT left_id, right_id, verdict, NULL::DOUBLE AS score FROM decided
             UNION ALL
             SELECT j.left_id, j.right_id, j.verdict, j.score
             FROM spool.judgement j
             WHERE j.run_id = {run} AND NOT EXISTS (
               SELECT 1 FROM decided d
               WHERE d.left_id = j.left_id AND d.right_id = j.right_id)",
            entity_type = config.entity_type,
        ))?;
        let mut evidence = EvidenceSummary::default();

        let mut statement = conn.prepare(&format!(
            "SELECT record_id::VARCHAR FROM {scope} ORDER BY record_id"
        ))?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            evidence.records.push(NewRecord {
                record: row.get::<_, String>(0)?.parse().map_err(id_error)?,
                entity_type: config.entity_type.clone(),
            });
        }

        let mut statement = conn.prepare(&format!(
            "SELECT left_id::VARCHAR, right_id::VARCHAR, verdict
             FROM {JUDGED}
             WHERE left_id IN (SELECT record_id FROM {scope})
               AND right_id IN (SELECT record_id FROM {scope})
             ORDER BY left_id, right_id, verdict"
        ))?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            evidence.pairs.push(NewPair {
                a: row.get::<_, String>(0)?.parse().map_err(id_error)?,
                b: row.get::<_, String>(1)?.parse().map_err(id_error)?,
                verdict: row
                    .get::<_, String>(2)?
                    .parse::<Verdict>()
                    .map_err(|message| Error::Run { run, message })?,
            });
        }

        let mut statement = conn.prepare(&format!(
            "SELECT j.new_id::VARCHAR, m.identity_id::VARCHAR,
                    count(*) FILTER (WHERE j.verdict = 'match'),
                    count(*) FILTER (WHERE j.verdict = 'no_match'),
                    max(j.score) FILTER (WHERE j.verdict = 'match')
             FROM (
               SELECT left_id AS new_id, right_id AS other_id, verdict, score
               FROM {JUDGED}
               WHERE left_id IN (SELECT record_id FROM {scope})
               UNION ALL
               SELECT right_id, left_id, verdict, score
               FROM {JUDGED}
               WHERE right_id IN (SELECT record_id FROM {scope})
             ) j
             JOIN spool.membership m ON m.record_id = j.other_id AND m.to_seq IS NULL
             GROUP BY j.new_id, m.identity_id
             ORDER BY j.new_id, m.identity_id"
        ))?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            evidence.links.push(IdentityLink {
                record: row.get::<_, String>(0)?.parse().map_err(id_error)?,
                identity: row.get::<_, String>(1)?.parse().map_err(id_error)?,
                matches: row.get::<_, i64>(2)? as u64,
                no_matches: row.get::<_, i64>(3)? as u64,
                best_score: row.get(4)?,
            });
        }
        Ok(evidence)
    }

    /// Where a rematch's judgements disagree with the identities as they
    /// are. A pair a constraint has spoken for is left out: a reviewer
    /// decided it, and the strategy's view of it is no suggestion.
    fn suggestions(&self, run: RunId) -> Result<Suggestions> {
        let conn = &self.store.conn;
        conn.execute_batch(&format!(
            "CREATE OR REPLACE TEMP TABLE spool_rejudged AS
             SELECT j.verdict, a.identity_id AS left_identity, b.identity_id AS right_identity
             FROM (
               SELECT DISTINCT left_id, right_id, verdict FROM spool.judgement
               WHERE run_id = {run} AND verdict <> 'unsure'
             ) j
             JOIN spool.membership a ON a.record_id = j.left_id AND a.to_seq IS NULL
             JOIN spool.membership b ON b.record_id = j.right_id AND b.to_seq IS NULL
             WHERE NOT EXISTS (
               SELECT 1 FROM spool.constraint_link c
               WHERE c.to_seq IS NULL
                 AND least(c.record_a, c.record_b) = j.left_id
                 AND greatest(c.record_a, c.record_b) = j.right_id)"
        ))?;
        let mut suggestions = Suggestions::default();

        // Two identities a `cannot_link` keeps apart cannot be merged, so
        // merging them is not suggested.
        let mut statement = conn.prepare(
            "WITH apart AS (
               SELECT a.identity_id AS one, b.identity_id AS other
               FROM spool.constraint_link c
               JOIN spool.membership a ON a.record_id = c.record_a AND a.to_seq IS NULL
               JOIN spool.membership b ON b.record_id = c.record_b AND b.to_seq IS NULL
               WHERE c.to_seq IS NULL AND c.kind = 'cannot_link'
             )
             SELECT least(left_identity, right_identity)::VARCHAR,
                    greatest(left_identity, right_identity)::VARCHAR,
                    count(*) FILTER (WHERE verdict = 'match'),
                    count(*) FILTER (WHERE verdict = 'no_match')
             FROM spool_rejudged r
             WHERE left_identity <> right_identity
               AND NOT EXISTS (
                 SELECT 1 FROM apart
                 WHERE (one = left_identity AND other = right_identity)
                    OR (one = right_identity AND other = left_identity))
             GROUP BY 1, 2
             HAVING count(*) FILTER (WHERE verdict = 'match') > 0
             ORDER BY 1, 2",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            suggestions.merges.push(SuggestedMerge {
                identities: [
                    row.get::<_, String>(0)?.parse().map_err(id_error)?,
                    row.get::<_, String>(1)?.parse().map_err(id_error)?,
                ],
                matches: row.get::<_, i64>(2)? as u64,
                no_matches: row.get::<_, i64>(3)? as u64,
            });
        }

        let mut statement = conn.prepare(
            "SELECT left_identity::VARCHAR,
                    count(*) FILTER (WHERE verdict = 'no_match'),
                    count(*) FILTER (WHERE verdict = 'match')
             FROM spool_rejudged
             WHERE left_identity = right_identity
             GROUP BY 1
             HAVING count(*) FILTER (WHERE verdict = 'no_match') > 0
             ORDER BY 1",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            suggestions.splits.push(SuggestedSplit {
                identity: row.get::<_, String>(0)?.parse().map_err(id_error)?,
                no_matches: row.get::<_, i64>(1)? as u64,
                matches: row.get::<_, i64>(2)? as u64,
            });
        }
        Ok(suggestions)
    }
}

/// How many identities a draft's operations create, and how many records
/// they assign.
fn count_creates_and_assigns(ops: &[spool_core::Op]) -> (u64, u64) {
    let (mut created, mut assigned) = (0, 0);
    for op in ops {
        match op {
            spool_core::Op::CreateIdentity { .. } => created += 1,
            spool_core::Op::Assign { .. } => assigned += 1,
            _ => {}
        }
    }
    (created, assigned)
}
