//! Identifier types.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Identifies a record. Derived from the record's source, its key
/// within that source and its content, so loading the same thing twice
/// yields the same ID.
///
/// One case takes more: a row that holds again what it held before, with
/// something else in between. That version is a record of its own, and its
/// ID is derived from the content's and from the version it follows.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(from = "Uuid", into = "Uuid")]
pub struct RecordId(u128);

impl RecordId {
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

    /// The ID of a version that holds the content this ID was derived
    /// from, when a version with that content exists already: the first
    /// 128 bits of a SHA-256 over this ID and the ID of the version it
    /// follows. The input begins with eight bytes that no other ID's does.
    pub fn held_again(self, follows: Self) -> Self {
        Self(u128::from_be_bytes(again(
            [0xfe; 8],
            self.0.to_be_bytes(),
            follows.0.to_be_bytes(),
        )))
    }

    pub const fn from_u128(value: u128) -> Self {
        Self(value)
    }

    pub const fn as_u128(self) -> u128 {
        self.0
    }
}

fn again(marker: [u8; 8], content: [u8; 16], follows: [u8; 16]) -> [u8; 16] {
    let mut hasher = Sha256::new();
    hasher.update(marker);
    hasher.update(content);
    hasher.update(follows);
    let digest = hasher.finalize();
    let mut first = [0u8; 16];
    first.copy_from_slice(&digest[..16]);
    first
}

fn put_str(hasher: &mut Sha256, value: &str) {
    hasher.update((value.len() as u64).to_le_bytes());
    hasher.update(value.as_bytes());
}

impl From<Uuid> for RecordId {
    fn from(value: Uuid) -> Self {
        Self(value.as_u128())
    }
}

impl From<RecordId> for Uuid {
    fn from(value: RecordId) -> Self {
        Uuid::from_u128(value.0)
    }
}

impl fmt::Display for RecordId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Uuid::from_u128(self.0).fmt(f)
    }
}

impl fmt::Debug for RecordId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RecordId({self})")
    }
}

impl FromStr for RecordId {
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

uuid_id!(
    /// Identifies one version of a document. Derived from the document's
    /// source, its key within that source and its text, so storing the same
    /// text twice yields the same ID, and a changed text yields another. A
    /// version that holds a text again, with another in between, has an ID
    /// derived from the text's and from the version it follows.
    DocumentId
);

impl DocumentId {
    /// The first 128 bits of a SHA-256 over the three parts, each
    /// length-prefixed. The input begins with eight bytes that no
    /// record's does, so a document never shares a record's ID.
    pub fn derive(source_id: &str, source_key: &str, text: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update([0xff; 8]);
        put_str(&mut hasher, source_id);
        put_str(&mut hasher, source_key);
        put_str(&mut hasher, text);
        let digest = hasher.finalize();
        let mut first = [0u8; 16];
        first.copy_from_slice(&digest[..16]);
        Self(Uuid::from_bytes(first))
    }

    /// The ID of a version that holds the text this ID was derived from,
    /// when a version with that text exists already. As
    /// [`RecordId::held_again`], with eight bytes of its own in front.
    pub fn held_again(self, follows: Self) -> Self {
        Self(Uuid::from_bytes(again(
            [0xfd; 8],
            self.0.into_bytes(),
            follows.0.into_bytes(),
        )))
    }
}

/// A position in the changeset log. The first committed changeset is 1, and 0
/// is the empty log.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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

/// Identifies a run of the engine: a load, a match or a reconcile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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

/// How the types with a hand-written text form appear in the JSON Schema of
/// the API.
#[cfg(feature = "schema")]
mod schema {
    use std::borrow::Cow;

    use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};

    use super::{Actor, ChangesetId, DocumentId, EntityType, IdentityId, RecordId};

