//! The rules every changeset must pass before a store applies it.

use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use crate::changeset::{Changeset, ConstraintKind, ContentHash, Op};
use crate::ids::{EntityType, IdentityId, RecordId, Seq};
use crate::state::{Constraint, IdentityEntry, IdentityState, IdentityStatus, StateError};

/// A changeset that passed [`validate`]. There is no other way to make one,
/// and a store applies nothing else.
#[derive(Clone, Debug)]
pub struct Validated {
    changeset: Changeset,
    content_hash: ContentHash,
}

impl Validated {
    pub fn changeset(&self) -> &Changeset {
        &self.changeset
    }

    pub fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }

    pub fn into_changeset(self) -> Changeset {
        self.changeset
    }
}

/// Why a changeset was not accepted. `op_index` and `op` name the operation
/// that conflicted, and are `None` when the changeset as a whole is at fault.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Rejection {
    pub op_index: Option<usize>,
    pub op: Option<Op>,
    pub reason: Reason,
}

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.op_index, &self.op) {
            (Some(index), Some(op)) => {
                write!(f, "operation {index} ({}): {}", op.kind(), self.reason)
            }
            _ => write!(f, "{}", self.reason),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "reason", rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum Reason {
    #[error("the changeset has no operations")]
    EmptyChangeset,
    #[error("the changeset is based on log position {base}, and the log ends at {head}")]
    BaseAhead { base: Seq, head: Seq },
    #[error("the changeset was already committed at log position {seq}")]
    AlreadyCommitted { seq: Seq },
    #[error("identity {identity} already exists; an identity ID is never reused")]
    IdentityExists { identity: IdentityId },
    #[error("record {record} is not known")]
    UnknownRecord { record: RecordId },
    #[error("identity {identity} does not exist")]
    UnknownIdentity { identity: IdentityId },
    #[error("identity {identity} is not active: it was merged into {merged_into}")]
    IdentityNotActive {
        identity: IdentityId,
        merged_into: IdentityId,
    },
    #[error(
        "stale precondition: record {record} was expected to belong to {}, and belongs to {}",
        display_identity(.expected),
        display_identity(.actual)
    )]
    StaleAssignment {
        record: RecordId,
        expected: Option<IdentityId>,
        actual: Option<IdentityId>,
    },
    #[error("record {record} would stay where it is")]
    NoChange { record: RecordId },
    #[error("record {record} is a {record_type} and identity {identity} is a {identity_type}")]
    EntityTypeMismatch {
        record: RecordId,
        record_type: EntityType,
        identity: IdentityId,
        identity_type: EntityType,
    },
    #[error(
        "record {record} was already moved by operation {first_op}; a changeset may move a record once"
    )]
    RecordAssignedTwice { record: RecordId, first_op: usize },
    #[error("identity {identity} cannot be merged into itself")]
    MergeIntoItself { identity: IdentityId },
    #[error("identity {from} is a {from_type} and identity {into} is a {into_type}")]
    IdentityTypeMismatch {
        from: IdentityId,
        from_type: EntityType,
        into: IdentityId,
        into_type: EntityType,
    },
    #[error(
        "stale merge: the members of identity {identity} changed at log position {changed}, after the changeset's base {base}"
    )]
    StaleMerge {
        identity: IdentityId,
        changed: Seq,
        base: Seq,
    },
    #[error(
        "identity {identity} was given members by operation {first_op}, and merging it would move them again; a changeset may move a record once"
    )]
    MembersMovedTwice {
        identity: IdentityId,
        first_op: usize,
    },
    #[error("identity {identity} is active, so there is nothing to reinstate")]
    IdentityIsActive { identity: IdentityId },
    #[error(
        "identity {identity} was already merged or reinstated by operation {first_op}; a changeset may do that to an identity once"
    )]
    IdentityChangedTwice {
        identity: IdentityId,
        first_op: usize,
    },
    #[error("a constraint needs two records, and both sides are {record}")]
    SameRecord { record: RecordId },
    #[error("record {a} is a {a_type} and record {b} is a {b_type}")]
    ConstraintAcrossTypes {
        a: RecordId,
        a_type: EntityType,
        b: RecordId,
        b_type: EntityType,
    },
    #[error("records {a} and {b} already have a {} constraint", .kind.as_str())]
    ConstraintExists {
        kind: ConstraintKind,
        a: RecordId,
        b: RecordId,
    },
    #[error("records {a} and {b} have no {} constraint", .kind.as_str())]
    UnknownConstraint {
        kind: ConstraintKind,
        a: RecordId,
        b: RecordId,
    },
    #[error(
        "the {} constraint on records {a} and {b} was already changed by operation {first_op}; a changeset may change a constraint once",
        .kind.as_str()
    )]
    ConstraintChangedTwice {
        kind: ConstraintKind,
        a: RecordId,
        b: RecordId,
        first_op: usize,
    },
    #[error(
        "records {a} and {b} have a {} constraint, and would {}",
        .kind.as_str(),
        display_identities(.identity_a, .identity_b)
    )]
    ConstraintViolated {
        kind: ConstraintKind,
        a: RecordId,
        b: RecordId,
        identity_a: Option<IdentityId>,
        identity_b: Option<IdentityId>,
    },
}

