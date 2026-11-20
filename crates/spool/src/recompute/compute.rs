//! Working out what the identities of a type would be: the records a
//! recomputation places, the judgements of the rules, and what outranks
//! them, into an [`Outcome`].

use std::collections::{BTreeMap, BTreeSet, HashMap};

use duckdb::Connection;
use spool_core::{
    ChangesetId, ConstraintKind, DisjointSets, EntityType, EvidenceSummary, IdentityId,
    MemoryState, NewPair, NewRecord, Op, Policy, RecordId, ReviewItem, RunId, SeededIds, Seq,
    Verdict, reconcile,
};

use super::{ConstraintNote, Contradiction, Outcome, PersonDecision, Place};
use crate::error::{Error, Result};
use crate::possible_split;
use crate::project::RuleSet;
use crate::retract::NOT_RETRACTED;
use crate::rules::{self, NOT_DISMISSED, SCOPE};
use crate::store::parse;

/// Where a preview keeps the candidate pairs and judgements of the rules.
/// They are temporary tables of a connection that reads, and are gone
/// when its transaction ends.
const CANDIDATES: &str = "spool_recompute_candidate";
const JUDGEMENTS: &str = "spool_recompute_judgement";

/// Lists every record a recomputation of `entity_type` places in the
/// scope table: those loaded before `before_run`, other than those taken
/// back and those a reviewer dismissed from review that have no identity.
pub(crate) fn create_scope(
    conn: &Connection,
    entity_type: &EntityType,
    before_run: RunId,
) -> Result<()> {
    conn.execute_batch(&format!(
        "CREATE OR REPLACE TEMP TABLE {SCOPE} AS
         SELECT record_id FROM spool.record
         WHERE entity_type = '{entity_type}' AND load_run < {before_run} AND {NOT_RETRACTED}
           AND ({NOT_DISMISSED} OR record_id IN (
             SELECT record_id FROM spool.membership WHERE to_seq IS NULL))"
    ))?;
    Ok(())
}

/// Judges every pair the block rules find among the type's records into
/// temporary tables, for a preview. Returns the relation of judgements.
pub(crate) fn judge_for_preview(
    conn: &Connection,
    entity_type: &EntityType,
    rules: &RuleSet,
) -> Result<String> {
    create_scope(conn, entity_type, RunId(i64::MAX as u64))?;
    // Declared column by column: a copy made with `CREATE TABLE .. AS
    // SELECT .. WHERE false` read back the IDs of a large insert as null.
    conn.execute_batch(&format!(
        "CREATE OR REPLACE TEMP TABLE {CANDIDATES} (
           run_id BIGINT NOT NULL, left_id UUID NOT NULL, right_id UUID NOT NULL, rule VARCHAR);
         CREATE OR REPLACE TEMP TABLE {JUDGEMENTS} (
           run_id BIGINT NOT NULL, left_id UUID NOT NULL, right_id UUID NOT NULL,
           verdict VARCHAR NOT NULL, score DOUBLE, detail JSON);"
    ))?;
    rules::generate_candidates_into(conn, entity_type, RunId(0), &rules.block, CANDIDATES)?;
    rules::judge_candidates_into(
        conn,
        entity_type,
        RunId(0),
        &rules.no_match,
        &rules.match_,
        (CANDIDATES, JUDGEMENTS),
    )?;
    Ok(JUDGEMENTS.to_owned())
}

