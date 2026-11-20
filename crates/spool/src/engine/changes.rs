//! Changes: committing, authoring, undoing and splitting by changeset,
//! with the constraints, the drafts and the review items they answer.

use std::collections::{BTreeSet, HashSet};

use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};
use spool_core::{
    Actor, Changeset, ChangesetId, Committed, ConstraintEntry, ConstraintKind, IdentityId,
    IdentityState, IdentityStatus, IdentityStore, Op, RecordId, Seq, ValidationError, inverse,
    validate,
};

use super::{Engine, Inner, lock};
use crate::drafts::{self, DraftEntry, DraftFilter, DraftStatus, DraftSummary};
use crate::error::{Error, Result};
use crate::following::following;
use crate::retract;
use crate::review::{self, ReviewEntry};
use crate::store;

/// A changeset the engine wrote for a caller, and its receipt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Authored {
    pub changeset: Changeset,
    /// `None` when the caller asked only to see the changeset. It was
    /// validated all the same.
    pub committed: Option<Committed>,
}

/// What a split did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SplitReport {
    /// The identity that was split. It keeps its ID and the larger side.
    pub kept: IdentityId,
    /// The new identity.
    pub identity: IdentityId,
    /// The records that moved to the new identity, in order.
    pub moved: Vec<RecordId>,
    pub changeset: Changeset,
    pub committed: Committed,
}

/// Which constraints to list. The default lists those in force.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConstraintFilter {
    /// Only those that name this record.
    pub record: Option<RecordId>,
    /// Only those that name a current member of this identity.
    pub identity: Option<IdentityId>,
    /// Also those that were removed.
    pub include_removed: bool,
}

/// IDs minted for a caller that writes its own changesets.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MintedIds {
    pub identities: Vec<IdentityId>,
    pub changesets: Vec<ChangesetId>,
}

impl Engine {
    /// Validates a changeset against current state and commits it as it
    /// is. This is the only way identity state changes, whoever wrote the
    /// changeset.
    ///
    /// Committing the changeset of a stored draft approves the draft.
    pub fn commit(&self, changeset: Changeset) -> Result<Committed> {
        self.lock().commit(changeset, false)
    }

    /// Commits a changeset someone wrote, with the earlier versions of the
    /// rows it places moved along with them, as [`author`](Self::author)
    /// does. A changeset that carries the ID of a stored draft is committed
    /// as it was stored. What was made to be committed as it is, such as an
    /// undo that was returned without being committed, goes to
    /// [`commit`](Self::commit).
    pub fn commit_with_versions(&self, changeset: Changeset) -> Result<Committed> {
        self.lock().commit(changeset, true)
    }

    /// Writes a changeset from operations and commits it. The engine mints
    /// the changeset's ID and bases it on the head of the log, so this is for
    /// a caller that has just read the state it is changing. A caller that
    /// wants its own base checked, as a merge written from an older read
    /// should be, writes the changeset itself and calls
    /// [`commit`](Self::commit).
    pub fn author(&self, actor: Actor, reason: Option<String>, ops: Vec<Op>) -> Result<Authored> {
        self.lock().author(actor, reason, ops, true, true)
    }

    /// Takes back the changeset at a log position with a new changeset, which
    /// is validated like any other: an undo that no longer fits the state is
    /// rejected, and the rejection names the operation that does not fit.
    /// See [`inverse`] for what undoing each kind of operation means.
    ///
    /// With `commit` unset the changeset is validated and returned without
    /// being committed.
    pub fn undo(
        &self,
        seq: Seq,
        actor: Actor,
        reason: Option<String>,
        commit: bool,
    ) -> Result<Authored> {
        self.lock().undo(seq, actor, reason, commit)
    }

    /// Splits an identity in two: the named records on one side, the
    /// rest of its members on the other. The larger side keeps the ID, the
    /// side that was not named on a tie, and the other side gets a new
    /// identity. With `keep_apart`, a `cannot_link` constraint between the
    /// two sides is recorded in the same changeset, so that nothing puts
    /// them back together unnoticed.
    ///
    /// Which side moves depends on every member, also on those that were
    /// not named. A caller that chose the sides from what it read gives the
    /// log position it read at as `base`, and the split is refused if the
    /// identity's members changed after that.
    pub fn split(
        &self,
        identity: IdentityId,
        records: &[RecordId],
        keep_apart: bool,
        base: Option<Seq>,
        actor: Actor,
        reason: Option<String>,
    ) -> Result<SplitReport> {
        self.lock()
            .split(identity, records, keep_apart, base, actor, reason)
    }