fn display_identity(identity: &Option<IdentityId>) -> String {
    match identity {
        Some(identity) => identity.to_string(),
        None => "no identity".to_owned(),
    }
}

/// Where the two records of a broken constraint would be. A
/// cannot_link is broken by one identity holding both, which is named once.
fn display_identities(a: &Option<IdentityId>, b: &Option<IdentityId>) -> String {
    match (a, b) {
        (Some(a), Some(b)) if a == b => format!("both belong to identity {a}"),
        (Some(a), Some(b)) => format!("belong to identities {a} and {b}"),
        (Some(a), None) => format!("belong to identity {a} and no identity"),
        (None, Some(b)) => format!("belong to no identity and identity {b}"),
        (None, None) => "both belong to no identity".to_owned(),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("changeset rejected: {}", .0.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "))]
    Rejected(Vec<Rejection>),
    #[error(transparent)]
    State(#[from] StateError),
}

impl ValidationError {
    /// The rejections, when the changeset was rejected by the rules.
    pub fn rejections(&self) -> Option<&[Rejection]> {
        match self {
            ValidationError::Rejected(rejections) => Some(rejections),
            ValidationError::State(_) => None,
        }
    }
}

/// An identity as the operations checked so far leave it.
enum Known<'a> {
    Missing,
    Active(&'a EntityType),
    /// Merged, and the identity that stands for it.
    Merged(IdentityId),
}

/// What the operations checked so far have done, on top of the state the
/// changeset would be committed to.
#[derive(Default)]
struct Pending<'a> {
    existing: HashMap<IdentityId, Option<IdentityEntry>>,
    created: HashMap<IdentityId, &'a EntityType>,
    /// Merged by this changeset: the identity it was merged into, and by
    /// which operation.
    merged: HashMap<IdentityId, (IdentityId, usize)>,
    reinstated: HashMap<IdentityId, usize>,
    /// Identities that were given members, by the first operation that did.
    received: HashMap<IdentityId, usize>,
    /// Records an `Assign` named, by the operation that did.
    assigned: HashMap<RecordId, usize>,
    /// Where each assigned record ends up.
    placed: HashMap<RecordId, Option<IdentityId>>,
    constraints: BTreeSet<Constraint>,
    constrained: HashMap<Constraint, usize>,
}

impl Pending<'_> {
    /// The entity type of an identity an operation may give members to.
    fn active_type(&self, identity: IdentityId) -> Result<&EntityType, Reason> {
        match self.identity(&identity) {
            Known::Missing => Err(Reason::UnknownIdentity { identity }),
            Known::Merged(merged_into) => Err(Reason::IdentityNotActive {
                identity,
                merged_into,
            }),
            Known::Active(entity_type) => Ok(entity_type),
        }
    }

    fn identity(&self, id: &IdentityId) -> Known<'_> {
        let entity_type = match self.created.get(id) {
            Some(entity_type) => *entity_type,
            None => match self.existing.get(id).and_then(Option::as_ref) {
                None => return Known::Missing,
                Some(record) => {
                    if let IdentityStatus::MergedInto(survivor) = record.status
                        && !self.reinstated.contains_key(id)
                    {
                        return Known::Merged(survivor);
                    }
                    &record.entity_type
                }
            },
        };
        match self.merged.get(id) {
            Some((into, _)) => Known::Merged(*into),
            None => Known::Active(entity_type),
        }
    }
}