/// Works out what the identities of `entity_type` would be, from the
/// records in the scope table and the judgements in `judgements`, a
/// relation with the columns of `spool.judgement`.
#[expect(clippy::too_many_lines)]
pub(crate) fn compute(
    conn: &Connection,
    entity_type: &EntityType,
    judgements: &str,
) -> Result<Outcome> {
    let records: Vec<RecordId> = strings(
        conn,
        &format!("SELECT record_id::VARCHAR FROM {SCOPE} ORDER BY record_id"),
    )?
    .iter()
    .map(|id| parse(id))
    .collect::<Result<_>>()?;

    let mut now: HashMap<RecordId, IdentityId> = HashMap::new();
    let mut created: HashMap<IdentityId, i64> = HashMap::new();
    {
        let mut statement = conn.prepare(&format!(
            "SELECT m.record_id::VARCHAR, m.identity_id::VARCHAR, i.created_seq
             FROM {SCOPE} s
             JOIN spool.membership m ON m.record_id = s.record_id AND m.to_seq IS NULL
             JOIN spool.identity i ON i.identity_id = m.identity_id"
        ))?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let identity: IdentityId = parse(&row.get::<_, String>(1)?)?;
            now.insert(parse(&row.get::<_, String>(0)?)?, identity);
            created.insert(identity, row.get(2)?);
        }
    }
    let identities: Vec<IdentityId> = {
        let mut ids: Vec<IdentityId> = created.keys().copied().collect();
        ids.sort();
        ids
    };
    let pairs_now: u64 = conn.query_row(
        &format!(
            "SELECT coalesce(sum(n * (n - 1) // 2), 0)::BIGINT FROM (
               SELECT count(*) AS n FROM spool.membership
               WHERE to_seq IS NULL AND identity_id IN (
                 SELECT m.identity_id FROM {SCOPE} s
                 JOIN spool.membership m ON m.record_id = s.record_id AND m.to_seq IS NULL)
               GROUP BY identity_id)"
        ),
        [],
        |row| row.get::<_, i64>(0),
    )? as u64;

    // The evidence, weakest first: what is read later wins.
    let mut verdicts: BTreeMap<(RecordId, RecordId), Verdict> = BTreeMap::new();
    let mut rule_names: HashMap<(RecordId, RecordId), Vec<String>> = HashMap::new();
    {
        let mut statement = conn.prepare(&format!(
            "SELECT j.left_id::VARCHAR, j.right_id::VARCHAR, j.verdict,
                    coalesce(to_json(j.detail->'no_match')::VARCHAR, '[]')
             FROM {judgements} j
             WHERE j.verdict <> 'unsure'
               AND j.left_id IN (SELECT record_id FROM {SCOPE})
               AND j.right_id IN (SELECT record_id FROM {SCOPE})"
        ))?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let pair = ordered(
                parse(&row.get::<_, String>(0)?)?,
                parse(&row.get::<_, String>(1)?)?,
            );
            let verdict: Verdict = row
                .get::<_, String>(2)?
                .parse()
                .map_err(|message: String| Error::Recompute(message))?;
            if verdict == Verdict::NoMatch {
                let names: Vec<String> =
                    serde_json::from_str(&row.get::<_, String>(3)?).unwrap_or_default();
                rule_names.insert(pair, names);
            }
            verdicts.insert(pair, verdict);
        }
    }
    {
        let mut statement = conn.prepare(&format!(
            "SELECT record_id::VARCHAR, supersedes::VARCHAR FROM spool.record
             WHERE entity_type = '{entity_type}' AND supersedes IS NOT NULL
               AND record_id IN (SELECT record_id FROM {SCOPE})
               AND supersedes IN (SELECT record_id FROM {SCOPE})"
        ))?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let pair = ordered(
                parse(&row.get::<_, String>(0)?)?,
                parse(&row.get::<_, String>(1)?)?,
            );
            verdicts.insert(pair, Verdict::Match);
        }
    }

    // What people put together stays together.
    let mut held = DisjointSets::new(records.iter().copied());
    let mut held_together = 0;
    for groups in possible_split::held_together(conn, &identities)?.into_values() {
        for group in groups.into_iter().filter(|group| group.len() > 1) {
            held_together += 1;
            for record in &group[1..] {
                held.join(group[0], *record);
            }
        }
    }
    let mut dismissed_splits = 0;
    for item in strings(
        conn,
        &format!(
            "SELECT to_json(record_ids)::VARCHAR FROM spool.review_item
             WHERE entity_type = '{entity_type}' AND kind = 'possible_split'
               AND status = 'dismissed'
             ORDER BY review_id"
        ),
    )? {
        let ids: Vec<RecordId> = serde_json::from_str::<Vec<String>>(&item)
            .unwrap_or_default()
            .iter()
            .filter_map(|id| id.parse().ok())
            .collect();
        // Those of its records that still share an identity.
        let mut by_identity: BTreeMap<IdentityId, Vec<RecordId>> = BTreeMap::new();
        for id in ids {
            if let Some(identity) = now.get(&id) {
                by_identity.entry(*identity).or_default().push(id);
            }
        }
        let mut counted = false;
        for group in by_identity.into_values().filter(|group| group.len() > 1) {
            counted = true;
            for record in &group[1..] {
                held.join(group[0], *record);
            }
        }
        dismissed_splits += u64::from(counted);
    }
    verdicts
        .retain(|(a, b), verdict| *verdict != Verdict::NoMatch || held.find(*a) != held.find(*b));
    for (root, members) in joined(&held) {
        for record in members.into_iter().filter(|record| *record != root) {
            verdicts.insert(ordered(root, record), Verdict::Match);
        }
    }

    let mut constraints = Vec::new();
    let mut by_constraint: HashMap<(RecordId, RecordId), PersonDecision> = HashMap::new();
    {
        let mut statement = conn.prepare(&format!(
            "SELECT c.record_a::VARCHAR, c.record_b::VARCHAR, c.kind, c.from_seq,
                    ch.actor, ch.reason
             FROM spool.constraint_link c
             JOIN spool.changeset ch ON ch.seq = c.from_seq
             WHERE c.to_seq IS NULL
               AND c.record_a IN (SELECT record_id FROM {SCOPE})
               AND c.record_b IN (SELECT record_id FROM {SCOPE})
             ORDER BY c.record_a, c.record_b"
        ))?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let pair = ordered(
                parse(&row.get::<_, String>(0)?)?,
                parse(&row.get::<_, String>(1)?)?,
            );
            let kind = match row.get::<_, String>(2)?.as_str() {
                "must_link" => ConstraintKind::MustLink,
                _ => ConstraintKind::CannotLink,
            };
            let decision = PersonDecision {
                seq: Seq(row.get::<_, i64>(3)? as u64),
                actor: parse(&row.get::<_, String>(4)?)?,
                reason: row.get(5)?,
            };
            verdicts.insert(
                pair,
                match kind {
                    ConstraintKind::MustLink => Verdict::Match,
                    ConstraintKind::CannotLink => Verdict::NoMatch,
                },
            );
            if kind == ConstraintKind::CannotLink {
                by_constraint.insert(pair, decision.clone());
            }
            constraints.push(ConstraintNote {
                kind,
                records: [pair.0, pair.1],
                decision,
            });
        }
    }

    // The default policy, in an empty store where every record is new.
    let mut state = MemoryState::new();
    for record in &records {
        state.add_record(*record, entity_type.clone());
    }
    let evidence = EvidenceSummary {
        records: records
            .iter()
            .map(|record| NewRecord {
                record: *record,
                entity_type: entity_type.clone(),
            })
            .collect(),
        pairs: verdicts
            .iter()
            .map(|((a, b), verdict)| NewPair {
                a: *a,
                b: *b,
                verdict: *verdict,
            })
            .collect(),
        links: Vec::new(),
    };
    let draft = reconcile(
        &evidence,
        &state,
        &Policy::default(),
        &mut SeededIds::new(0),
    )?;
    let mut by_identity: BTreeMap<IdentityId, Vec<RecordId>> = BTreeMap::new();
    for op in &draft.changeset.ops {
        if let Op::Assign {
            record,
            to: Some(to),
            ..
        } = op
        {
            by_identity.entry(*to).or_default().push(*record);
        }
    }
    let mut groups: Vec<Vec<RecordId>> = by_identity
        .into_values()
        .map(|mut group| {
            group.sort();
            group
        })
        .collect();
    let mut waiting: Vec<Vec<RecordId>> = draft
        .review
        .iter()
        .map(|item| {
            let mut records = match item {
                ReviewItem::PossibleMerge { records, .. }
                | ReviewItem::Conflict { records, .. } => records.clone(),
            };
            records.sort();
            records
        })
        .collect();
    // What people put together stays an identity where the rules contradict
    // themselves around it, and only the other records of such a group wait.
    // Left waiting, its records would lose their identity when the result is
    // applied, and with it what says that a person put them together.
    // The records of a group that is kept out are what the rest contradicts
    // itself about, so the item they wait in names its identity.
    let mut kept_out: Vec<Option<RecordId>> = vec![None; waiting.len()];
    for set in joined(&held).into_values() {
        let Some(at) = waiting
            .iter()
            .position(|group| group.binary_search(&set[0]).is_ok())
        else {
            continue;
        };
        if set
            .iter()
            .all(|record| waiting[at].binary_search(record).is_ok())
        {
            let taken: BTreeSet<RecordId> = set.iter().copied().collect();
            waiting[at].retain(|record| !taken.contains(record));
            kept_out[at].get_or_insert(set[0]);
            groups.push(set);
        }
    }
    let mut waiting_with: Vec<(Vec<RecordId>, Option<RecordId>)> = waiting
        .into_iter()
        .zip(kept_out)
        .filter(|(group, _)| !group.is_empty())
        .collect();
    waiting_with.sort();
    let (waiting, kept_out): (Vec<Vec<RecordId>>, Vec<Option<RecordId>>) =
        waiting_with.into_iter().unzip();
    groups.sort();

    let mut place: HashMap<RecordId, Place> = HashMap::new();
    for (at, group) in groups.iter().enumerate() {
        for record in group {
            place.insert(*record, Place::Group(at));
        }
    }
    for (at, group) in waiting.iter().enumerate() {
        for record in group {
            place.insert(*record, Place::Waiting(at));
        }
    }

    // The `no_match` pairs inside each waiting group, found from the
    // verdicts rather than from every pair of its records.
    let mut blocked: Vec<Vec<Contradiction>> = vec![Vec::new(); waiting.len()];
    for ((a, b), verdict) in &verdicts {
        if *verdict != Verdict::NoMatch {
            continue;
        }
        if let (Some(Place::Waiting(at)), Some(Place::Waiting(other))) =
            (place.get(a), place.get(b))
            && at == other
        {
            let pair = (*a, *b);
            blocked[*at].push(Contradiction {
                records: [*a, *b],
                constraint: by_constraint.get(&pair).cloned(),
                rules: match by_constraint.contains_key(&pair) {
                    true => Vec::new(),
                    false => rule_names.get(&pair).cloned().unwrap_or_default(),
                },
            });
        }
    }
    // A constraint is the decision of a person, so those come first.
    for found in &mut blocked {
        found.sort_by_key(|contradiction| contradiction.constraint.is_none());
    }

    // Which identity each group becomes. An identity's home is the group
    // that holds most of its records, on a tie the first group.
    let mut counts: BTreeMap<IdentityId, BTreeMap<usize, u64>> = BTreeMap::new();
    for (record, identity) in &now {
        if let Some(Place::Group(at)) = place.get(record) {
            *counts.entry(*identity).or_default().entry(*at).or_default() += 1;
        }
    }
    let mut claimants: BTreeMap<usize, Vec<(IdentityId, u64)>> = BTreeMap::new();
    for (identity, at) in &counts {
        let (home, most) = at
            .iter()
            .max_by(|(a, x), (b, y)| x.cmp(y).then_with(|| b.cmp(a)))
            .map(|(home, most)| (*home, *most))
            .expect("an identity counted here has a record in a group");
        claimants.entry(home).or_default().push((*identity, most));
    }
    let mut keeper: Vec<Option<IdentityId>> = vec![None; groups.len()];
    let mut merged: BTreeMap<IdentityId, IdentityId> = BTreeMap::new();
    for (home, mut identities) in claimants {
        // The most records there, then the oldest, then the smallest ID.
        identities.sort_by(|(a, x), (b, y)| {
            y.cmp(x)
                .then_with(|| created[a].cmp(&created[b]))
                .then_with(|| a.cmp(b))
        });
        let kept = identities[0].0;
        keeper[home] = Some(kept);
        for (identity, _) in &identities[1..] {
            merged.insert(*identity, kept);
        }
    }

    let labels = labels(conn)?;
    let shaped = shaped_by_people(conn)?;
    let approved = approved_placements(conn)?;
    let open_items: Vec<Vec<RecordId>> = strings(
        conn,
        &format!(
            "SELECT to_json(record_ids)::VARCHAR FROM spool.review_item
             WHERE entity_type = '{entity_type}' AND status = 'open'
               AND kind <> 'possible_split'
             ORDER BY review_id"
        ),
    )?
    .iter()
    .map(|ids| {
        serde_json::from_str::<Vec<String>>(ids)
            .unwrap_or_default()
            .iter()
            .filter_map(|id| id.parse().ok())
            .collect()
    })
    .collect();

    Ok(Outcome {
        entity_type: entity_type.clone(),
        records,
        now,
        groups,
        waiting,
        kept_out,
        place,
        keeper,
        merged,
        blocked,
        labels,
        shaped,
        approved,
        constraints,
        held_together,
        dismissed_splits,
        open_items,
        pairs_now,
    })
}