    /// Constraints, ordered by their records.
    pub fn constraints(&self, filter: &ConstraintFilter) -> Result<Vec<ConstraintEntry>> {
        self.read(|conn| constraints(conn, filter))
    }

    /// Closes an open review item without giving its records an
    /// identity. Later runs no longer examine them on its account.
    pub fn dismiss_review(
        &self,
        id: u64,
        actor: &Actor,
        reason: Option<&str>,
    ) -> Result<ReviewEntry> {
        let mut inner = self.lock();
        let now = inner.store.clock.now_micros();
        review::dismiss(&inner.store.conn, id, actor, now, reason)
    }

    /// Commits a stored draft. It is validated like any changeset: if state
    /// has moved under it, the rejections name the operations that no longer
    /// hold, and the draft stays open.
    pub fn approve_draft(&self, draft: ChangesetId) -> Result<Committed> {
        self.lock().approve_draft(draft)
    }

    /// Closes a stored draft without committing it. Its match run has not
    /// succeeded, so the next run examines the same records again.
    pub fn reject_draft(&self, draft: ChangesetId, reason: Option<&str>) -> Result<DraftSummary> {
        self.lock().reject_draft(draft, reason)
    }

    /// A stored draft with its changeset.
    pub fn draft(&self, draft: ChangesetId) -> Result<DraftEntry> {
        self.read(|conn| drafts::get(conn, draft))?
            .ok_or(Error::UnknownDraft(draft))
    }

    /// Stored drafts without their operations, newest first.
    pub fn drafts(&self, filter: &DraftFilter) -> Result<Vec<DraftSummary>> {
        self.read(|conn| drafts::list(conn, filter))
    }

    /// Mints IDs for a caller that writes its own changesets.
    pub fn mint_ids(&self, identities: usize, changesets: usize) -> MintedIds {
        let mut ids = lock(&self.ids);
        MintedIds {
            identities: (0..identities).map(|_| ids.identity_id()).collect(),
            changesets: (0..changesets).map(|_| ids.changeset_id()).collect(),
        }
    }
}

fn constraints(conn: &Connection, filter: &ConstraintFilter) -> Result<Vec<ConstraintEntry>> {
    let mut statement = conn.prepare(
        "SELECT c.record_a::VARCHAR, c.record_b::VARCHAR, c.kind, c.from_seq, c.to_seq
         FROM spool.constraint_link c
         WHERE ($1::BOOLEAN OR c.to_seq IS NULL)
           AND ($2::VARCHAR IS NULL OR $2::UUID IN (c.record_a, c.record_b))
           AND ($3::VARCHAR IS NULL OR EXISTS (
                 SELECT 1 FROM spool.membership m
                 WHERE m.to_seq IS NULL AND m.identity_id = $3::UUID
                   AND m.record_id IN (c.record_a, c.record_b)))",
    )?;
    let mut rows = statement.query(params![
        filter.include_removed,
        filter.record.map(|id| id.to_string()),
        filter.identity.map(|id| id.to_string()),
    ])?;
    let mut found = Vec::new();
    while let Some(row) = rows.next()? {
        found.push(ConstraintEntry {
            constraint: store::constraint(row, 0)?,
            from: Seq(row.get::<_, i64>(3)? as u64),
            to: row.get::<_, Option<i64>>(4)?.map(|seq| Seq(seq as u64)),
        });
    }
    found.sort();
    Ok(found)
}