/// Checks a changeset against the state it would be committed to.
///
/// Operations are checked in order, each against the state the earlier ones
/// leave behind, so a changeset may create an identity and assign to it. The
/// changeset's `base` does not have to be the head of the log: what is checked
/// is each operation's own precondition.
///
/// Enforced here:
///
/// - a record belongs to at most one identity, and is assigned only to
///   an active identity of its own entity type;
/// - an identity ID is never reused;
/// - only active identities of one entity type are merged, and only if their
///   members are the ones the author saw;
/// - the state the changeset leaves behind breaks no constraint;
/// - a changeset moves a record at most once, and merges or reinstates
///   an identity at most once, because history has one position per
///   changeset.
pub fn validate<S: IdentityState + ?Sized>(
    state: &S,
    changeset: Changeset,
) -> Result<Validated, ValidationError> {
    let mut rejections = check_whole(state, &changeset)?
        .into_iter()
        .map(|reason| Rejection {
            op_index: None,
            op: None,
            reason,
        })
        .collect::<Vec<_>>();
    let mut check = Check::new(state, &changeset)?;
    for (index, op) in changeset.ops.iter().enumerate() {
        if let Err(Broken(reasons)) = check.op(index, op) {
            rejections.extend(reasons.into_iter().map(|reason| Rejection {
                op_index: Some(index),
                op: Some(op.clone()),
                reason,
            }));
        }
    }
    // Each operation held. What is left is the state they add up to: no
    // constraint in force may be broken by it.
    if rejections.is_empty() {
        rejections = check.broken_constraints(state, &changeset.ops)?;
    }

    if rejections.is_empty() {
        let content_hash = changeset.content_hash();
        Ok(Validated {
            changeset,
            content_hash,
        })
    } else {
        Err(ValidationError::Rejected(rejections))
    }
}

/// What is wrong with a changeset as a whole.
fn check_whole<S: IdentityState + ?Sized>(
    state: &S,
    changeset: &Changeset,
) -> Result<Vec<Reason>, StateError> {
    let mut reasons = Vec::new();
    if changeset.ops.is_empty() {
        reasons.push(Reason::EmptyChangeset);
    }
    let head = state.head()?;
    if changeset.base > head {
        reasons.push(Reason::BaseAhead {
            base: changeset.base,
            head,
        });
    }
    if let Some(seq) = state.changeset_seq(changeset.id)? {
        reasons.push(Reason::AlreadyCommitted { seq });
    }
    Ok(reasons)
}

/// What the operations of a changeset name, each list sorted and without
/// repeats.
#[derive(Default)]
struct Named {
    records: Vec<RecordId>,
    identities: Vec<IdentityId>,
    /// Both sides of every merge.
    merging: Vec<IdentityId>,
    merged_away: Vec<IdentityId>,
}

impl Named {
    fn of(ops: &[Op]) -> Self {
        let mut named = Named::default();
        for op in ops {
            match op {
                Op::CreateIdentity { identity, .. } | Op::Reinstate { identity } => {
                    named.identities.push(*identity);
                }
                Op::Assign { record, to, .. } => {
                    named.records.push(*record);
                    named.identities.extend(*to);
                }
                Op::Merge { from, into } => {
                    named.identities.extend([*from, *into]);
                    named.merging.extend([*from, *into]);
                    named.merged_away.push(*from);
                }
                Op::Constrain { a, b, .. } | Op::Unconstrain { a, b, .. } => {
                    named.records.extend([*a, *b]);
                }
            }
        }
        for ids in [
            &mut named.identities,
            &mut named.merging,
            &mut named.merged_away,
        ] {
            ids.sort_unstable();
            ids.dedup();
        }
        named.records.sort_unstable();
        named.records.dedup();
        named
    }
}

/// Each of `keys` with what was read for it, in the same order.
fn zip_with<K: Copy + Eq + std::hash::Hash, V>(keys: &[K], values: Vec<V>) -> HashMap<K, V> {
    keys.iter().copied().zip(values).collect()
}

