//! An in-memory identity store: the reference model other stores are compared
//! with, and the target of log replay.

use std::collections::BTreeMap;

use crate::changeset::{ContentHash, Op};
use crate::ids::{ChangesetId, EntityType, IdentityId, ObservationId, Seq};
use crate::state::{
    Committed, Constraint, ConstraintRecord, IdentityRecord, IdentityState, IdentityStatus,
    IdentityStore, LogEntry, Membership, Merged, Result, Snapshot, StateError,
};
use crate::validate::Validated;

#[derive(Clone, Debug, Default)]
pub struct MemoryState {
    observations: BTreeMap<ObservationId, EntityType>,
    /// `status` here names the identity a merge named. What is handed out
    /// names the active identity at the end of the chain: see `resolved`.
    identities: BTreeMap<IdentityId, IdentityRecord>,
    current: BTreeMap<ObservationId, IdentityId>,
    history: Vec<Membership>,
    membership_changed: BTreeMap<IdentityId, Seq>,
    /// Constraints in force, and the log position each has held since.
    constraints: BTreeMap<Constraint, Seq>,
    constraint_history: Vec<ConstraintRecord>,
    log: Vec<LogEntry>,
}

/// The log could not be replayed, which means it is corrupt.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ReplayError {
    #[error("the log skips from position {previous} to {found}")]
    Gap { previous: Seq, found: Seq },
    #[error("the operations at log position {seq} do not match their recorded content hash")]
    HashMismatch { seq: Seq },
    #[error("operation {op_index} at log position {seq} cannot be applied: {message}")]
    Op {
        seq: Seq,
        op_index: usize,
        message: String,
    },
}

impl MemoryState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Makes an observation known. Observations are evidence, not identity
    /// state, so this is not a changeset.
    pub fn observe(&mut self, observation: ObservationId, entity_type: EntityType) {
        self.observations.insert(observation, entity_type);
    }

    /// Rebuilds identity state from a log alone. The result holds no
    /// observations, only what the log says about them.
    pub fn replay(log: &[LogEntry]) -> Result<Self, ReplayError> {
        let mut state = Self::new();
        for entry in log {
            let previous = state.head_seq();
            if entry.seq != previous.next() {
                return Err(ReplayError::Gap {
                    previous,
                    found: entry.seq,
                });
            }
            if ContentHash::of(&entry.changeset.ops) != entry.content_hash {
                return Err(ReplayError::HashMismatch { seq: entry.seq });
            }
            state
                .apply_ops(entry.seq, &entry.changeset.ops)
                .map_err(|(op_index, message)| ReplayError::Op {
                    seq: entry.seq,
                    op_index,
                    message,
                })?;
            state.log.push(entry.clone());
        }
        Ok(state)
    }

    fn head_seq(&self) -> Seq {
        self.log.last().map_or(Seq::ZERO, |entry| entry.seq)
    }

    /// The active identity that stands for `id`.
    fn survivor(&self, id: IdentityId) -> IdentityId {
        let mut current = id;
        while let Some(Merged { into, .. }) = self
            .identities
            .get(&current)
            .and_then(|record| record.merged)
        {
            current = into;
        }
        current
    }

    fn resolved(&self, record: &IdentityRecord) -> IdentityRecord {
        let mut record = record.clone();
        if record.merged.is_some() {
            record.status = IdentityStatus::MergedInto(self.survivor(record.id));
        }
        record
    }

    fn active(&self, identity: &IdentityId) -> Result<(), String> {
        match self.identities.get(identity) {
            Some(record) if record.merged.is_none() => Ok(()),
            Some(_) => Err(format!("identity {identity} is not active")),
            None => Err(format!("identity {identity} does not exist")),
        }
    }

    fn move_observation(
        &mut self,
        seq: Seq,
        observation: ObservationId,
        from: Option<IdentityId>,
        to: Option<IdentityId>,
    ) -> Result<(), String> {
        if let Some(from) = from {
            let open = self
                .history
                .iter_mut()
                .rev()
                .find(|row| row.observation == observation && row.to.is_none())
                .ok_or_else(|| format!("observation {observation} has no open membership"))?;
            open.to = Some(seq);
            self.current.remove(&observation);
            self.membership_changed.insert(from, seq);
        }
        if let Some(to) = to {
            self.current.insert(observation, to);
            self.history.push(Membership {
                observation,
                identity: to,
                from: seq,
                to: None,
            });
            self.membership_changed.insert(to, seq);
        }
        Ok(())
    }

    fn apply_ops(&mut self, seq: Seq, ops: &[Op]) -> Result<(), (usize, String)> {
        for (index, op) in ops.iter().enumerate() {
            self.apply_op(seq, op).map_err(|message| (index, message))?;
        }
        Ok(())
    }

    fn apply_op(&mut self, seq: Seq, op: &Op) -> Result<(), String> {
        match op {
            Op::CreateIdentity {
                identity,
                entity_type,
            } => {
                if self.identities.contains_key(identity) {
                    return Err(format!("identity {identity} already exists"));
                }
                self.identities.insert(
                    *identity,
                    IdentityRecord {
                        id: *identity,
                        entity_type: entity_type.clone(),
                        created: seq,
                        status: IdentityStatus::Active,
                        merged: None,
                    },
                );
            }
            Op::Assign {
                observation,
                from,
                to,
            } => {
                if self.current.get(observation) != from.as_ref() {
                    return Err(format!(
                        "observation {observation} does not belong to the identity the operation expects"
                    ));
                }
                if let Some(target) = to {
                    self.active(target)?;
                }
                self.move_observation(seq, *observation, *from, *to)?;
            }
            Op::Merge { from, into } => {
                if from == into {
                    return Err(format!("identity {from} cannot be merged into itself"));
                }
                self.active(from)?;
                self.active(into)?;
                let members: Vec<ObservationId> = self
                    .current
                    .iter()
                    .filter(|(_, identity)| *identity == from)
                    .map(|(observation, _)| *observation)
                    .collect();
                for observation in members {
                    self.move_observation(seq, observation, Some(*from), Some(*into))?;
                }
                let record = self.identities.get_mut(from).expect("checked above");
                record.status = IdentityStatus::MergedInto(*into);
                record.merged = Some(Merged { into: *into, seq });
            }
            Op::Reinstate { identity } => {
                let record = self
                    .identities
                    .get_mut(identity)
                    .ok_or_else(|| format!("identity {identity} does not exist"))?;
                if record.merged.is_none() {
                    return Err(format!("identity {identity} is active"));
                }
                record.status = IdentityStatus::Active;
                record.merged = None;
            }
            Op::Constrain { kind, a, b } => {
                let constraint = Constraint::new(*kind, *a, *b);
                if self.constraints.contains_key(&constraint) {
                    return Err(format!(
                        "observations {a} and {b} already have a {} constraint",
                        kind.as_str()
                    ));
                }
                self.constraints.insert(constraint, seq);
            }
            Op::Unconstrain { kind, a, b } => {
                let constraint = Constraint::new(*kind, *a, *b);
                let from = self.constraints.remove(&constraint).ok_or_else(|| {
                    format!(
                        "observations {a} and {b} have no {} constraint",
                        kind.as_str()
                    )
                })?;
                self.constraint_history.push(ConstraintRecord {
                    constraint,
                    from,
                    to: Some(seq),
                });
            }
        }
        Ok(())
    }
}

