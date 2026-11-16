//! The rules every changeset must pass before a store applies it.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::changeset::{Changeset, ContentHash, Op};
use crate::ids::{EntityType, IdentityId, ObservationId, Seq};
use crate::state::{IdentityState, IdentityStatus, StateError};

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
pub enum Reason {
    #[error("the changeset has no operations")]
    EmptyChangeset,
    #[error("the changeset is based on log position {base}, and the log ends at {head}")]
    BaseAhead { base: Seq, head: Seq },
    #[error("the changeset was already committed at log position {seq}")]
    AlreadyCommitted { seq: Seq },
    #[error("identity {identity} already exists; an identity ID is never reused")]
    IdentityExists { identity: IdentityId },
    #[error("observation {observation} is not known")]
    UnknownObservation { observation: ObservationId },
    #[error("identity {identity} does not exist")]
    UnknownIdentity { identity: IdentityId },
    #[error("identity {identity} is not active: it was merged into {merged_into}")]
    IdentityNotActive {
        identity: IdentityId,
        merged_into: IdentityId,
    },
    #[error(
        "stale precondition: observation {observation} was expected to belong to {}, and belongs to {}",
        display_identity(.expected),
        display_identity(.actual)
    )]
    StaleAssignment {
        observation: ObservationId,
        expected: Option<IdentityId>,
        actual: Option<IdentityId>,
    },
    #[error("observation {observation} would stay where it is")]
    NoChange { observation: ObservationId },
    #[error(
        "observation {observation} is a {observation_type} and identity {identity} is a {identity_type}"
    )]
    EntityTypeMismatch {
        observation: ObservationId,
        observation_type: EntityType,
        identity: IdentityId,
        identity_type: EntityType,
    },
    #[error(
        "observation {observation} was already assigned by operation {first_op}; a changeset may assign an observation once"
    )]
    ObservationAssignedTwice {
        observation: ObservationId,
        first_op: usize,
    },
    #[error("`{kind}` operations are not implemented yet")]
    NotImplemented { kind: String },
}

fn display_identity(identity: &Option<IdentityId>) -> String {
    match identity {
        Some(identity) => identity.to_string(),
        None => "no identity".to_owned(),
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
/// - an observation belongs to at most one identity, and is assigned only to
///   an active identity of its own entity type;
/// - an identity ID is never reused;
/// - a changeset assigns an observation at most once, because membership
///   history has one position per changeset.
pub fn validate<S: IdentityState + ?Sized>(
    state: &S,
    changeset: Changeset,
) -> Result<Validated, ValidationError> {
    let mut rejections = Vec::new();
    let mut reject = |op: Option<(usize, &Op)>, reason: Reason| {
        rejections.push(Rejection {
            op_index: op.map(|(index, _)| index),
            op: op.map(|(_, op)| op.clone()),
            reason,
        });
    };

    if changeset.ops.is_empty() {
        reject(None, Reason::EmptyChangeset);
    }
    let head = state.head()?;
    if changeset.base > head {
        reject(
            None,
            Reason::BaseAhead {
                base: changeset.base,
                head,
            },
        );
    }
    if let Some(seq) = state.changeset_seq(changeset.id)? {
        reject(None, Reason::AlreadyCommitted { seq });
    }

    let mut observations = Vec::new();
    let mut identities = Vec::new();
    for op in &changeset.ops {
        match op {
            Op::CreateIdentity { identity, .. } => identities.push(*identity),
            Op::Assign {
                observation, to, ..
            } => {
                observations.push(*observation);
                identities.extend(*to);
            }
            _ => {}
        }
    }
    observations.sort_unstable();
    observations.dedup();
    identities.sort_unstable();
    identities.dedup();

    let types: HashMap<_, _> = observations
        .iter()
        .copied()
        .zip(state.observation_types(&observations)?)
        .collect();
    let assigned: HashMap<_, _> = observations
        .iter()
        .copied()
        .zip(state.assignments(&observations)?)
        .collect();
    let existing: HashMap<_, _> = identities
        .iter()
        .copied()
        .zip(state.identities(&identities)?)
        .collect();

    let mut created: HashMap<IdentityId, &EntityType> = HashMap::new();
    let mut assigned_by: HashMap<ObservationId, usize> = HashMap::new();

    for (index, op) in changeset.ops.iter().enumerate() {
        let at = Some((index, op));
        match op {
            Op::CreateIdentity {
                identity,
                entity_type,
            } => {
                let exists = existing.get(identity).is_some_and(Option::is_some);
                if exists || created.contains_key(identity) {
                    reject(
                        at,
                        Reason::IdentityExists {
                            identity: *identity,
                        },
                    );
                } else {
                    created.insert(*identity, entity_type);
                }
            }
            Op::Assign {
                observation,
                from,
                to,
            } => {
                let Some(observation_type) = types.get(observation).and_then(Option::as_ref) else {
                    reject(
                        at,
                        Reason::UnknownObservation {
                            observation: *observation,
                        },
                    );
                    continue;
                };
                if let Some(first_op) = assigned_by.insert(*observation, index) {
                    assigned_by.insert(*observation, first_op);
                    reject(
                        at,
                        Reason::ObservationAssignedTwice {
                            observation: *observation,
                            first_op,
                        },
                    );
                    continue;
                }
                let actual = assigned.get(observation).copied().flatten();
                if actual != *from {
                    reject(
                        at,
                        Reason::StaleAssignment {
                            observation: *observation,
                            expected: *from,
                            actual,
                        },
                    );
                    continue;
                }
                if from == to {
                    reject(
                        at,
                        Reason::NoChange {
                            observation: *observation,
                        },
                    );
                    continue;
                }
                let Some(target) = to else { continue };
                let identity_type = if let Some(entity_type) = created.get(target) {
                    *entity_type
                } else {
                    match existing.get(target).and_then(Option::as_ref) {
                        None => {
                            reject(at, Reason::UnknownIdentity { identity: *target });
                            continue;
                        }
                        Some(record) => match record.status {
                            IdentityStatus::MergedInto(merged_into) => {
                                reject(
                                    at,
                                    Reason::IdentityNotActive {
                                        identity: *target,
                                        merged_into,
                                    },
                                );
                                continue;
                            }
                            IdentityStatus::Active => &record.entity_type,
                        },
                    }
                };
                if identity_type != observation_type {
                    reject(
                        at,
                        Reason::EntityTypeMismatch {
                            observation: *observation,
                            observation_type: observation_type.clone(),
                            identity: *target,
                            identity_type: identity_type.clone(),
                        },
                    );
                }
            }
            Op::Merge { .. }
            | Op::Constrain { .. }
            | Op::Unconstrain { .. }
            | Op::Reinstate { .. } => reject(
                at,
                Reason::NotImplemented {
                    kind: op.kind().to_owned(),
                },
            ),
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