/// The rules an operation broke: one, or both identities of a merge that
/// cannot be merged.
struct Broken(Vec<Reason>);

impl From<Reason> for Broken {
    fn from(reason: Reason) -> Self {
        Broken(vec![reason])
    }
}

/// The state a changeset is checked against, read once, and what its
/// operations checked so far have done to it.
struct Check<'a> {
    base: Seq,
    types: HashMap<RecordId, Option<EntityType>>,
    /// Where each record was before the changeset.
    assigned: HashMap<RecordId, Option<IdentityId>>,
    /// When the members of each identity a merge names last changed.
    changed: HashMap<IdentityId, Seq>,
    pending: Pending<'a>,
}

impl<'a> Check<'a> {
    fn new<S: IdentityState + ?Sized>(
        state: &S,
        changeset: &'a Changeset,
    ) -> Result<Self, StateError> {
        let named = Named::of(&changeset.ops);
        Ok(Check {
            base: changeset.base,
            types: zip_with(&named.records, state.record_types(&named.records)?),
            assigned: zip_with(&named.records, state.assignments(&named.records)?),
            changed: zip_with(&named.merging, state.membership_changed(&named.merging)?),
            // The constraints this changeset names or could break: those on
            // the records it names, and those on the members a merge moves.
            pending: Pending {
                existing: zip_with(&named.identities, state.identities(&named.identities)?),
                constraints: state
                    .constraints_on(&named.records)?
                    .into_iter()
                    .chain(state.constraints_of_members(&named.merged_away)?)
                    .collect(),
                ..Pending::default()
            },
        })
    }

    /// Checks one operation against what the ones before it left, and adds
    /// what it does.
    fn op(&mut self, index: usize, op: &'a Op) -> Result<(), Broken> {
        match op {
            Op::CreateIdentity {
                identity,
                entity_type,
            } => self.create(*identity, entity_type),
            Op::Assign { record, from, to } => self.assign(index, *record, *from, *to),
            Op::Merge { from, into } => self.merge(index, *from, *into),
            Op::Reinstate { identity } => self.reinstate(index, *identity),
            Op::Constrain { kind, a, b } => self.constrain(index, *kind, *a, *b),
            Op::Unconstrain { kind, a, b } => self.unconstrain(index, *kind, *a, *b),
        }
    }

    fn create(&mut self, identity: IdentityId, entity_type: &'a EntityType) -> Result<(), Broken> {
        let pending = &mut self.pending;
        let exists = pending.existing.get(&identity).is_some_and(Option::is_some);
        if exists || pending.created.contains_key(&identity) {
            return Err(Reason::IdentityExists { identity }.into());
        }
        pending.created.insert(identity, entity_type);
        Ok(())
    }

    fn assign(
        &mut self,
        index: usize,
        record: RecordId,
        from: Option<IdentityId>,
        to: Option<IdentityId>,
    ) -> Result<(), Broken> {
        let Some(record_type) = self.types.get(&record).and_then(Option::as_ref) else {
            return Err(Reason::UnknownRecord { record }.into());
        };
        let pending = &mut self.pending;
        if let Some(first_op) = pending.assigned.get(&record) {
            return Err(Reason::RecordAssignedTwice {
                record,
                first_op: *first_op,
            }
            .into());
        }
        pending.assigned.insert(record, index);
        // A merge earlier in the changeset moved the members of the
        // identity it merged away.
        let before = self.assigned.get(&record).copied().flatten();
        let moved = before.and_then(|identity| pending.merged.get(&identity).copied());
        let actual = match moved {
            Some((into, _)) => Some(into),
            None => before,
        };
        if actual != from {
            return Err(Reason::StaleAssignment {
                record,
                expected: from,
                actual,
            }
            .into());
        }
        if let Some((_, first_op)) = moved {
            return Err(Reason::RecordAssignedTwice { record, first_op }.into());
        }
        if from == to {
            return Err(Reason::NoChange { record }.into());
        }
        pending.placed.insert(record, to);
        let Some(target) = to else { return Ok(()) };
        let identity_type = pending.active_type(target)?.clone();
        if identity_type != *record_type {
            return Err(Reason::EntityTypeMismatch {
                record,
                record_type: record_type.clone(),
                identity: target,
                identity_type,
            }
            .into());
        }
        pending.received.entry(target).or_insert(index);
        Ok(())
    }