    macro_rules! text_schema {
        ($type:ty, $name:literal, $schema:tt) => {
            impl JsonSchema for $type {
                fn schema_name() -> Cow<'static, str> {
                    Cow::Borrowed($name)
                }

                fn json_schema(_: &mut SchemaGenerator) -> Schema {
                    json_schema!($schema)
                }
            }
        };
    }

    text_schema!(RecordId, "RecordId", {
        "description": "Identifies a record. Derived by the engine from the record's source, key and content, and for a version that holds what its row held before, from the version it follows as well.",
        "type": "string",
        "format": "uuid"
    });
    text_schema!(IdentityId, "IdentityId", {
        "description": "Identifies an identity. Minted once, never derived from content and never reused.",
        "type": "string",
        "format": "uuid"
    });
    text_schema!(ChangesetId, "ChangesetId", {
        "description": "Identifies a changeset, and the draft it was while it waited.",
        "type": "string",
        "format": "uuid"
    });
    text_schema!(DocumentId, "DocumentId", {
        "description": "Identifies one version of a document. Derived by the engine from the document's source, key and text, and for a version that holds a text the document held before, from the version it follows as well.",
        "type": "string",
        "format": "uuid"
    });
    text_schema!(EntityType, "EntityType", {
        "description": "The name of an entity type the project declares.",
        "type": "string",
        "pattern": "^[a-z][a-z0-9_]{0,47}$"
    });
    text_schema!(Actor, "Actor", {
        "description": "Who authored a changeset: `engine`, `user:<id>` or `agent:<id>`.",
        "type": "string",
        "pattern": "^(engine|user:.+|agent:.+)$"
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_id_depends_on_every_part_and_is_no_records() {
        let base = DocumentId::derive("news", "a", "Acme opened an office.");
        assert_eq!(
            base,
            DocumentId::derive("news", "a", "Acme opened an office.")
        );
        assert_ne!(
            base,
            DocumentId::derive("wire", "a", "Acme opened an office.")
        );
        assert_ne!(
            base,
            DocumentId::derive("news", "b", "Acme opened an office.")
        );
        assert_ne!(
            base,
            DocumentId::derive("news", "a", "Acme closed an office.")
        );
        // The same three texts as a record's parts are another ID.
        let record = RecordId::derive("news", Some("a"), "Acme opened an office.");
        assert_ne!(base.to_string(), record.to_string());
    }

    #[test]
    fn record_id_depends_on_every_part() {
        let base = RecordId::derive("crm", Some("1"), r#"{"name":"Acme"}"#);
        assert_eq!(
            base,
            RecordId::derive("crm", Some("1"), r#"{"name":"Acme"}"#)
        );
        assert_ne!(
            base,
            RecordId::derive("registry", Some("1"), r#"{"name":"Acme"}"#)
        );
        assert_ne!(
            base,
            RecordId::derive("crm", Some("2"), r#"{"name":"Acme"}"#)
        );
        assert_ne!(base, RecordId::derive("crm", None, r#"{"name":"Acme"}"#));
        assert_ne!(
            base,
            RecordId::derive("crm", Some("1"), r#"{"name":"Acme Ltd"}"#)
        );
    }

    #[test]
    fn a_version_held_again_has_an_id_of_its_own() {
        let first = RecordId::derive("crm", Some("1"), r#"{"name":"Acme"}"#);
        let second = RecordId::derive("crm", Some("1"), r#"{"name":"Acme Ltd"}"#);
        let third = first.held_again(second);
        assert_eq!(third, first.held_again(second));
        assert_ne!(third, first);
        assert_ne!(third, second);
        // What it follows is part of it, and so is which of the two it holds.
        let fourth = second.held_again(third);
        assert_ne!(first.held_again(fourth), third);
        assert_ne!(second.held_again(first), third);

        let text = DocumentId::derive("news", "a", "Acme opened an office.");
        let other = DocumentId::derive("news", "a", "Acme closed an office.");
        let again = text.held_again(other);
        assert_eq!(again, text.held_again(other));
        assert_ne!(again, text);
        assert_ne!(again, other.held_again(text));
        // A document's is not a record's, from the same two IDs.
        let as_records = RecordId::from(text.as_uuid()).held_again(RecordId::from(other.as_uuid()));
        assert_ne!(again.to_string(), as_records.to_string());
    }

    #[test]
    fn record_id_parts_do_not_run_together() {
        assert_ne!(
            RecordId::derive("ab", Some("c"), "{}"),
            RecordId::derive("a", Some("bc"), "{}")
        );
        assert_ne!(
            RecordId::derive("a", Some(""), "{}"),
            RecordId::derive("a", None, "{}")
        );
    }

    #[test]
    fn record_id_round_trips_as_a_uuid_string() {
        let id = RecordId::derive("crm", Some("1"), "{}");
        assert_eq!(id.to_string().parse::<RecordId>().unwrap(), id);
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