impl Inner {
    /// The moves that take the earlier versions of the rows `changeset`
    /// places along with them, as far as the state allows: a move it
    /// refuses, such as one a constraint forbids, is left out. Nothing when
    /// the state refuses the changeset's own operations, so that the
    /// refusal is theirs alone, or when it still refuses a move once those
    /// it named are left out, so it is validated with the moves twice at
    /// most.
    pub(super) fn versions_along(&self, changeset: &Changeset) -> Result<Vec<Op>> {
        let own = changeset.ops.len();
        let mut along = following(&self.store.conn, &changeset.ops)?;
        let mut rounds = 0;
        while !along.is_empty() {
            if rounds == 2 {
                return Ok(Vec::new());
            }
            rounds += 1;
            let mut whole = changeset.clone();
            whole.ops.extend(along.iter().cloned());
            let rejections = match validate(&self.store, whole) {
                Ok(_) => break,
                Err(ValidationError::Rejected(rejections)) => rejections,
                Err(_) => return Ok(Vec::new()),
            };
            let refused: HashSet<usize> = rejections
                .iter()
                .filter_map(|rejection| rejection.op_index)
                .filter(|index| *index >= own)
                .map(|index| index - own)
                .collect();
            if refused.is_empty() {
                return Ok(Vec::new());
            }
            let mut at = 0;
            along.retain(|_| {
                at += 1;
                !refused.contains(&(at - 1))
            });
        }
        Ok(along)
    }

    fn commit(&mut self, mut changeset: Changeset, mut along: bool) -> Result<Committed> {
        // A changeset that carries the ID of a stored draft is that draft or
        // it is refused: the ID is what a reviewer approved.
        if let Some(record) = drafts::get(&self.store.conn, changeset.id)? {
            let refuse = |message: String| Error::Draft {
                draft: changeset.id,
                message,
            };
            match record.draft.status {
                DraftStatus::Open if record.changeset != changeset => {
                    return Err(refuse(
                        "the changeset is not the one stored as this draft".to_owned(),
                    ));
                }
                DraftStatus::Rejected | DraftStatus::Superseded => {
                    return Err(refuse(format!(
                        "it was {} and cannot be committed",
                        record.draft.status
                    )));
                }
                // A committed draft is in the log, and the validator says so.
                DraftStatus::Open | DraftStatus::Committed => {}
            }
            along = false;
        }
        if along {
            let along = self.versions_along(&changeset)?;
            changeset.ops.extend(along);
        }
        retract::refuse_placing(&self.store.conn, &changeset.ops)?;
        let validated = validate(&self.store, changeset)?;
        Ok(self.store.apply(validated)?)
    }

    fn author(
        &mut self,
        actor: Actor,
        reason: Option<String>,
        ops: Vec<Op>,
        commit: bool,
        along: bool,
    ) -> Result<Authored> {
        let mut changeset = Changeset {
            id: lock(&self.ids).changeset_id(),
            base: self.store.head()?,
            actor,
            reason,
            run: None,
            ops,
        };
        if along {
            let along = self.versions_along(&changeset)?;
            changeset.ops.extend(along);
        }
        retract::refuse_placing(&self.store.conn, &changeset.ops)?;
        let validated = validate(&self.store, changeset.clone())?;
        let committed = match commit {
            true => Some(self.store.apply(validated)?),
            false => None,
        };
        Ok(Authored {
            changeset,
            committed,
        })
    }

    fn undo(
        &mut self,
        seq: Seq,
        actor: Actor,
        reason: Option<String>,
        commit: bool,
    ) -> Result<Authored> {
        let entry = match seq.0.checked_sub(1) {
            Some(before) => store::read_log_after(&self.store.conn, Seq(before), 1)?.pop(),
            None => None,
        }
        .ok_or(Error::UnknownChangeset(seq))?;
        let ops = inverse(&self.store, &entry)?;
        if ops.is_empty() {
            return Err(Error::NothingToUndo(seq));
        }
        let reason = reason.unwrap_or_else(|| format!("undo of the changeset at position {seq}"));
        // An undo puts back what the changeset moved and nothing else: a
        // version it left where it was stays there.
        self.author(actor, Some(reason), ops, commit, false)
    }

