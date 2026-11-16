//! Operations and changesets: the only way identity state changes.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ids::{Actor, ChangesetId, EntityType, IdentityId, ObservationId, RunId, Seq};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum ConstraintKind {
    MustLink,
    CannotLink,
}

impl ConstraintKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            ConstraintKind::MustLink => "must_link",
            ConstraintKind::CannotLink => "cannot_link",
        }
    }
}

/// One change to identity state. Each operation carries its own precondition,
/// which is checked against the state the changeset is committed to.
///
/// `Merge`, `Constrain`, `Unconstrain` and `Reinstate` are part of the format
/// and are not implemented yet: a changeset that contains one is rejected.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum Op {
    /// Precondition: the ID has never been used.
    CreateIdentity {
        identity: IdentityId,
        entity_type: EntityType,
    },
    /// Attach (`from` is `None`), move, or detach (`to` is `None`).
    /// Precondition: `from` is the observation's current identity.
    Assign {
        observation: ObservationId,
        from: Option<IdentityId>,
        to: Option<IdentityId>,
    },
    /// Moves every current member of `from` to `into` and redirects `from`.
    Merge { from: IdentityId, into: IdentityId },
    Constrain {
        kind: ConstraintKind,
        a: ObservationId,
        b: ObservationId,
    },
    Unconstrain {
        kind: ConstraintKind,
        a: ObservationId,
        b: ObservationId,
    },
    /// Makes a merged identity active again; used only to undo a merge.
    Reinstate { identity: IdentityId },
}

impl Op {
    /// The operation's name in the log.
    pub const fn kind(&self) -> &'static str {
        match self {
            Op::CreateIdentity { .. } => "create_identity",
            Op::Assign { .. } => "assign",
            Op::Merge { .. } => "merge",
            Op::Constrain { .. } => "constrain",
            Op::Unconstrain { .. } => "unconstrain",
            Op::Reinstate { .. } => "reinstate",
        }
    }

    fn encode(&self, out: &mut Sha256) {
        fn identity(out: &mut Sha256, id: IdentityId) {
            out.update(id.as_uuid().as_bytes());
        }
        fn observation(out: &mut Sha256, id: ObservationId) {
            out.update(id.as_u128().to_be_bytes());
        }
        fn optional(out: &mut Sha256, id: Option<IdentityId>) {
            match id {
                Some(id) => {
                    out.update([1]);
                    identity(out, id);
                }
                None => out.update([0]),
            }
        }
        fn constraint(out: &mut Sha256, kind: ConstraintKind) {
            out.update([match kind {
                ConstraintKind::MustLink => 1,
                ConstraintKind::CannotLink => 2,
            }]);
        }
        match self {
            Op::CreateIdentity {
                identity: id,
                entity_type,
            } => {
                out.update([1]);
                identity(out, *id);
                out.update((entity_type.as_str().len() as u64).to_le_bytes());
                out.update(entity_type.as_str().as_bytes());
            }
            Op::Assign {
                observation: obs,
                from,
                to,
            } => {
                out.update([2]);
                observation(out, *obs);
                optional(out, *from);
                optional(out, *to);
            }
            Op::Merge { from, into } => {
                out.update([3]);
                identity(out, *from);
                identity(out, *into);
            }
            Op::Constrain { kind, a, b } => {
                out.update([4]);
                constraint(out, *kind);
                observation(out, *a);
                observation(out, *b);
            }
            Op::Unconstrain { kind, a, b } => {
                out.update([5]);
                constraint(out, *kind);
                observation(out, *a);
                observation(out, *b);
            }
            Op::Reinstate { identity: id } => {
                out.update([6]);
                identity(out, *id);
            }
        }
    }
}

/// An ordered list of operations that is applied atomically or not at all.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Changeset {
    pub id: ChangesetId,
    /// The log position the author had seen.
    pub base: Seq,
    pub actor: Actor,
    pub reason: Option<String>,
    /// The reconcile run that drafted it, if any.
    pub run: Option<RunId>,
    pub ops: Vec<Op>,
}

impl Changeset {
    /// A hash of the operations alone, in order. Two changesets with the same
    /// operations have the same hash whoever authored them.
    pub fn content_hash(&self) -> ContentHash {
        ContentHash::of(&self.ops)
    }
}

/// SHA-256 of the canonical encoding of a list of operations, as lowercase hex.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ContentHash(String);

impl ContentHash {
    pub fn of(ops: &[Op]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"spool.ops.v1\0");
        hasher.update((ops.len() as u64).to_le_bytes());
        for op in ops {
            op.encode(&mut hasher);
        }
        let hex = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Self(hex)
    }

    /// Wraps a hash read back from a store.
    pub fn from_stored(hex: String) -> Self {
        Self(hex)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ContentHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generate::{IdGenerator, SeededIds};

    fn company() -> EntityType {
        EntityType::new("company").unwrap()
    }

    #[test]
    fn content_hash_covers_operations_and_their_order() {
        let mut ids = SeededIds::new(1);
        let (x, y) = (ids.identity_id(), ids.identity_id());
        let obs = ObservationId::from_u128(9);
        let create = |identity| Op::CreateIdentity {
            identity,
            entity_type: company(),
        };
        let attach = |to| Op::Assign {
            observation: obs,
            from: None,
            to: Some(to),
        };

        let hash = ContentHash::of(&[create(x), attach(x)]);
        assert_eq!(hash, ContentHash::of(&[create(x), attach(x)]));
        assert_eq!(hash.as_str().len(), 64);
        assert_ne!(hash, ContentHash::of(&[attach(x), create(x)]));
        assert_ne!(hash, ContentHash::of(&[create(y), attach(y)]));
        assert_ne!(hash, ContentHash::of(&[create(x)]));
        assert_ne!(
            ContentHash::of(&[Op::Assign {
                observation: obs,
                from: Some(x),
                to: None
            }]),
            ContentHash::of(&[attach(x)])
        );
    }

    #[test]
    fn content_hash_ignores_authorship() {
        let mut ids = SeededIds::new(1);
        let ops = vec![Op::CreateIdentity {
            identity: ids.identity_id(),
            entity_type: company(),
        }];
        let first = Changeset {
            id: ids.changeset_id(),
            base: Seq(0),
            actor: Actor::Engine,
            reason: None,
            run: None,
            ops: ops.clone(),
        };
        let second = Changeset {
            id: ids.changeset_id(),
            base: Seq(4),
            actor: Actor::User("ada".into()),
            reason: Some("by hand".into()),
            run: Some(RunId(3)),
            ops,
        };
        assert_eq!(first.content_hash(), second.content_hash());
    }
}
