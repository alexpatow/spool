//! Identities whose members nothing holds together any more.
//!
//! Records come to share an identity through evidence: a strategy judged two
//! of them a match, a reviewer said so, or they are versions of one row. That
//! evidence often runs through a third record. When the source of that
//! record takes it back, the others stay where they are, and nothing the
//! store knows may be left that says they are one thing. Nothing reports
//! that by itself: no rule says they differ, so a rematch is silent.
//!
//! A retraction therefore looks at each identity it took a record out of,
//! and sorts the members that are left into groups. A load does the same
//! for each identity it gave a record back to: the record is where it was,
//! and what held it there may still be gone. Two members are in one
//! group when a chain of these leads from one to the other, through members
//! only:
//!
//! - the latest judgement of the two by a match run that has been
//!   reconciled, or is being, is a `match`. A run that still takes
//!   judgements may be given up, so what it has said so far is not
//!   evidence yet;
//! - a `must_link` constraint in force names the two;
//! - one is the earlier version of the other, under the same key;
//! - a decision joined them. A changeset that is not the engine's and
//!   places records in an identity says that they belong with the members
//!   it had, and one that merges identities says the same of theirs. What
//!   it joins is whole sides: the records it placed, the members of each
//!   identity it merged away, and the members that were there. A side that
//!   has since come apart itself is joined with nothing, because the
//!   decision does not say which part of it was meant.
//!
//! One group is the common case, and nothing happens. More than one is put
//! to a reviewer as a `possible_split` review item, which names the
//! identity and the groups. The engine does not split: the groups may well
//! be one thing, and only what joined them is gone.
//!
//! The item changes no identity and makes no work for a match run. It is
//! open while the members of the identity are in more than one group by
//! the rules above, and there is one open item for an identity at most.
//! Its identity, records and groups are kept in step with where its
//! records are, in the transaction that commits a changeset which touches
//! them and when a reconcile run has judged: a record that joins is in the
//! group it matches, and one that leaves is gone from the item. So a split
//! made along the groups is a split of the identity as it is.
//!
//! The item goes with its records. When its identity is merged into
//! another, it names the one that stands for both, and if that one had an
//! item of its own, one of the two is kept. When the merge is undone, each
//! identity that is left in more than one group has an item again. The
//! item closes like this:
//!
//! - `resolved`: a changeset left nothing to ask. A reviewer split the
//!   identity, or said the groups are one thing with a `must_link`; or the
//!   record that was taken back came back; or a run placed a new record
//!   that matches both groups; or a whole group left; or its identity was
//!   merged with one whose item asks for both.
//! - `superseded`: a run judged members of the groups a match and
//!   committed nothing that touches the identity, as a rematch may.
//! - `dismissed`: a reviewer closed it as it is. A later retraction from
//!   the identity, or a record that comes back to it, asks again.

use std::collections::{BTreeMap, BTreeSet};

use duckdb::vtab::arrow::ArrowBatchRegistration;
use duckdb::{Connection, params};
use spool_core::{EntityType, IdentityId, RecordId, RunId, Seq, StateError};

use crate::batch::{Column, batch};
use crate::error::Result;
use crate::store::parse;