impl IdentityState for MemoryState {
    fn head(&self) -> Result<Seq> {
        Ok(self.head_seq())
    }

    fn changeset_seq(&self, id: ChangesetId) -> Result<Option<Seq>> {
        Ok(self
            .log
            .iter()
            .find(|entry| entry.changeset.id == id)
            .map(|entry| entry.seq))
    }

    fn observation_types(&self, observations: &[ObservationId]) -> Result<Vec<Option<EntityType>>> {
        Ok(observations
            .iter()
            .map(|id| self.observations.get(id).cloned())
            .collect())
    }

    fn identities(&self, ids: &[IdentityId]) -> Result<Vec<Option<IdentityRecord>>> {
        Ok(ids
            .iter()
            .map(|id| self.identities.get(id).map(|record| self.resolved(record)))
            .collect())
    }

    fn assignments(&self, observations: &[ObservationId]) -> Result<Vec<Option<IdentityId>>> {
        Ok(observations
            .iter()
            .map(|id| self.current.get(id).copied())
            .collect())
    }

    fn members(&self, ids: &[IdentityId]) -> Result<Vec<(IdentityId, ObservationId)>> {
        let mut members: Vec<_> = self
            .current
            .iter()
            .filter(|(_, identity)| ids.contains(identity))
            .map(|(observation, identity)| (*identity, *observation))
            .collect();
        members.sort_unstable();
        Ok(members)
    }

    fn membership_changed(&self, ids: &[IdentityId]) -> Result<Vec<Seq>> {
        Ok(ids
            .iter()
            .map(|id| {
                self.membership_changed
                    .get(id)
                    .copied()
                    .unwrap_or(Seq::ZERO)
            })
            .collect())
    }

    fn constraints_on(&self, observations: &[ObservationId]) -> Result<Vec<Constraint>> {
        Ok(self
            .constraints
            .keys()
            .filter(|constraint| {
                observations.contains(&constraint.a) || observations.contains(&constraint.b)
            })
            .copied()
            .collect())
    }

