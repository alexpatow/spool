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
#[expect(clippy::too_many_lines)]
pub fn validate<S: IdentityState + ?Sized>(
    state: &S,
    changeset: Changeset,
) -> Result<Validated, ValidationError> {
    let mut rejections = Vec::new();
    macro_rules! reject {
        ($op:expr, $reason:expr $(,)?) => {{
            let op: Option<(usize, &Op)> = $op;
            rejections.push(Rejection {
                op_index: op.map(|(index, _)| index),
                op: op.map(|(_, op)| op.clone()),
                reason: $reason,
            })
        }};
    }

    if changeset.ops.is_empty() {
        reject!(None, Reason::EmptyChangeset);
    }
    let head = state.head()?;
    if changeset.base > head {
        reject!(
            None,
            Reason::BaseAhead {
                base: changeset.base,
                head,
            },
        );
    }
    if let Some(seq) = state.changeset_seq(changeset.id)? {
        reject!(None, Reason::AlreadyCommitted { seq });
    }

    let mut records = Vec::new();
    let mut identities = Vec::new();
    let mut merging = Vec::new();
    let mut merged_away = Vec::new();
    for op in &changeset.ops {
        match op {
            Op::CreateIdentity { identity, .. } | Op::Reinstate { identity } => {
                identities.push(*identity);
            }
            Op::Assign { record, to, .. } => {
                records.push(*record);
                identities.extend(*to);
            }
            Op::Merge { from, into } => {
                identities.extend([*from, *into]);
                merging.extend([*from, *into]);
                merged_away.push(*from);
            }
            Op::Constrain { a, b, .. } | Op::Unconstrain { a, b, .. } => {
                records.extend([*a, *b]);
            }
        }
    }
    for ids in [&mut identities, &mut merging, &mut merged_away] {
        ids.sort_unstable();
        ids.dedup();
    }
    records.sort_unstable();
    records.dedup();

    let types: HashMap<_, _> = records
        .iter()
        .copied()
        .zip(state.record_types(&records)?)
        .collect();
    let mut assigned: HashMap<_, _> = records
        .iter()
        .copied()
        .zip(state.assignments(&records)?)
        .collect();
    let changed: HashMap<_, _> = merging
        .iter()
        .copied()
        .zip(state.membership_changed(&merging)?)
        .collect();

    // The constraints this changeset names or could break: those on the
    // records it names, and those on the members a merge moves.
    let mut pending = Pending {
        existing: identities
            .iter()
            .copied()
            .zip(state.identities(&identities)?)
            .collect(),
        constraints: state
            .constraints_on(&records)?
            .into_iter()
            .chain(state.constraints_of_members(&merged_away)?)
            .collect(),
        ..Pending::default()
    };

    for (index, op) in changeset.ops.iter().enumerate() {
        let at = Some((index, op));
        match op {
            Op::CreateIdentity {
                identity,
                entity_type,
            } => {
                let exists = pending.existing.get(identity).is_some_and(Option::is_some);
                if exists || pending.created.contains_key(identity) {
                    reject!(
                        at,
                        Reason::IdentityExists {
                            identity: *identity,
                        },
                    );
                } else {
                    pending.created.insert(*identity, entity_type);
                }
            }
            Op::Assign { record, from, to } => {
                let Some(record_type) = types.get(record).and_then(Option::as_ref) else {
                    reject!(at, Reason::UnknownRecord { record: *record },);
                    continue;
                };
                if let Some(first_op) = pending.assigned.get(record) {
                    reject!(
                        at,
                        Reason::RecordAssignedTwice {
                            record: *record,
                            first_op: *first_op,
                        },
                    );
                    continue;
                }
                pending.assigned.insert(*record, index);
                // A merge earlier in the changeset moved the members of the
                // identity it merged away.
                let before = assigned.get(record).copied().flatten();
                let moved = before.and_then(|identity| pending.merged.get(&identity).copied());
                let actual = match moved {
                    Some((into, _)) => Some(into),
                    None => before,
                };
                if actual != *from {
                    reject!(
                        at,
                        Reason::StaleAssignment {
                            record: *record,
                            expected: *from,
                            actual,
                        },
                    );
                    continue;
                }
                if let Some((_, first_op)) = moved {
                    reject!(
                        at,
                        Reason::RecordAssignedTwice {
                            record: *record,
                            first_op,
                        },
                    );
                    continue;
                }
                if from == to {
                    reject!(at, Reason::NoChange { record: *record },);
                    continue;
                }
                pending.placed.insert(*record, *to);
                let Some(target) = to else { continue };
                let identity_type = match pending.identity(target) {
                    Known::Missing => {
                        reject!(at, Reason::UnknownIdentity { identity: *target });
                        continue;
                    }
                    Known::Merged(merged_into) => {
                        reject!(
                            at,
                            Reason::IdentityNotActive {
                                identity: *target,
                                merged_into,
                            },
                        );
                        continue;
                    }
                    Known::Active(entity_type) => entity_type.clone(),
                };
                if identity_type != *record_type {
                    reject!(
                        at,
                        Reason::EntityTypeMismatch {
                            record: *record,
                            record_type: record_type.clone(),
                            identity: *target,
                            identity_type,
                        },
                    );
                    continue;
                }
                pending.received.entry(*target).or_insert(index);
            }
            Op::Merge { from, into } => {
                if from == into {
                    reject!(at, Reason::MergeIntoItself { identity: *from });
                    continue;
                }
                let mut entity_types = Vec::new();
                for identity in [from, into] {
                    match pending.identity(identity) {
                        Known::Missing => reject!(
                            at,
                            Reason::UnknownIdentity {
                                identity: *identity,
                            },
                        ),
                        Known::Merged(merged_into) => reject!(
                            at,
                            Reason::IdentityNotActive {
                                identity: *identity,
                                merged_into,
                            },
                        ),
                        Known::Active(entity_type) => entity_types.push(entity_type.clone()),
                    }
                }
                let [from_type, into_type] = entity_types.as_slice() else {
                    continue;
                };
                if from_type != into_type {
                    reject!(
                        at,
                        Reason::IdentityTypeMismatch {
                            from: *from,
                            from_type: from_type.clone(),
                            into: *into,
                            into_type: into_type.clone(),
                        },
                    );
                    continue;
                }
                if let Some(first_op) = pending.reinstated.get(from) {
                    reject!(
                        at,
                        Reason::IdentityChangedTwice {
                            identity: *from,
                            first_op: *first_op,
                        },
                    );
                    continue;
                }
                if let Some(first_op) = pending.received.get(from) {
                    reject!(
                        at,
                        Reason::MembersMovedTwice {
                            identity: *from,
                            first_op: *first_op,
                        },
                    );
                    continue;
                }
                let stale = [from, into].into_iter().find_map(|identity| {
                    let changed = changed.get(identity).copied().unwrap_or(Seq::ZERO);
                    (changed > changeset.base).then_some((*identity, changed))
                });
                if let Some((identity, changed)) = stale {
                    reject!(
                        at,
                        Reason::StaleMerge {
                            identity,
                            changed,
                            base: changeset.base,
                        },
                    );
                    continue;
                }
                pending.merged.insert(*from, (*into, index));
                pending.received.entry(*into).or_insert(index);
            }
            Op::Reinstate { identity } => {
                let first_op = pending
                    .merged
                    .get(identity)
                    .map(|(_, first_op)| *first_op)
                    .or_else(|| pending.reinstated.get(identity).copied());
                if let Some(first_op) = first_op {
                    reject!(
                        at,
                        Reason::IdentityChangedTwice {
                            identity: *identity,
                            first_op,
                        },
                    );
                    continue;
                }
                match pending.identity(identity) {
                    Known::Missing => reject!(
                        at,
                        Reason::UnknownIdentity {
                            identity: *identity,
                        },
                    ),
                    Known::Active(_) => reject!(
                        at,
                        Reason::IdentityIsActive {
                            identity: *identity,
                        },
                    ),
                    Known::Merged(_) => {
                        pending.reinstated.insert(*identity, index);
                    }
                }
            }
            Op::Constrain { kind, a, b } | Op::Unconstrain { kind, a, b } => {
                if a == b {
                    reject!(at, Reason::SameRecord { record: *a });
                    continue;
                }
                let known = |record: &RecordId| types.get(record).and_then(Option::as_ref);
                let (a_type, b_type) = match (known(a), known(b)) {
                    (Some(a_type), Some(b_type)) => (a_type, b_type),
                    (None, _) => {
                        reject!(at, Reason::UnknownRecord { record: *a });
                        continue;
                    }
                    (_, None) => {
                        reject!(at, Reason::UnknownRecord { record: *b });
                        continue;
                    }
                };
                if a_type != b_type {
                    reject!(
                        at,
                        Reason::ConstraintAcrossTypes {
                            a: *a,
                            a_type: a_type.clone(),
                            b: *b,
                            b_type: b_type.clone(),
                        },
                    );
                    continue;
                }
                let constraint = Constraint::new(*kind, *a, *b);
                if let Some(first_op) = pending.constrained.get(&constraint) {
                    reject!(
                        at,
                        Reason::ConstraintChangedTwice {
                            kind: *kind,
                            a: *a,
                            b: *b,
                            first_op: *first_op,
                        },
                    );
                    continue;
                }
                let in_force = pending.constraints.contains(&constraint);
                if matches!(op, Op::Constrain { .. }) {
                    // A pair has one constraint. Both kinds at once would
                    // hold only while one of the two has no identity.
                    let other = Constraint::new(
                        match kind {
                            ConstraintKind::MustLink => ConstraintKind::CannotLink,
                            ConstraintKind::CannotLink => ConstraintKind::MustLink,
                        },
                        *a,
                        *b,
                    );
                    let existing = if in_force {
                        Some(*kind)
                    } else if pending.constraints.contains(&other) {
                        Some(other.kind)
                    } else {
                        None
                    };
                    if let Some(kind) = existing {
                        reject!(at, Reason::ConstraintExists { kind, a: *a, b: *b },);
                        continue;
                    }
                    pending.constraints.insert(constraint);
                } else {
                    if !in_force {
                        reject!(
                            at,
                            Reason::UnknownConstraint {
                                kind: *kind,
                                a: *a,
                                b: *b,
                            },
                        );
                        continue;
                    }
                    pending.constraints.remove(&constraint);
                }
                pending.constrained.insert(constraint, index);
            }
        }
    }

    // Each operation held. What is left is the state they add up to: no
    // constraint in force may be broken by it.
    if rejections.is_empty() {
        let mut others: Vec<RecordId> = pending
            .constraints
            .iter()
            .flat_map(|constraint| [constraint.a, constraint.b])
            .filter(|record| !assigned.contains_key(record))
            .collect();
        others.sort_unstable();
        others.dedup();
        let found = state.assignments(&others)?;
        assigned.extend(others.into_iter().zip(found));

        // Where a record ends up, and the operation that moved it.
        let end = |record: &RecordId| -> (Option<IdentityId>, Option<usize>) {
            if let Some(to) = pending.placed.get(record) {
                return (*to, pending.assigned.get(record).copied());
            }
            let before = assigned.get(record).copied().flatten();
            match before.and_then(|identity| pending.merged.get(&identity)) {
                Some((into, index)) => (Some(*into), Some(*index)),
                None => (before, None),
            }
        };
        for constraint in &pending.constraints {
            let ((identity_a, moved_a), (identity_b, moved_b)) =
                (end(&constraint.a), end(&constraint.b));
            if constraint.holds(identity_a, identity_b) {
                continue;
            }
            // The last operation that had a part in it is the one to name.
            let index = [
                moved_a,
                moved_b,
                pending.constrained.get(constraint).copied(),
            ]
            .into_iter()
            .flatten()
            .max();
            reject!(
                index.map(|index| (index, &changeset.ops[index])),
                Reason::ConstraintViolated {
                    kind: constraint.kind,
                    a: constraint.a,
                    b: constraint.b,
                    identity_a,
                    identity_b,
                },
            );
        }
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