/// The members of each of the given active identities, as the groups that
/// are each held together, the largest group first. An identity without
/// members has no entry.
///
/// `concluding` is the match run that is being reconciled, if one is: its
/// judgements count from then on, and its status says so only when the
/// reconciling is done.
fn groups(
    conn: &Connection,
    identities: &[IdentityId],
    concluding: Option<RunId>,
) -> Result<BTreeMap<IdentityId, Vec<Vec<RecordId>>>> {
    let mut found = BTreeMap::new();
    if identities.is_empty() {
        return Ok(found);
    }
    let asked = || -> Result<ArrowBatchRegistration> {
        Ok(ArrowBatchRegistration::new(batch(vec![(
            "identity_id",
            Column::Text(
                identities
                    .iter()
                    .map(|identity| Some(identity.to_string()))
                    .collect(),
            ),
        )])?))
    };
    // A match run has been reconciled once it is drafted or has succeeded.
    let reconciled = match concluding {
        Some(run) => format!(
            "(r.status IN ('drafted', 'succeeded') OR r.run_id = {})",
            run.0
        ),
        None => "r.status IN ('drafted', 'succeeded')".to_owned(),
    };
    const MEMBERS: &str = "
        member AS (
          SELECT m.record_id, m.identity_id
          FROM spool.membership m
          WHERE m.to_seq IS NULL
            AND m.identity_id IN (SELECT DISTINCT identity_id::UUID FROM arrow(?)))";

    let mut index: BTreeMap<RecordId, usize> = BTreeMap::new();
    let mut members: Vec<(RecordId, IdentityId)> = Vec::new();
    let mut union = Union::default();
    {
        let mut statement = conn.prepare(&format!(
            "WITH {MEMBERS}
             SELECT record_id::VARCHAR, identity_id::VARCHAR
             FROM member ORDER BY identity_id, record_id"
        ))?;
        let mut rows = statement.query([&asked()?])?;
        while let Some(row) = rows.next()? {
            let record: RecordId = parse(&row.get::<_, String>(0)?)?;
            index.insert(record, union.node());
            members.push((record, parse(&row.get::<_, String>(1)?)?));
        }
    }
    {
        // A pair is stored once, the smaller ID first, in each of the three
        // places, and two members of one identity are all that is asked
        // about: a record belongs to one identity.
        let mut statement = conn.prepare(&format!(
            "WITH {MEMBERS},
             judged AS (
               SELECT j.left_id, j.right_id, j.verdict
               FROM spool.judgement j
               JOIN member a ON a.record_id = j.left_id
               JOIN member b ON b.record_id = j.right_id AND b.identity_id = a.identity_id
               JOIN spool.run r ON r.run_id = j.run_id AND {reconciled}
               QUALIFY j.run_id = max(j.run_id) OVER (PARTITION BY j.left_id, j.right_id))
             SELECT left_id::VARCHAR, right_id::VARCHAR FROM judged
             GROUP BY left_id, right_id
             HAVING bool_or(verdict = 'match') AND NOT bool_or(verdict = 'no_match')
             UNION ALL
             SELECT c.record_a::VARCHAR, c.record_b::VARCHAR
             FROM spool.constraint_link c
             JOIN member a ON a.record_id = c.record_a
             JOIN member b ON b.record_id = c.record_b AND b.identity_id = a.identity_id
             WHERE c.to_seq IS NULL AND c.kind = 'must_link'
             UNION ALL
             SELECT o.record_id::VARCHAR, o.supersedes::VARCHAR
             FROM spool.record o
             JOIN member a ON a.record_id = o.record_id
             JOIN member b ON b.record_id = o.supersedes AND b.identity_id = a.identity_id"
        ))?;
        let mut rows = statement.query([&asked()?])?;
        while let Some(row) = rows.next()? {
            let a: RecordId = parse(&row.get::<_, String>(0)?)?;
            let b: RecordId = parse(&row.get::<_, String>(1)?)?;
            union.join(index[&a], index[&b]);
        }
    }
    {
        // The decisions, oldest first, each as the sides it joined. A
        // record that came back after its source had taken it back is where
        // the engine put it, so every stay a member had in the identity
        // counts, and in the identities that were merged into it.
        //
        // - `placed`: the records the changeset assigned to the identity.
        //   Whoever placed them together said they are one thing, unless
        //   the changeset reinstated the identity: that undoes a merge, and
        //   puts back what was there without saying anything about it.
        // - an identity's ID: the members of an identity it merged away.
        // - `there`: the members the identity had before and kept.
        let mut statement = conn.prepare(&format!(
            "WITH {MEMBERS},
             stay AS (
               SELECT m.record_id, h.identity_id, h.from_seq, h.to_seq
               FROM member m
               JOIN spool.membership h ON h.record_id = m.record_id
               JOIN spool.identity i ON i.identity_id = h.identity_id
               WHERE coalesce(i.merged_into, i.identity_id) = m.identity_id),
             decided AS (
               SELECT op.seq, op.kind, op.identity_id, op.from_identity_id, op.record_id
               FROM spool.changeset_op op
               JOIN spool.changeset c ON c.seq = op.seq AND c.actor <> 'engine'
               WHERE op.kind IN ('assign', 'merge', 'reinstate') AND op.identity_id IS NOT NULL)
             SELECT d.seq, d.identity_id::VARCHAR, 'placed', s.record_id::VARCHAR,
                    NOT EXISTS (
                      SELECT 1 FROM decided undone
                      WHERE undone.seq = d.seq AND undone.kind = 'reinstate'
                        AND undone.identity_id = d.identity_id)
             FROM decided d
             JOIN stay s
               ON s.record_id = d.record_id AND s.identity_id = d.identity_id
              AND s.from_seq = d.seq
             WHERE d.kind = 'assign'
             UNION ALL
             SELECT d.seq, d.identity_id::VARCHAR, d.from_identity_id::VARCHAR,
                    s.record_id::VARCHAR, false
             FROM decided d
             JOIN stay s ON s.identity_id = d.from_identity_id AND s.to_seq = d.seq
             WHERE d.kind = 'merge'
             UNION ALL
             SELECT d.seq, d.identity_id::VARCHAR, 'there', s.record_id::VARCHAR, false
             FROM (SELECT DISTINCT seq, identity_id FROM decided WHERE kind <> 'reinstate') d
             JOIN stay s
               ON s.identity_id = d.identity_id AND s.from_seq < d.seq
              AND (s.to_seq IS NULL OR s.to_seq > d.seq)
             ORDER BY 1, 2, 3, 4"
        ))?;
        let mut rows = statement.query([&asked()?])?;
        // One decision at a time: a changeset and an identity it placed
        // records in or merged others into.
        let mut decision: Option<(i64, String)> = None;
        let mut sides: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        let mut together: BTreeSet<String> = BTreeSet::new();
        let decide = |union: &mut Union,
                      sides: &mut BTreeMap<String, Vec<usize>>,
                      together: &mut BTreeSet<String>| {
            let mut whole: Option<usize> = None;
            for (side, nodes) in sides.iter() {
                if together.contains(side) {
                    for node in &nodes[1..] {
                        union.join(nodes[0], *node);
                    }
                }
                let root = union.root(nodes[0]);
                if nodes.iter().all(|node| union.root(*node) == root) {
                    if let Some(other) = whole {
                        union.join(other, root);
                    }
                    whole = Some(root);
                }
            }
            sides.clear();
            together.clear();
        };
        while let Some(row) = rows.next()? {
            let at = (row.get::<_, i64>(0)?, row.get::<_, String>(1)?);
            if decision.as_ref() != Some(&at) {
                decide(&mut union, &mut sides, &mut together);
                decision = Some(at);
            }
            let side: String = row.get(2)?;
            let record: RecordId = parse(&row.get::<_, String>(3)?)?;
            if row.get::<_, bool>(4)? {
                together.insert(side.clone());
            }
            sides.entry(side).or_default().push(index[&record]);
        }
        decide(&mut union, &mut sides, &mut together);
    }

    let mut grouped: BTreeMap<IdentityId, BTreeMap<usize, Vec<RecordId>>> = BTreeMap::new();
    for (record, identity) in members {
        // Members come ordered by record, so each group is in order.
        grouped
            .entry(identity)
            .or_default()
            .entry(union.root(index[&record]))
            .or_default()
            .push(record);
    }
    for (identity, groups) in grouped {
        let mut groups: Vec<Vec<RecordId>> = groups.into_values().collect();
        groups.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        found.insert(identity, groups);
    }
    Ok(found)
}