    fn merge(&mut self, index: usize, from: IdentityId, into: IdentityId) -> Result<(), Broken> {
        if from == into {
            return Err(Reason::MergeIntoItself { identity: from }.into());
        }
        let pending = &mut self.pending;
        let (from_type, into_type) = match (pending.active_type(from), pending.active_type(into)) {
            (Ok(from_type), Ok(into_type)) => (from_type.clone(), into_type.clone()),
            (from_type, into_type) => {
                return Err(Broken(
                    [from_type.err(), into_type.err()]
                        .into_iter()
                        .flatten()
                        .collect(),
                ));
            }
        };
        if from_type != into_type {
            return Err(Reason::IdentityTypeMismatch {
                from,
                from_type,
                into,
                into_type,
            }
            .into());
        }
        if let Some(first_op) = pending.reinstated.get(&from) {
            return Err(Reason::IdentityChangedTwice {
                identity: from,
                first_op: *first_op,
            }
            .into());
        }
        if let Some(first_op) = pending.received.get(&from) {
            return Err(Reason::MembersMovedTwice {
                identity: from,
                first_op: *first_op,
            }
            .into());
        }
        let stale = [from, into].into_iter().find_map(|identity| {
            let changed = self.changed.get(&identity).copied().unwrap_or(Seq::ZERO);
            (changed > self.base).then_some((identity, changed))
        });
        if let Some((identity, changed)) = stale {
            return Err(Reason::StaleMerge {
                identity,
                changed,
                base: self.base,
            }
            .into());
        }
        pending.merged.insert(from, (into, index));
        pending.received.entry(into).or_insert(index);
        Ok(())
    }

    fn reinstate(&mut self, index: usize, identity: IdentityId) -> Result<(), Broken> {
        let pending = &mut self.pending;
        let first_op = pending
            .merged
            .get(&identity)
            .map(|(_, first_op)| *first_op)
            .or_else(|| pending.reinstated.get(&identity).copied());
        if let Some(first_op) = first_op {
            return Err(Reason::IdentityChangedTwice { identity, first_op }.into());
        }
        match pending.identity(&identity) {
            Known::Missing => Err(Reason::UnknownIdentity { identity }.into()),
            Known::Active(_) => Err(Reason::IdentityIsActive { identity }.into()),
            Known::Merged(_) => {
                pending.reinstated.insert(identity, index);
                Ok(())
            }
        }
    }

    fn constrain(
        &mut self,
        index: usize,
        kind: ConstraintKind,
        a: RecordId,
        b: RecordId,
    ) -> Result<(), Broken> {
        let constraint = self.check_pair(kind, a, b)?;
        // A pair has one constraint. Both kinds at once would hold only
        // while one of the two has no identity.
        let other = Constraint::new(
            match kind {
                ConstraintKind::MustLink => ConstraintKind::CannotLink,
                ConstraintKind::CannotLink => ConstraintKind::MustLink,
            },
            a,
            b,
        );
        let pending = &mut self.pending;
        let existing = if pending.constraints.contains(&constraint) {
            Some(kind)
        } else if pending.constraints.contains(&other) {
            Some(other.kind)
        } else {
            None
        };
        if let Some(kind) = existing {
            return Err(Reason::ConstraintExists { kind, a, b }.into());
        }
        pending.constraints.insert(constraint);
        pending.constrained.insert(constraint, index);
        Ok(())
    }

    fn unconstrain(
        &mut self,
        index: usize,
        kind: ConstraintKind,
        a: RecordId,
        b: RecordId,
    ) -> Result<(), Broken> {
        let constraint = self.check_pair(kind, a, b)?;
        let pending = &mut self.pending;
        if !pending.constraints.remove(&constraint) {
            return Err(Reason::UnknownConstraint { kind, a, b }.into());
        }
        pending.constrained.insert(constraint, index);
        Ok(())
    }