fn ordered(a: RecordId, b: RecordId) -> (RecordId, RecordId) {
    if a <= b { (a, b) } else { (b, a) }
}

fn strings(conn: &Connection, sql: &str) -> Result<Vec<String>> {
    let mut statement = conn.prepare(sql)?;
    let mut rows = statement.query([])?;
    let mut found = Vec::new();
    while let Some(row) = rows.next()? {
        found.push(row.get(0)?);
    }
    Ok(found)
}

fn labels(conn: &Connection) -> Result<HashMap<RecordId, (String, i64)>> {
    let mut statement = conn.prepare(&format!(
        "SELECT l.record_id::VARCHAR, l.label, epoch_us(o.loaded_at) FROM spool.record_label l
         JOIN spool.record o ON o.record_id = l.record_id
         WHERE l.label IS NOT NULL AND l.record_id IN (SELECT record_id FROM {SCOPE})"
    ))?;
    let mut rows = statement.query([])?;
    let mut found = HashMap::new();
    while let Some(row) = rows.next()? {
        found.insert(
            parse(&row.get::<_, String>(0)?)?,
            (row.get(1)?, row.get(2)?),
        );
    }
    Ok(found)
}

/// For each identity a record in scope is in, the changesets by people or
/// agents that named it or an identity merged into it, other than to
/// record or remove a constraint.
fn shaped_by_people(conn: &Connection) -> Result<BTreeMap<IdentityId, Vec<PersonDecision>>> {
    let mut statement = conn.prepare(&format!(
        "WITH here AS (
           SELECT DISTINCT m.identity_id FROM {SCOPE} s
           JOIN spool.membership m ON m.record_id = s.record_id AND m.to_seq IS NULL)
         SELECT DISTINCT coalesce(i.merged_into, i.identity_id)::VARCHAR, c.seq, c.actor, c.reason
         FROM spool.changeset_op op
         JOIN spool.changeset c ON c.seq = op.seq AND c.actor <> 'engine'
         JOIN spool.identity i ON i.identity_id IN (op.identity_id, op.from_identity_id)
         WHERE op.kind IN ('create_identity', 'assign', 'merge', 'reinstate')
           AND coalesce(i.merged_into, i.identity_id) IN (SELECT identity_id FROM here)
         ORDER BY 1, 2"
    ))?;
    let mut rows = statement.query([])?;
    let mut found: BTreeMap<IdentityId, Vec<PersonDecision>> = BTreeMap::new();
    while let Some(row) = rows.next()? {
        found
            .entry(parse(&row.get::<_, String>(0)?)?)
            .or_default()
            .push(PersonDecision {
                seq: Seq(row.get::<_, i64>(1)? as u64),
                actor: parse(&row.get::<_, String>(2)?)?,
                reason: row.get(3)?,
            });
    }
    Ok(found)
}