/// Disjoint sets of nodes.
#[derive(Default)]
struct Union {
    parent: Vec<usize>,
}

impl Union {
    fn node(&mut self) -> usize {
        self.parent.push(self.parent.len());
        self.parent.len() - 1
    }

    fn root(&mut self, mut node: usize) -> usize {
        while self.parent[node] != node {
            self.parent[node] = self.parent[self.parent[node]];
            node = self.parent[node];
        }
        node
    }

    fn join(&mut self, a: usize, b: usize) {
        let (a, b) = (self.root(a), self.root(b));
        // The smaller root stands for both, so the outcome does not depend
        // on the order the pairs were read in.
        self.parent[a.max(b)] = a.min(b);
    }
}

/// An open `possible_split` item, reduced to what deciding about it takes.
struct Open {
    id: i64,
    /// The identity it names.
    identity: IdentityId,
    groups: Vec<Vec<RecordId>>,
}

/// The open `possible_split` items that `condition` holds for. The
/// condition is about `r`, a row of `spool.review_item`, and `i`, the
/// identity it names.
fn open(conn: &Connection, condition: &str) -> Result<Vec<Open>> {
    let mut statement = conn.prepare(&format!(
        "SELECT r.review_id, i.identity_id::VARCHAR, to_json(r.groups)::VARCHAR
         FROM spool.review_item r
         JOIN spool.identity i ON i.identity_id = r.identity_ids[1]
         WHERE r.status = 'open' AND r.kind = 'possible_split' AND ({condition})
         ORDER BY r.review_id"
    ))?;
    let mut rows = statement.query([])?;
    let mut items = Vec::new();
    while let Some(row) = rows.next()? {
        items.push(Open {
            id: row.get(0)?,
            identity: parse(&row.get::<_, String>(1)?)?,
            groups: match row.get::<_, Option<String>>(2)? {
                Some(groups) => serde_json::from_str(&groups).map_err(StateError::new)?,
                None => Vec::new(),
            },
        });
    }
    Ok(items)
}

