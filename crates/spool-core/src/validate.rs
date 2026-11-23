//! The rules every changeset must pass before a store applies it.

use serde::{Deserialize, Serialize};

use crate::changeset::{Changeset, ConstraintKind, ContentHash, Op};
use crate::ids::{EntityType, IdentityId, RecordId, Seq};
use crate::state::{IdentityState, StateError};

mod check;
use check::{Broken, Check};

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