    /// The constraint an operation adds or removes, if it may: its records
    /// are two known records of one type, and no other operation changed it.
    fn check_pair(
        &self,
        kind: ConstraintKind,
        a: RecordId,
        b: RecordId,
    ) -> Result<Constraint, Reason> {
        if a == b {
            return Err(Reason::SameRecord { record: a });
        }
        let known = |record: RecordId| {
            self.types
                .get(&record)
                .and_then(Option::as_ref)
                .ok_or(Reason::UnknownRecord { record })
        };
        let a_type = known(a)?;
        let b_type = known(b)?;
        if a_type != b_type {
            return Err(Reason::ConstraintAcrossTypes {
                a,
                a_type: a_type.clone(),
                b,
                b_type: b_type.clone(),
            });
        }
        let constraint = Constraint::new(kind, a, b);
        if let Some(first_op) = self.pending.constrained.get(&constraint) {
            return Err(Reason::ConstraintChangedTwice {
                kind,
                a,
                b,
                first_op: *first_op,
            });
        }
        Ok(constraint)
    }

    /// The constraints in force that the state the operations add up to
    /// breaks, each named by the last operation that had a part in it.
    fn broken_constraints<S: IdentityState + ?Sized>(
        mut self,
        state: &S,
        ops: &[Op],
    ) -> Result<Vec<Rejection>, StateError> {
        let pending = &self.pending;
        let mut others: Vec<RecordId> = pending
            .constraints
            .iter()
            .flat_map(|constraint| [constraint.a, constraint.b])
            .filter(|record| !self.assigned.contains_key(record))
            .collect();
        others.sort_unstable();
        others.dedup();
        let found = state.assignments(&others)?;
        self.assigned.extend(others.into_iter().zip(found));

        // Where a record ends up, and the operation that moved it.
        let end = |record: &RecordId| -> (Option<IdentityId>, Option<usize>) {
            if let Some(to) = pending.placed.get(record) {
                return (*to, pending.assigned.get(record).copied());
            }
            let before = self.assigned.get(record).copied().flatten();
            match before.and_then(|identity| pending.merged.get(&identity)) {
                Some((into, index)) => (Some(*into), Some(*index)),
                None => (before, None),
            }
        };
        let mut rejections = Vec::new();
        for constraint in &pending.constraints {
            let ((identity_a, moved_a), (identity_b, moved_b)) =
                (end(&constraint.a), end(&constraint.b));
            if constraint.holds(identity_a, identity_b) {
                continue;
            }
            let index = [
                moved_a,
                moved_b,
                pending.constrained.get(constraint).copied(),
            ]
            .into_iter()
            .flatten()
            .max();
            rejections.push(Rejection {
                op_index: index,
                op: index.map(|index| ops[index].clone()),
                reason: Reason::ConstraintViolated {
                    kind: constraint.kind,
                    a: constraint.a,
                    b: constraint.b,
                    identity_a,
                    identity_b,
                },
            });
        }
        Ok(rejections)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_broken_constraint_names_one_identity_once() {
        let (a, b) = (RecordId::from_u128(1), RecordId::from_u128(2));
        let (x, y) = (
            "00000000-0000-7000-8000-000000000003"
                .parse::<IdentityId>()
                .unwrap(),
            "00000000-0000-7000-8000-000000000004"
                .parse::<IdentityId>()
                .unwrap(),
        );
        let broken = |kind, identity_a, identity_b| {
            Reason::ConstraintViolated {
                kind,
                a,
                b,
                identity_a,
                identity_b,
            }
            .to_string()
        };
        assert_eq!(
            broken(ConstraintKind::CannotLink, Some(x), Some(x)),
            format!(
                "records {a} and {b} have a cannot_link constraint, and would both belong to identity {x}"
            )
        );
        assert_eq!(
            broken(ConstraintKind::MustLink, Some(x), Some(y)),
            format!(
                "records {a} and {b} have a must_link constraint, and would belong to identities {x} and {y}"
            )
        );
        assert_eq!(
            broken(ConstraintKind::MustLink, Some(x), None),
            format!(
                "records {a} and {b} have a must_link constraint, and would belong to identity {x} and no identity"
            )
        );
    }
}