/// Whether there is an open `possible_split` item at all. Most stores have
/// none, and this is one cheap look.
fn any_open(conn: &Connection) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT count(*) > 0 FROM spool.review_item
         WHERE status = 'open' AND kind = 'possible_split'",
        [],
        |row| row.get(0),
    )?)
}

fn ids(items: impl IntoIterator<Item = i64>) -> String {
    let ids: Vec<String> = items.into_iter().map(|id| id.to_string()).collect();
    ids.join(", ")
}

/// The identities that the records of the given items are in now, for each
/// item, with those of its records that are there.
fn whereabouts(
    conn: &Connection,
    items: &[Open],
) -> Result<BTreeMap<i64, BTreeMap<IdentityId, BTreeSet<RecordId>>>> {
    let mut found: BTreeMap<i64, BTreeMap<IdentityId, BTreeSet<RecordId>>> = BTreeMap::new();
    if items.is_empty() {
        return Ok(found);
    }
    let mut statement = conn.prepare(&format!(
        "SELECT r.review_id, m.identity_id::VARCHAR, m.record_id::VARCHAR
         FROM (SELECT review_id, unnest(record_ids) AS record_id
               FROM spool.review_item WHERE review_id IN ({})) r
         JOIN spool.membership m ON m.record_id = r.record_id AND m.to_seq IS NULL",
        ids(items.iter().map(|item| item.id))
    ))?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        found
            .entry(row.get(0)?)
            .or_default()
            .entry(parse(&row.get::<_, String>(1)?)?)
            .or_default()
            .insert(parse(&row.get::<_, String>(2)?)?);
    }
    Ok(found)
}

/// Whether records have something to ask of the identity they are in:
/// they come from more than one of the groups it is in now.
fn apart(records: &BTreeSet<RecordId>, groups: &[Vec<RecordId>]) -> bool {
    groups
        .iter()
        .filter(|group| group.iter().any(|record| records.contains(record)))
        .count()
        > 1
}

/// A list of record IDs as SQL.
fn list(records: &[RecordId]) -> String {
    let quoted: Vec<String> = records.iter().map(|id| format!("'{id}'")).collect();
    format!("[{}]::UUID[]", quoted.join(", "))
}

/// The records and the groups of an item as SQL: every record in the
/// groups, in order, and the groups as they are.
fn held(groups: &[Vec<RecordId>]) -> (String, String) {
    let mut records: Vec<RecordId> = groups.iter().flatten().copied().collect();
    records.sort();
    let grouped: Vec<String> = groups.iter().map(|group| list(group)).collect();
    (list(&records), format!("[{}]", grouped.join(", ")))
}