    fn constraints_of_members(&self, ids: &[IdentityId]) -> Result<Vec<Constraint>> {
        let member = |observation: &ObservationId| {
            self.current
                .get(observation)
                .is_some_and(|identity| ids.contains(identity))
        };
        Ok(self
            .constraints
            .keys()
            .filter(|constraint| member(&constraint.a) || member(&constraint.b))
            .copied()
            .collect())
    }

    fn left_at(&self, identity: IdentityId, seq: Seq) -> Result<Vec<ObservationId>> {
        let mut left: Vec<_> = self
            .history
            .iter()
            .filter(|row| row.identity == identity && row.to == Some(seq))
            .map(|row| row.observation)
            .collect();
        left.sort_unstable();
        Ok(left)
    }

    fn merged_before(&self, identity: IdentityId, seq: Seq) -> Result<Option<IdentityId>> {
        Ok(self
            .log
            .iter()
            .take_while(|entry| entry.seq < seq)
            .flat_map(|entry| &entry.changeset.ops)
            .filter_map(|op| match op {
                Op::Merge { from, into } if *from == identity => Some(*into),
                _ => None,
            })
            .last())
    }
}

impl IdentityStore for MemoryState {
    fn apply(&mut self, changeset: Validated) -> Result<Committed> {
        let seq = self.head_seq().next();
        let content_hash = changeset.content_hash().clone();
        let changeset = changeset.into_changeset();

        // Work on a copy so a failure part-way leaves the state untouched.
        let mut next = self.clone();
        next.apply_ops(seq, &changeset.ops)
            .map_err(|(op_index, message)| {
                StateError::new(format!("operation {op_index}: {message}"))
            })?;
        let committed = Committed {
            seq,
            changeset: changeset.id,
            content_hash: content_hash.clone(),
        };
        next.log.push(LogEntry {
            seq,
            changeset,
            content_hash,
        });
        *self = next;
        Ok(committed)
    }

    fn log(&self) -> Result<Vec<LogEntry>> {
        Ok(self.log.clone())
    }

    fn snapshot(&self) -> Result<Snapshot> {
        let mut memberships = self.history.clone();
        memberships.sort();
        let mut constraints = self.constraint_history.clone();
        constraints.extend(
            self.constraints
                .iter()
                .map(|(constraint, from)| ConstraintRecord {
                    constraint: *constraint,
                    from: *from,
                    to: None,
                }),
        );
        constraints.sort();
        Ok(Snapshot {
            head: self.head_seq(),
            identities: self
                .identities
                .values()
                .map(|record| self.resolved(record))
                .collect(),
            memberships,
            constraints,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changeset::Changeset;
    use crate::generate::{IdGenerator, SeededIds};
    use crate::ids::Actor;
    use crate::validate::validate;

    fn widget() -> EntityType {
        EntityType::new("widget").unwrap()
    }

    fn log_of_two() -> Vec<LogEntry> {
        let mut state = MemoryState::new();
        let mut ids = SeededIds::new(3);
        for n in 1..=2 {
            let observation = ObservationId::from_u128(n);
            state.observe(observation, widget());
            let identity = ids.identity_id();
            let changeset = Changeset {
                id: ids.changeset_id(),
                base: state.head_seq(),
                actor: Actor::Engine,
                reason: None,
                run: None,
                ops: vec![
                    Op::CreateIdentity {
                        identity,
                        entity_type: widget(),
                    },
                    Op::Assign {
                        observation,
                        from: None,
                        to: Some(identity),
                    },
                ],
            };
            state.apply(validate(&state, changeset).unwrap()).unwrap();
        }
        state.log().unwrap()
    }

    #[test]
    fn replay_rejects_a_log_with_a_gap() {
        let log = log_of_two();
        assert_eq!(
            MemoryState::replay(&log[1..]).unwrap_err(),
            ReplayError::Gap {
                previous: Seq(0),
                found: Seq(2)
            }
        );
    }

    #[test]
    fn replay_rejects_operations_that_do_not_match_their_hash() {
        let mut log = log_of_two();
        log[1].changeset.ops.pop();
        assert_eq!(
            MemoryState::replay(&log).unwrap_err(),
            ReplayError::HashMismatch { seq: Seq(2) }
        );
    }

    #[test]
    fn replay_rejects_operations_that_cannot_follow_the_log_before_them() {
        let mut log = log_of_two();
        log[1].changeset.ops = log[0].changeset.ops.clone();
        log[1].content_hash = log[0].content_hash.clone();
        assert!(matches!(
            MemoryState::replay(&log).unwrap_err(),
            ReplayError::Op {
                seq: Seq(2),
                op_index: 0,
                ..
            }
        ));
    }
}
