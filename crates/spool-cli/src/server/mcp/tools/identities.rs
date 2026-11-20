//! Tools that find identities and read one: as it is, as it was, and its
//! history.

use schemars::JsonSchema;
use serde::Deserialize;
use spool::IdentityFilter;
use spool::core::{EntityType, IdentityId, Seq};

use super::{Declared, Effect, declare_tool, limit_or_default};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SearchIdentities {
    /// Only identities of this entity type.
    #[serde(default)]
    entity_type: Option<EntityType>,
    /// Only identities with a member whose label or record holds every
    /// word of this text. Case and accents do not matter.
    #[serde(default)]
    q: Option<String>,
    /// Only these identities. One that was merged away is not found here:
    /// `get_identity` says which identity stands for it.
    #[serde(default)]
    ids: Option<Vec<IdentityId>>,
    /// For the next page: the `next` of the page before.
    #[serde(default)]
    after: Option<IdentityId>,
    /// How many to return, 20 unless given.
    #[serde(default)]
    limit: Option<u64>,
}

pub(super) fn search_identities() -> Declared {
    declare_tool(
        "search_identities",
        "Search identities",
        Effect::Reads,
        "Find identities by name, domain or other record values. Results rank identity \
         labels first, then aliases (other member labels), then record values. Each \
         result's `match` says why it appeared, with the matching record and field when \
         available. Without `q` it lists identities in ID order. Follow with \
         `get_identity` or `search_records` for detail.",
        |app, _, asked: SearchIdentities| async move {
            let filter = IdentityFilter {
                entity_type: asked.entity_type,
                q: asked.q,
                ids: asked.ids,
                after: asked.after,
                limit: limit_or_default(asked.limit),
                sort: None,
            };
            app.list_identities(filter).await
        },
    )
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OneIdentity {
    identity: IdentityId,
}

pub(super) fn get_identity() -> Declared {
    declare_tool(
        "get_identity",
        "Get an identity",
        Effect::Reads,
        "One identity and the records that belong to it now, each with its source and \
         key. If the identity was merged into another, `current` is the one that stands \
         for it and the records are that one's. The values of each record are in \
         `search_records` with `identity`.",
        |app, _, asked: OneIdentity| async move { app.read_identity(asked.identity).await },
    )
}

pub(super) fn get_identity_history() -> Declared {
    declare_tool(
        "get_identity_history",
        "Get an identity's history",
        Effect::Reads,
        "Every change that touched one identity, oldest first: who made it, when, why, \
         and the operations that named the identity. Use this to answer how an identity \
         came to hold what it holds, and to find the `seq` of a change to undo.",
        |app, _, asked: OneIdentity| async move { app.identity_history(asked.identity).await },
    )
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct IdentityAt {
    identity: IdentityId,
    /// The inclusive changeset position, from `list_changesets` or identity history.
    seq: Seq,
}

pub(super) fn get_identity_at() -> Declared {
    declare_tool(
        "get_identity_at",
        "Get an identity at a changeset",
        Effect::Reads,
        "Read what an identity contained immediately after changeset `seq`. The response \
         gives the requested `id`, the survivor at that point as `current`, and its label \
         and members at that point. Position zero is the empty store.",
        |app, _, asked: IdentityAt| async move { app.identity_at(asked.identity, asked.seq).await },
    )
}