/// Brings open items in step with where their records are, so that each
/// identity that the records of an item leave in more than one group has
/// one open item, which names it and its groups as they are: what a
/// reviewer reads, and a split made along it, are about the identity as it
/// is.
///
/// An item goes with its records. It stays with the identity it names
/// while that one has the question. Otherwise it goes to an identity its
/// records are in that has the question and no item: the one its own was
/// merged into, or the one it was in before a merge that is undone. Where
/// an identity has the question and every item that could ask it is
/// taken, a new item is made, of the run that raised the one it comes
/// from. The items that are left with nothing to ask are returned, for
/// the caller to close: their identity is in one group, or another item
/// asks for it.
fn settle(conn: &Connection, items: Vec<Open>, concluding: Option<RunId>) -> Result<Vec<i64>> {
    let places = whereabouts(conn, &items)?;
    let identities: Vec<IdentityId> = places
        .values()
        .flat_map(|identities| identities.keys().copied())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let now = groups(conn, &identities, concluding)?;
    // The identities that have the question, each with the items whose
    // records ask it, in the order of the items.
    let mut asking: BTreeMap<IdentityId, Vec<&Open>> = BTreeMap::new();
    for item in &items {
        for (identity, records) in places.get(&item.id).into_iter().flatten() {
            if now
                .get(identity)
                .is_some_and(|groups| apart(records, groups))
            {
                asking.entry(*identity).or_default().push(item);
            }
        }
    }
    let mut kept: BTreeMap<IdentityId, &Open> = BTreeMap::new();
    let mut taken: BTreeSet<i64> = BTreeSet::new();
    for (identity, items) in &asking {
        if let Some(item) = items.iter().find(|item| item.identity == *identity) {
            kept.insert(*identity, item);
            taken.insert(item.id);
        }
    }
    let mut next_id: Option<i64> = None;
    for (identity, items) in &asking {
        if kept.contains_key(identity) {
            continue;
        }
        if let Some(item) = items.iter().find(|item| !taken.contains(&item.id)) {
            kept.insert(*identity, item);
            taken.insert(item.id);
            continue;
        }
        // An item that the caller did not pass may name the identity
        // already. It is in step, or the caller would have passed it.
        let named: bool = conn.query_row(
            &format!(
                "SELECT count(*) > 0 FROM spool.review_item
                 WHERE status = 'open' AND kind = 'possible_split'
                   AND identity_ids[1] = '{identity}'"
            ),
            [],
            |row| row.get(0),
        )?;
        if named {
            continue;
        }
        let id = match next_id {
            Some(id) => id,
            None => conn.query_row(
                "SELECT coalesce(max(review_id), 0) + 1 FROM spool.review_item",
                [],
                |row| row.get(0),
            )?,
        };
        let (records, grouped) = held(&now[identity]);
        conn.execute(
            &format!(
                "INSERT INTO spool.review_item
                 SELECT ?, run_id, entity_type, 'possible_split', {records},
                        ['{identity}']::UUID[], 'open', NULL, NULL, NULL, NULL, NULL, {grouped}
                 FROM spool.review_item WHERE review_id = ?"
            ),
            params![id, items[0].id],
        )?;
        next_id = Some(id + 1);
    }
    for (identity, item) in &kept {
        let groups = &now[identity];
        if item.identity != *identity || item.groups != *groups {
            let (records, grouped) = held(groups);
            conn.execute(
                &format!(
                    "UPDATE spool.review_item
                     SET identity_ids = ['{identity}']::UUID[], record_ids = {records},
                         groups = {grouped}
                     WHERE review_id = ?"
                ),
                [item.id],
            )?;
        }
    }
    Ok(items
        .iter()
        .map(|item| item.id)
        .filter(|id| !taken.contains(id))
        .collect())
}

/// Raises a `possible_split` item for each of the identities whose members
/// are in more than one group, and returns how many it raised. The
/// identities are those the run `run` took a record out of, or gave one
/// back to. An identity that has an open item keeps it, in step with its
/// groups, and is not counted.
pub(crate) fn raise(conn: &Connection, run: RunId, identities: &[IdentityId]) -> Result<u64> {
    let apart: BTreeMap<IdentityId, Vec<Vec<RecordId>>> = groups(conn, identities, None)?
        .into_iter()
        .filter(|(_, groups)| groups.len() > 1)
        .collect();
    if apart.is_empty() {
        return Ok(0);
    }
    let known: BTreeMap<IdentityId, Open> = open(conn, "true")?
        .into_iter()
        .map(|item| (item.identity, item))
        .collect();
    let mut next_id: i64 = conn.query_row(
        "SELECT coalesce(max(review_id), 0) + 1 FROM spool.review_item",
        [],
        |row| row.get(0),
    )?;
    let mut raised = 0;
    for (identity, groups) in apart {
        let (records, grouped) = held(&groups);
        if let Some(item) = known.get(&identity) {
            if item.groups != groups {
                conn.execute(
                    &format!(
                        "UPDATE spool.review_item SET record_ids = {records}, groups = {grouped}
                         WHERE review_id = ?"
                    ),
                    [item.id],
                )?;
            }
            continue;
        }
        conn.execute(
            &format!(
                "INSERT INTO spool.review_item
                 SELECT ?, ?, entity_type, 'possible_split', {records}, ['{identity}']::UUID[],
                        'open', NULL, NULL, NULL, NULL, NULL, {grouped}
                 FROM spool.identity WHERE identity_id = '{identity}'"
            ),
            params![next_id, run.0 as i64],
        )?;
        next_id += 1;
        raised += 1;
    }
    Ok(raised)
}

