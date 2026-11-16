//! Identifier types.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Identifies an observation. Derived from the observation's source, its key
/// within that source and its content, so ingesting the same thing twice
/// yields the same ID.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(from = "Uuid", into = "Uuid")]
pub struct ObservationId(u128);

impl ObservationId {
    /// The first 128 bits of a SHA-256 over the three parts. Each part is
    /// length-prefixed, so no two different inputs share an encoding.
    pub fn derive(source_id: &str, source_key: Option<&str>, canonical_payload: &str) -> Self {
        let mut hasher = Sha256::new();
        put_str(&mut hasher, source_id);
        match source_key {
            Some(key) => {
                hasher.update([1]);
                put_str(&mut hasher, key);
            }
            None => hasher.update([0]),
        }
        put_str(&mut hasher, canonical_payload);
        let digest = hasher.finalize();
        let mut first = [0u8; 16];
        first.copy_from_slice(&digest[..16]);
        Self(u128::from_be_bytes(first))
    }

    pub const fn from_u128(value: u128) -> Self {
        Self(value)
    }

    pub const fn as_u128(self) -> u128 {
        self.0
    }
}

fn put_str(hasher: &mut Sha256, value: &str) {
    hasher.update((value.len() as u64).to_le_bytes());
    hasher.update(value.as_bytes());
}

impl From<Uuid> for ObservationId {
    fn from(value: Uuid) -> Self {
        Self(value.as_u128())
    }
}

impl From<ObservationId> for Uuid {
    fn from(value: ObservationId) -> Self {
        Uuid::from_u128(value.0)
    }
}

impl fmt::Display for ObservationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Uuid::from_u128(self.0).fmt(f)
    }
}

impl fmt::Debug for ObservationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ObservationId({self})")
    }
}

impl FromStr for ObservationId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self::from)
    }
}

macro_rules! uuid_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            pub const fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }

            pub const fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self.0)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(s).map(Self)
            }
        }
    };
}

uuid_id!(
    /// Identifies an identity. Minted once by an [`IdGenerator`](crate::IdGenerator),
    /// never derived from content and never reused.
    IdentityId
);

uuid_id!(
    /// Identifies a changeset. Minted by the changeset's author.
    ChangesetId
);

/// A position in the changeset log. The first committed changeset is 1, and 0
/// is the empty log.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Seq(pub u64);

impl Seq {
    pub const ZERO: Seq = Seq(0);

    pub const fn next(self) -> Seq {
        Seq(self.0 + 1)
    }
}

impl fmt::Display for Seq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Identifies a run of the engine: an ingest, a match or a reconcile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunId(pub u64);

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// The name of an entity type a project declares, such as `company` or
/// `person`. The engine defines none. A name becomes part of a table name, so
/// it is limited to a lowercase letter followed by lowercase letters, digits
/// and underscores, 48 characters at most.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct EntityType(String);

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error(
    "invalid entity type name {0:?}: use a lowercase letter followed by lowercase letters, digits or underscores, 48 characters at most"
)]
pub struct InvalidEntityType(pub String);

impl EntityType {
    pub const MAX_LEN: usize = 48;

    pub fn new(name: impl Into<String>) -> Result<Self, InvalidEntityType> {
        let name = name.into();
        let mut chars = name.chars();
        let starts_with_letter = chars.next().is_some_and(|c| c.is_ascii_lowercase());
        let rest_ok = chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if starts_with_letter && rest_ok && name.len() <= Self::MAX_LEN {
            Ok(Self(name))
        } else {
            Err(InvalidEntityType(name))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for EntityType {
    type Error = InvalidEntityType;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<EntityType> for String {
    fn from(value: EntityType) -> Self {
        value.0
    }
}

impl fmt::Display for EntityType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Who authored a changeset. Written as `engine`, `user:<id>` or `agent:<id>`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum Actor {
    Engine,
    User(String),
    Agent(String),
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("invalid actor {0:?}: expected `engine`, `user:<id>` or `agent:<id>`")]
pub struct InvalidActor(pub String);

impl fmt::Display for Actor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Actor::Engine => f.write_str("engine"),
            Actor::User(id) => write!(f, "user:{id}"),
            Actor::Agent(id) => write!(f, "agent:{id}"),
        }
    }
}

impl FromStr for Actor {
    type Err = InvalidActor;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.split_once(':') {
            None if s == "engine" => Ok(Actor::Engine),
            Some(("user", id)) if !id.is_empty() => Ok(Actor::User(id.to_owned())),
            Some(("agent", id)) if !id.is_empty() => Ok(Actor::Agent(id.to_owned())),
            _ => Err(InvalidActor(s.to_owned())),
        }
    }
}

impl TryFrom<String> for Actor {
    type Error = InvalidActor;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<Actor> for String {
    fn from(value: Actor) -> Self {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observation_id_depends_on_every_part() {
        let base = ObservationId::derive("crm", Some("1"), r#"{"name":"Acme"}"#);
        assert_eq!(
            base,
            ObservationId::derive("crm", Some("1"), r#"{"name":"Acme"}"#)
        );
        assert_ne!(
            base,
            ObservationId::derive("registry", Some("1"), r#"{"name":"Acme"}"#)
        );
        assert_ne!(
            base,
            ObservationId::derive("crm", Some("2"), r#"{"name":"Acme"}"#)
        );
        assert_ne!(
            base,
            ObservationId::derive("crm", None, r#"{"name":"Acme"}"#)
        );
        assert_ne!(
            base,
            ObservationId::derive("crm", Some("1"), r#"{"name":"Acme Ltd"}"#)
        );
    }

    #[test]
    fn observation_id_parts_do_not_run_together() {
        assert_ne!(
            ObservationId::derive("ab", Some("c"), "{}"),
            ObservationId::derive("a", Some("bc"), "{}")
        );
        assert_ne!(
            ObservationId::derive("a", Some(""), "{}"),
            ObservationId::derive("a", None, "{}")
        );
    }

    #[test]
    fn observation_id_round_trips_as_a_uuid_string() {
        let id = ObservationId::derive("crm", Some("1"), "{}");
        assert_eq!(id.to_string().parse::<ObservationId>().unwrap(), id);
        assert_eq!(id.to_string().len(), 36);
    }

    #[test]
    fn entity_type_names_are_restricted() {
        for ok in ["company", "person", "a", "product_2", &"a".repeat(48)] {
            assert!(EntityType::new(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "Company",
            "2fast",
            "_x",
            "my-type",
            "a b",
            "a;drop",
            &"a".repeat(49),
        ] {
            assert!(EntityType::new(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn actor_round_trips() {
        for text in ["engine", "user:ada", "agent:resolver-1"] {
            assert_eq!(text.parse::<Actor>().unwrap().to_string(), text);
        }
        for bad in ["", "user:", "robot:1", "engine:1"] {
            assert!(bad.parse::<Actor>().is_err(), "{bad}");
        }
    }
}
