//! What an [`Outcome`] amounts to: its plan code, the operations that make
//! the identities what they would be, and the changes and counts a report
//! shows.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use duckdb::Connection;
use sha2::{Digest, Sha256};
use spool_core::{ConstraintKind, EntityType, IdGenerator, IdentityId, Op, RecordId, ReviewItem};

use super::{
    After, ApprovedPlacement, Before, Change, ChangeDetail, ChangeKind, Contradiction,
    DecisionSummary, Fate, Outcome, PersonDecision, Place, RecomputeRequest, RecomputeSummary,
    RecordMove, TypeRecompute,
};
use crate::error::Result;
use crate::store::parse;

/// A node of the graph whose connected parts are the changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Node {
    Identity(IdentityId),
    Unplaced(RecordId),
    Group(usize),
    Waiting(usize),
}

fn choose(n: u64) -> u64 {
    n * n.saturating_sub(1) / 2
}

impl Outcome {
    pub(crate) fn entity_type(&self) -> &EntityType {
        &self.entity_type
    }

    fn target(&self, record: RecordId) -> Option<Place> {
        self.place.get(&record).copied()
    }

    /// The identity a group becomes, named the way the plan names it: an
    /// identity that keeps its ID, or the group's first record.
    fn group_name(&self, at: usize) -> String {
        match self.keeper[at] {
            Some(identity) => identity.to_string(),
            None => format!("new:{}", self.groups[at][0]),
        }
    }

    /// The plan code of this type: a hash of where each record is and
    /// where it would go.
    pub(crate) fn plan(&self) -> String {
        let mut hash = Sha256::new();
        hash.update(b"spool.recompute.v1\0");
        hash.update(self.entity_type.as_str().as_bytes());
        hash.update([0]);
        for record in &self.records {
            let now = self
                .now
                .get(record)
                .map_or_else(|| "-".to_owned(), ToString::to_string);
            let then = match self.target(*record) {
                Some(Place::Group(at)) => self.group_name(at),
                Some(Place::Waiting(at)) => format!("waiting:{}", self.waiting[at][0]),
                None => "-".to_owned(),
            };
            hash.update(format!("{record} {now} {then}\n").as_bytes());
        }
        hex::encode(hash.finalize())
    }

    /// The operations that make the identities what they would be, with
    /// `mint` for the IDs of new identities: the new identities, then
    /// each record that moves, then each merge. A record of an identity
    /// that is merged into the one it would be in moves with the merge.
    /// The identity of each group is returned with them.
    pub(crate) fn operations(
        &self,
        mut mint: impl FnMut() -> IdentityId,
    ) -> (Vec<Op>, Vec<IdentityId>) {
        let mut ops = Vec::new();
        let mut ids: Vec<Option<IdentityId>> = self.keeper.clone();
        for id in &mut ids {
            if id.is_none() {
                let identity = mint();
                ops.push(Op::CreateIdentity {
                    identity,
                    entity_type: self.entity_type.clone(),
                });
                *id = Some(identity);
            }
        }
        for record in &self.records {
            let from = self.now.get(record).copied();
            let to = match self.target(*record) {
                Some(Place::Group(at)) => ids[at],
                Some(Place::Waiting(_)) => None,
                None => continue,
            };
            if from == to {
                continue;
            }
            if let (Some(from), Some(to)) = (from, to)
                && self.merged.get(&from) == Some(&to)
            {
                continue;
            }
            ops.push(Op::Assign {
                record: *record,
                from,
                to,
            });
        }
        for (from, into) in &self.merged {
            ops.push(Op::Merge {
                from: *from,
                into: *into,
            });
        }
        (ops, ids.into_iter().flatten().collect())
    }

    /// The groups that would wait, as the review items they are left in.
    /// `ids` is the identity of each group, as [`operations`](Self::operations)
    /// returns them: an item names the identity of what people put together
    /// that was kept out of its group.
    pub(crate) fn review(&self, ids: &[IdentityId]) -> Vec<ReviewItem> {
        self.waiting
            .iter()
            .zip(&self.kept_out)
            .map(|(records, kept)| ReviewItem::Conflict {
                records: records.clone(),
                identity: kept.and_then(|record| match self.target(record) {
                    Some(Place::Group(at)) => ids.get(at).copied(),
                    _ => None,
                }),
            })
            .collect()
    }