/// Brings the open items of the identities that the changeset at `seq`
/// touches in step with them, and closes as resolved those it left nothing
/// to ask about. Runs inside the transaction that commits the changeset.
///
/// A changeset that a reconcile run drafted is what the run's judgements
/// came to, so the match run it reconciled counts as evidence here,
/// whether or not it has been recorded as finished yet.
pub(crate) fn resolve_answered(conn: &Connection, seq: Seq) -> Result<()> {
    if !any_open(conn)? {
        return Ok(());
    }
    let concluding: Option<i64> = conn.query_row(
        "SELECT max((r.config->>'match_run')::BIGINT)
         FROM spool.changeset c
         JOIN spool.run r ON r.run_id = c.run_id AND r.kind = 'reconcile'
         WHERE c.seq = ?",
        [seq.0 as i64],
        |row| row.get(0),
    )?;
    let concluding = concluding.map(|run| RunId(run as u64));
    let touched = format!(
        "SELECT identity_id FROM spool.changeset_op
         WHERE seq = {seq} AND identity_id IS NOT NULL
         UNION
         SELECT from_identity_id FROM spool.changeset_op
         WHERE seq = {seq} AND from_identity_id IS NOT NULL
         UNION
         SELECT m.identity_id
         FROM spool.changeset_op op
         JOIN spool.membership m
           ON m.to_seq IS NULL AND m.record_id IN (op.record_id, op.other_record_id)
         WHERE op.seq = {seq} AND op.kind IN ('constrain', 'unconstrain')"
    );
    let items = open(
        conn,
        &format!(
            "i.identity_id IN ({touched})
             OR coalesce(i.merged_into, i.identity_id) IN ({touched})"
        ),
    )?;
    let answered = settle(conn, items, concluding)?;
    if !answered.is_empty() {
        conn.execute(
            &format!(
                "UPDATE spool.review_item SET status = 'resolved', closed_seq = ?
                 WHERE review_id IN ({})",
                ids(answered)
            ),
            [seq.0 as i64],
        )?;
    }
    Ok(())
}

/// Brings the open items of an entity type in step with their identities,
/// and closes as superseded by `run` those that have nothing left to ask:
/// the reconcile run `run` does this where it concludes the match run
/// `match_run`, because judgements alone may hold groups together.
pub(crate) fn supersede_answered(
    conn: &Connection,
    entity_type: &EntityType,
    run: RunId,
    match_run: RunId,
) -> Result<()> {
    if !any_open(conn)? {
        return Ok(());
    }
    let items = open(conn, &format!("r.entity_type = '{entity_type}'"))?;
    let answered = settle(conn, items, Some(match_run))?;
    if !answered.is_empty() {
        conn.execute(
            &format!(
                "UPDATE spool.review_item SET status = 'superseded', closed_run = ?
                 WHERE review_id IN ({})",
                ids(answered)
            ),
            [run.0 as i64],
        )?;
    }
    Ok(())
}

/// How many open items the store should not have: one whose records have
/// nothing to ask of the identity it names, which is closed in the
/// transaction that answers it, or one that names an identity another open
/// item names.
pub(crate) fn stale(conn: &Connection) -> Result<u64> {
    if !any_open(conn)? {
        return Ok(0);
    }
    let items = open(conn, "true")?;
    let places = whereabouts(conn, &items)?;
    let identities: Vec<IdentityId> = items
        .iter()
        .map(|item| item.identity)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let now = groups(conn, &identities, None)?;
    let mut asked: BTreeSet<IdentityId> = BTreeSet::new();
    let mut stale = 0;
    for item in &items {
        let asks = match (
            places.get(&item.id).and_then(|at| at.get(&item.identity)),
            now.get(&item.identity),
        ) {
            (Some(records), Some(groups)) => apart(records, groups),
            _ => false,
        };
        if !asks || !asked.insert(item.identity) {
            stale += 1;
        }
    }
    Ok(stale)
}

/// The records that a retraction took out of the given identities, or out
/// of identities merged into them since, and that have not come back.
pub(crate) fn taken_back(conn: &Connection, identities: &[IdentityId]) -> Result<Vec<RecordId>> {
    if identities.is_empty() {
        return Ok(Vec::new());
    }
    let mut statement = conn.prepare(&format!(
        "SELECT DISTINCT t.record_id::VARCHAR
         FROM spool.retraction t
         JOIN spool.membership m ON m.record_id = t.record_id AND m.to_seq = t.detached_seq
         JOIN spool.identity i ON i.identity_id = m.identity_id
         WHERE t.restored_run IS NULL
           AND list_contains({}, coalesce(i.merged_into, i.identity_id))
         ORDER BY 1",
        crate::lookup::uuid_list(identities)
    ))?;
    let mut rows = statement.query([])?;
    let mut records = Vec::new();
    while let Some(row) = rows.next()? {
        records.push(parse(&row.get::<_, String>(0)?)?);
    }
    Ok(records)
}