    fn split(
        &mut self,
        identity: IdentityId,
        records: &[RecordId],
        keep_apart: bool,
        base: Option<Seq>,
        actor: Actor,
        reason: Option<String>,
    ) -> Result<SplitReport> {
        let refuse = |message: String| Error::Split { identity, message };
        let entry = self
            .store
            .identities(&[identity])?
            .remove(0)
            .ok_or(Error::UnknownIdentity(identity))?;
        if let IdentityStatus::MergedInto(survivor) = entry.status {
            return Err(refuse(format!(
                "it was merged into {survivor}, which has its members now"
            )));
        }
        if let Some(base) = base {
            let head = self.store.head()?;
            if base > head {
                return Err(refuse(format!(
                    "the split is based on log position {base}, and the log ends at {head}"
                )));
            }
            let changed = self.store.membership_changed(&[identity])?[0];
            if changed > base {
                return Err(refuse(format!(
                    "its members changed at log position {changed}, after log position {base} that the split is based on"
                )));
            }
        }
        let members: BTreeSet<RecordId> = self
            .store
            .members(&[identity])?
            .into_iter()
            .map(|(_, record)| record)
            .collect();
        let named: BTreeSet<RecordId> = records.iter().copied().collect();
        if named.is_empty() {
            return Err(refuse("no records were named".to_owned()));
        }
        if let Some(stranger) = named.difference(&members).next() {
            return Err(refuse(format!("record {stranger} does not belong to it")));
        }
        let rest: BTreeSet<RecordId> = members.difference(&named).copied().collect();
        if rest.is_empty() {
            return Err(refuse(
                "every member was named, and a split leaves records on both sides".to_owned(),
            ));
        }
        // The larger side keeps the ID: more of what refers to the identity
        // is likely to mean that side.
        let (moved, kept) = if named.len() > rest.len() {
            (rest, named)
        } else {
            (named, rest)
        };

        let new = lock(&self.ids).identity_id();
        let mut ops = vec![Op::CreateIdentity {
            identity: new,
            entity_type: entry.entity_type,
        }];
        ops.extend(moved.iter().map(|record| Op::Assign {
            record: *record,
            from: Some(identity),
            to: Some(new),
        }));
        if keep_apart {
            // One constraint between the sides is enough to stop a merge. If
            // one is there already, nothing is added.
            let moved_list: Vec<RecordId> = moved.iter().copied().collect();
            let apart = self.store.constraints_on(&moved_list)?.iter().any(|link| {
                link.kind == ConstraintKind::CannotLink
                    && (kept.contains(&link.a) || kept.contains(&link.b))
            });
            if !apart && let (Some(a), Some(b)) = (moved.first(), kept.first()) {
                ops.push(Op::Constrain {
                    kind: ConstraintKind::CannotLink,
                    a: *a,
                    b: *b,
                });
            }
        }
        // A split moves the records it was given and no other: the
        // reviewer named both sides, versions of one row included.
        let authored = self.author(actor, reason, ops, true, false)?;
        Ok(SplitReport {
            kept: identity,
            identity: new,
            moved: moved.into_iter().collect(),
            committed: authored
                .committed
                .expect("the changeset was committed above"),
            changeset: authored.changeset,
        })
    }

    fn open_draft(&self, draft: ChangesetId, verb: &str) -> Result<DraftEntry> {
        let record = drafts::get(&self.store.conn, draft)?.ok_or(Error::UnknownDraft(draft))?;
        if record.draft.status != DraftStatus::Open {
            return Err(Error::Draft {
                draft,
                message: format!("it is {} and cannot be {verb}", record.draft.status),
            });
        }
        Ok(record)
    }

    fn approve_draft(&mut self, draft: ChangesetId) -> Result<Committed> {
        // A draft is approved as it was stored and shown: the run that
        // drafted it moved the earlier versions along already.
        let changeset = self.open_draft(draft, "approved")?.changeset;
        retract::refuse_placing(&self.store.conn, &changeset.ops)?;
        let validated = validate(&self.store, changeset)?;
        Ok(self.store.apply(validated)?)
    }

    fn reject_draft(&mut self, draft: ChangesetId, reason: Option<&str>) -> Result<DraftSummary> {
        self.open_draft(draft, "rejected")?;
        let now = self.store.clock.now_micros();
        drafts::reject(&self.store.conn, draft, now, reason)?;
        Ok(drafts::get(&self.store.conn, draft)?
            .ok_or(Error::UnknownDraft(draft))?
            .draft)
    }
}