    /// The label most of the records have; of labels as many have, the one
    /// loaded first.
    fn label_of<'a>(&self, records: impl Iterator<Item = &'a RecordId>) -> Option<String> {
        let mut counts: BTreeMap<&str, (u64, (i64, RecordId))> = BTreeMap::new();
        for record in records {
            if let Some((label, loaded)) = self.labels.get(record) {
                let first = (*loaded, *record);
                let entry = counts.entry(label.as_str()).or_insert((0, first));
                entry.0 += 1;
                entry.1 = entry.1.min(first);
            }
        }
        counts
            .into_iter()
            .max_by(|(_, (x, a)), (_, (y, b))| x.cmp(y).then_with(|| b.cmp(a)))
            .map(|(label, _)| label.to_owned())
    }

    /// Whether a record would be in another identity than now, or in none.
    fn changes_identity(&self, record: RecordId) -> bool {
        match (self.now.get(&record), self.target(record)) {
            (Some(identity), Some(Place::Group(at))) => self.keeper[at] != Some(*identity),
            (None, Some(Place::Group(_))) | (Some(_), Some(Place::Waiting(_))) => true,
            _ => false,
        }
    }

    /// Where the records of a change would go, each place with its records,
    /// the place with most first and then by first record. This is the
    /// order of a change's `after`.
    fn destinations(&self, records: &[RecordId]) -> Vec<(Place, Vec<RecordId>)> {
        let mut by_place: BTreeMap<Place, Vec<RecordId>> = BTreeMap::new();
        for record in records {
            if let Some(place) = self.target(*record) {
                by_place.entry(place).or_default().push(*record);
            }
        }
        let mut found: Vec<(Place, Vec<RecordId>)> = by_place
            .into_iter()
            .map(|(place, mut members)| {
                members.sort();
                (place, members)
            })
            .collect();
        found.sort_by(|(_, a), (_, b)| b.len().cmp(&a.len()).then_with(|| a[0].cmp(&b[0])));
        found
    }

    /// The changes, largest first, each with its records.
    #[expect(clippy::too_many_lines)]
    fn changes(&self) -> Vec<(Change, Vec<RecordId>)> {
        let mut nodes: Vec<Node> = Vec::new();
        let mut index: HashMap<Node, usize> = HashMap::new();
        let mut parent: Vec<usize> = Vec::new();
        fn find(parent: &mut [usize], mut node: usize) -> usize {
            while parent[node] != node {
                parent[node] = parent[parent[node]];
                node = parent[node];
            }
            node
        }
        let mut node = |node: Node, nodes: &mut Vec<Node>, parent: &mut Vec<usize>| -> usize {
            *index.entry(node).or_insert_with(|| {
                nodes.push(node);
                parent.push(parent.len());
                parent.len() - 1
            })
        };
        let mut merged_into: BTreeMap<IdentityId, Vec<IdentityId>> = BTreeMap::new();
        for (from, into) in &self.merged {
            merged_into.entry(*into).or_default().push(*from);
        }
        let mut edges: Vec<(RecordId, usize, usize)> = Vec::new();
        for record in &self.records {
            let Some(target) = self.target(*record) else {
                continue;
            };
            let source = match self.now.get(record) {
                Some(identity) => Node::Identity(*identity),
                None => Node::Unplaced(*record),
            };
            let target = match target {
                Place::Group(at) => Node::Group(at),
                Place::Waiting(at) => Node::Waiting(at),
            };
            let (a, b) = (
                node(source, &mut nodes, &mut parent),
                node(target, &mut nodes, &mut parent),
            );
            let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
            parent[ra.max(rb)] = ra.min(rb);
            edges.push((*record, a, b));
        }
        let mut parts: BTreeMap<usize, (BTreeSet<Node>, Vec<RecordId>)> = BTreeMap::new();
        for (record, a, b) in edges {
            let root = find(&mut parent, a);
            let part = parts.entry(root).or_default();
            part.0.insert(nodes[a]);
            part.0.insert(nodes[b]);
            part.1.push(record);
        }

        let mut changes = Vec::new();
        for (nodes, records) in parts.into_values() {
            let olds: Vec<IdentityId> = nodes
                .iter()
                .filter_map(|node| match node {
                    Node::Identity(identity) => Some(*identity),
                    _ => None,
                })
                .collect();
            let groups: Vec<usize> = nodes
                .iter()
                .filter_map(|node| match node {
                    Node::Group(at) => Some(*at),
                    _ => None,
                })
                .collect();
            let waits: Vec<usize> = nodes
                .iter()
                .filter_map(|node| match node {
                    Node::Waiting(at) => Some(*at),
                    _ => None,
                })
                .collect();
            let unplaced = records
                .iter()
                .filter(|record| !self.now.contains_key(record))
                .count() as u64;
            // Nothing changes: one identity keeps exactly its records, or
            // records that wait go on waiting.
            if olds.is_empty() && groups.is_empty() {
                continue;
            }
            if olds.len() == 1 && groups.len() == 1 && waits.is_empty() && unplaced == 0 {
                continue;
            }
            let kind = match (olds.len(), groups.len(), waits.len()) {
                (0, _, _) => ChangeKind::New,
                (1, 0, _) => ChangeKind::Review,
                (1, 1, 0) => ChangeKind::Gain,
                (1, 1, _) => ChangeKind::Lose,
                (1, _, _) => ChangeKind::Split,
                (_, 0, _) => ChangeKind::Review,
                (_, 1, 0) => ChangeKind::Merge,
                _ => ChangeKind::Regroup,
            };

            let mut by_old: BTreeMap<Option<IdentityId>, Vec<RecordId>> = BTreeMap::new();
            for record in &records {
                by_old
                    .entry(self.now.get(record).copied())
                    .or_default()
                    .push(*record);
            }
            let mut before: Vec<Before> = by_old
                .iter()
                .map(|(identity, members)| Before {
                    identity: *identity,
                    label: self.label_of(members.iter()),
                    records: members.len() as u64,
                })
                .collect();
            before.sort_by(|a, b| {
                b.records
                    .cmp(&a.records)
                    .then_with(|| a.identity.is_none().cmp(&b.identity.is_none()))
                    .then_with(|| a.identity.cmp(&b.identity))
            });
            let after: Vec<After> = self
                .destinations(&records)
                .iter()
                .map(|(place, members)| {
                    let (fate, identity, merged) = match place {
                        Place::Group(at) => match self.keeper[*at] {
                            Some(kept) => (
                                Fate::Kept,
                                Some(kept),
                                merged_into.get(&kept).cloned().unwrap_or_default(),
                            ),
                            None => (Fate::New, None, Vec::new()),
                        },
                        Place::Waiting(_) => (Fate::Waiting, None, Vec::new()),
                    };
                    After {
                        fate,
                        identity,
                        label: self.label_of(members.iter()),
                        records: members.len() as u64,
                        merged,
                    }
                })
                .collect();
            let moved = records
                .iter()
                .filter(|record| self.changes_identity(**record))
                .count() as u64;
            let mut shaped_by: Vec<PersonDecision> = olds
                .iter()
                .flat_map(|identity| self.shaped.get(identity).into_iter().flatten())
                .cloned()
                .collect();
            shaped_by.sort_by_key(|decision| decision.seq);
            shaped_by.dedup();
            let approved_drafts = records
                .iter()
                .filter(|record| self.approved.contains_key(record))
                .count() as u64;
            let blocked_by: Vec<Contradiction> = waits
                .iter()
                .filter_map(|at| self.blocked[*at].first().cloned())
                .collect();
            changes.push((
                Change {
                    rank: 0,
                    kind,
                    records: records.len() as u64,
                    moved,
                    before,
                    after,
                    shaped_by,
                    approved_drafts,
                    blocked_by,
                },
                records,
            ));
        }
        changes.sort_by(|(a, x), (b, y)| {
            b.records
                .cmp(&a.records)
                .then_with(|| x.iter().min().cmp(&y.iter().min()))
        });
        for (rank, (change, _)) in changes.iter_mut().enumerate() {
            change.rank = rank as u64 + 1;
        }
        changes
    }

    pub(crate) fn summary(&self) -> RecomputeSummary {
        let mut summary = RecomputeSummary {
            records: self.records.len() as u64,
            pairs_now: self.pairs_now,
            ..RecomputeSummary::default()
        };
        let mut members: BTreeMap<IdentityId, Vec<RecordId>> = BTreeMap::new();
        for (record, identity) in &self.now {
            members.entry(*identity).or_default().push(*record);
        }
        summary.identities_now = members.len() as u64;
        summary.identities_after = self.groups.len() as u64;
        summary.new_identities = self.keeper.iter().filter(|kept| kept.is_none()).count() as u64;
        summary.merged_away = self.merged.len() as u64;
        let mut cells: HashMap<(IdentityId, usize), u64> = HashMap::new();
        for (identity, records) in &members {
            let places: BTreeSet<Place> = records
                .iter()
                .filter_map(|record| self.target(*record))
                .collect();
            let groups = places
                .iter()
                .filter(|place| matches!(place, Place::Group(_)))
                .count();
            if groups > 1 {
                summary.splitting += 1;
            }
            if groups == 0 {
                summary.emptied += 1;
            }
            for record in records {
                if let Some(Place::Group(at)) = self.target(*record) {
                    *cells.entry((*identity, at)).or_default() += 1;
                }
            }
        }
        for (at, group) in self.groups.iter().enumerate() {
            let from: BTreeSet<Option<IdentityId>> = group
                .iter()
                .map(|record| self.now.get(record).copied())
                .collect();
            if from.iter().flatten().count() > 1 {
                summary.merging += 1;
            }
            if let Some(kept) = self.keeper[at] {
                let had = &members[&kept];
                let stays = group
                    .iter()
                    .filter(|record| self.now.get(record) == Some(&kept))
                    .count();
                if stays == had.len() && group.len() == had.len() {
                    summary.unchanged += 1;
                }
                if group.len() > stays {
                    summary.gaining += 1;
                }
                if had.len() > stays {
                    summary.losing += 1;
                }
            }
            summary.pairs_after += choose(group.len() as u64);
        }
        summary.pairs_kept = cells.values().map(|n| choose(*n)).sum();
        for record in &self.records {
            let from = self.now.get(record).copied();
            match (from, self.target(*record)) {
                (None, Some(Place::Group(_))) => summary.records_placed += 1,
                (Some(_), Some(Place::Waiting(_))) => summary.records_unplaced += 1,
                (Some(identity), Some(Place::Group(at))) if self.keeper[at] != Some(identity) => {
                    summary.records_moved += 1;
                }
                _ => {}
            }
        }
        summary.records_waiting = self.waiting.iter().map(|group| group.len() as u64).sum();
        summary.review_items = self.waiting.len() as u64;
        summary.review_items_closed = self
            .open_items
            .iter()
            .filter(|records| {
                records
                    .iter()
                    .all(|record| matches!(self.target(*record), Some(Place::Group(_))))
            })
            .count() as u64;
        summary
    }

    pub(crate) fn report(
        &self,
        conn: &Connection,
        request: &RecomputeRequest,
        detail_of: Option<IdentityId>,
    ) -> Result<TypeRecompute> {
        let changes = self.changes();
        let mut decisions = DecisionSummary {
            held_together: self.held_together,
            dismissed_splits: self.dismissed_splits,
            ..DecisionSummary::default()
        };
        for constraint in &self.constraints {
            match constraint.kind {
                ConstraintKind::MustLink => decisions.must_link += 1,
                ConstraintKind::CannotLink => decisions.cannot_link += 1,
            }
        }
        for blocked in &self.blocked {
            match blocked.first() {
                Some(contradiction) if contradiction.constraint.is_some() => {
                    decisions.blocked_by_constraints += 1;
                }
                Some(_) => decisions.blocked_by_rules += 1,
                None => {}
            }
        }
        decisions.changes_shaped_by_people = changes
            .iter()
            .filter(|(change, _)| !change.shaped_by.is_empty())
            .count() as u64;
        decisions.changes_with_approved_drafts = changes
            .iter()
            .filter(|(change, _)| change.approved_drafts > 0)
            .count() as u64;

        let detail = match detail_of {
            Some(identity) => changes
                .iter()
                .find(|(change, _)| {
                    change
                        .before
                        .iter()
                        .any(|before| before.identity == Some(identity))
                        || change.after.iter().any(|after| {
                            after.identity == Some(identity) || after.merged.contains(&identity)
                        })
                })
                .map(|(change, records)| self.detail(conn, change, records))
                .transpose()?,
            None => None,
        };
        Ok(TypeRecompute {
            entity_type: self.entity_type.clone(),
            summary: self.summary(),
            decisions,
            changes_total: changes.len() as u64,
            changes: changes
                .into_iter()
                .skip(request.offset as usize)
                .take(request.limit as usize)
                .map(|(change, _)| change)
                .collect(),
            detail,
        })
    }

    fn detail(
        &self,
        conn: &Connection,
        change: &Change,
        records: &[RecordId],
    ) -> Result<ChangeDetail> {
        let places: Vec<Place> = self
            .destinations(records)
            .into_iter()
            .map(|(place, _)| place)
            .collect();
        let mut found = Vec::new();
        let mut sources: HashMap<RecordId, (String, Option<String>)> = HashMap::new();
        {
            let list: Vec<String> = records.iter().map(|id| format!("'{id}'")).collect();
            let mut statement = conn.prepare(&format!(
                "SELECT record_id::VARCHAR, source_id, source_key FROM spool.record
                 WHERE record_id IN (SELECT unnest([{}]::UUID[]))",
                list.join(", ")
            ))?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                sources.insert(
                    parse(&row.get::<_, String>(0)?)?,
                    (row.get(1)?, row.get(2)?),
                );
            }
        }
        for record in records {
            let to = self
                .target(*record)
                .and_then(|place| places.iter().position(|known| *known == place))
                .unwrap_or(0) as u64;
            let (source, key) = sources.get(record).cloned().unwrap_or_default();
            found.push(RecordMove {
                record: *record,
                source,
                key,
                label: self.labels.get(record).map(|(label, _)| label.clone()),
                from: self.now.get(record).copied(),
                to,
            });
        }
        found.sort_by(|a, b| {
            a.to.cmp(&b.to)
                .then_with(|| a.source.cmp(&b.source))
                .then_with(|| a.key.cmp(&b.key))
                .then_with(|| a.record.cmp(&b.record))
        });
        let members: BTreeSet<RecordId> = records.iter().copied().collect();
        let constraints = self
            .constraints
            .iter()
            .filter(|note| members.contains(&note.records[0]) && members.contains(&note.records[1]))
            .cloned()
            .collect();
        let approved_drafts = records
            .iter()
            .filter_map(|record| {
                self.approved
                    .get(record)
                    .map(|(draft, seq)| ApprovedPlacement {
                        record: *record,
                        draft: *draft,
                        seq: *seq,
                    })
            })
            .collect();
        let mut change = change.clone();
        change.blocked_by = change_blocked(self, records);
        Ok(ChangeDetail {
            change,
            records: found,
            constraints,
            approved_drafts,
        })
    }
}

/// Every contradiction in the waiting groups of a change, for its detail.
fn change_blocked(outcome: &Outcome, records: &[RecordId]) -> Vec<Contradiction> {
    let waits: BTreeSet<usize> = records
        .iter()
        .filter_map(|record| match outcome.target(*record) {
            Some(Place::Waiting(at)) => Some(at),
            _ => None,
        })
        .collect();
    waits
        .into_iter()
        .flat_map(|at| outcome.blocked[at].iter().cloned())
        .collect()
}

/// The plan code of several types.
pub(crate) fn combined_plan(plans: &[String]) -> String {
    if let [only] = plans {
        return only.clone();
    }
    let mut hash = Sha256::new();
    hash.update(b"spool.recompute.v1.types\0");
    for plan in plans {
        hash.update(plan.as_bytes());
        hash.update([0]);
    }
    hex::encode(hash.finalize())
}

/// Mints the IDs of the new identities an outcome needs.
pub(crate) fn mint<'a>(ids: &'a mut dyn IdGenerator) -> impl FnMut() -> IdentityId + 'a {
    move || ids.identity_id()
}