/// The records in scope that drafts placed which reviewers approved, with
/// the last such draft.
fn approved_placements(conn: &Connection) -> Result<BTreeMap<RecordId, (ChangesetId, Seq)>> {
    let mut statement = conn.prepare(&format!(
        "SELECT op.record_id::VARCHAR, d.changeset_id::VARCHAR, op.seq
         FROM spool.changeset_op op
         JOIN spool.draft d ON d.closed_seq = op.seq AND d.status = 'committed'
         WHERE op.kind = 'assign' AND op.identity_id IS NOT NULL
           AND op.record_id IN (SELECT record_id FROM {SCOPE})
         ORDER BY op.seq"
    ))?;
    let mut rows = statement.query([])?;
    let mut found = BTreeMap::new();
    while let Some(row) = rows.next()? {
        found.insert(
            parse(&row.get::<_, String>(0)?)?,
            (
                parse(&row.get::<_, String>(1)?)?,
                Seq(row.get::<_, i64>(2)? as u64),
            ),
        );
    }
    Ok(found)
}

/// Each set of more than one record, by its first record, with its records
/// in order.
fn joined(held: &DisjointSets<RecordId>) -> BTreeMap<RecordId, Vec<RecordId>> {
    let mut sets = held.sets();
    sets.retain(|_, members| members.len() > 1);
    sets
}
