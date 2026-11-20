//! Tools for changesets: committing one, undoing one, splitting an
//! identity, the constraints that stand, the log, and IDs to commit with.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use spool::ConstraintFilter;
use spool::core::{IdentityId, Op, RecordId, Seq};

use super::{DEFAULT_LIMIT, Declared, Effect, declare, limit_or_default};
use crate::api::{CommitRequest, MintRequest, SplitRequest, UndoRequest};
use crate::server::App;
use crate::server::auth::Caller;
use crate::server::error::{ApiError, ApiResult};
use crate::server::mcp::limits::{MAX_RESULT_BYTES, prune, size};
use crate::server::work::engine;

/// An operation has at most three UUIDs and no free text, so a thousand
/// fit in a tool result even when every operation uses all three.
const MAX_LOG_OPS: u64 = 1_000;

/// The most IDs `mint_identity_ids` mints in one call.
const MAX_MINTED: u32 = 100;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Constraints {
    /// Only constraints that name this record.
    #[serde(default)]
    record: Option<RecordId>,
    /// Only constraints that name a record of this identity.
    #[serde(default)]
    identity: Option<IdentityId>,
}

pub(super) fn list_constraints() -> Declared {
    declare(
        "list_constraints",
        "List constraints",
        Effect::Reads,
        "The decisions in force that two records must share an identity (`must_link`) \
         or must not (`cannot_link`). A changeset that would break one is refused. Give \
         `record` or `identity` to see only those that bear on it.",
        |app, _, asked: Constraints| async move {
            let filter = ConstraintFilter {
                record: asked.record,
                identity: asked.identity,
                include_removed: false,
            };
            app.list_constraints(filter).await
        },
    )
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Log {
    /// Only changesets after this log position. Left out, from the start.
    #[serde(default)]
    after: Option<Seq>,
    /// Maximum summaries to return, 20 unless given and 1000 at most.
    #[serde(default)]
    limit: Option<u64>,
}

pub(super) fn list_changesets() -> Declared {
    declare(
        "list_changesets",
        "List changes",
        Effect::Reads,
        "Summaries of committed changesets, oldest first. Each gives its log position `seq`, \
         author, reason, run, and `operation_count`. `started_by` says who asked for a run. \
         `head` is the position of the latest commit. Pass `next` as `after` until `next` is \
         null. Use `list_changeset_ops` with a summary's `seq` to read its operations.",
        read_changeset_summaries,
    )
}

async fn read_changeset_summaries(app: App, _: Caller, asked: Log) -> ApiResult<Value> {
    let after = asked.after.unwrap_or(Seq::ZERO);
    let limit = limit_or_default(asked.limit);
    let (head, summaries) =
        engine(&app, move |engine| engine.changeset_summaries(after, limit)).await?;
    // Reserve space for the envelope and the longest possible cursor.
    let mut room =
        MAX_RESULT_BYTES - size(&json!({ "changesets": [], "head": head, "next": u64::MAX }));
    let mut changesets = Vec::new();
    let mut next = None;
    for summary in summaries {
        let mut entry = json!({
            "seq": summary.seq,
            "changeset": {
                "id": summary.id,
                "base": summary.base,
                "actor": summary.actor,
                "reason": summary.reason,
                "run": summary.run,
            },
            "content_hash": summary.content_hash,
            "started_by": summary.started_by,
            "operation_count": summary.operation_count,
        });
        let mut left_out = Vec::new();
        prune(entry.as_object_mut().unwrap(), "", true, &mut left_out);
        if !left_out.is_empty() {
            entry["left_out"] = json!(left_out);
        }
        // Also allow a byte for the comma that separates entries.
        let takes = size(&entry) + 1;
        if takes > room {
            break;
        }
        room -= takes;
        next = (summary.seq < head).then_some(summary.seq);
        changesets.push(entry);
    }
    Ok(json!({ "changesets": changesets, "head": head, "next": next }))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LogOps {
    /// The committed changeset's log position, `seq` from `list_changesets`.
    seq: Seq,
    /// Zero-based operation offset. Omit for the first page, then pass back `next`.
    #[serde(default)]
    offset: Option<u64>,
    /// Maximum operations to return, 20 unless given and 1000 at most.
    #[serde(default)]
    limit: Option<u64>,
}

pub(super) fn list_changeset_ops() -> Declared {
    declare(
        "list_changeset_ops",
        "List changeset operations",
        Effect::Reads,
        "Operations of one committed changeset in their original order. Give its `seq` from \
         `list_changesets`. Pass `next` as `offset` until `next` is null.",
        read_changeset_ops,
    )
}

async fn read_changeset_ops(app: App, _: Caller, asked: LogOps) -> ApiResult<Value> {
    let seq = asked.seq;
    let offset = asked.offset.unwrap_or(0);
    let limit = asked.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LOG_OPS);
    let mut ops = engine(&app, move |engine| {
        engine.changeset_ops(seq, offset, limit + 1)
    })
    .await?;
    let next = if ops.len() as u64 > limit {
        ops.pop();
        Some(offset + limit)
    } else {
        None
    };
    Ok(json!({ "seq": seq, "offset": offset, "ops": ops, "next": next }))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MintIdentityIds {
    /// How many IDs, 1 unless given, 100 at most.
    #[serde(default)]
    count: Option<u32>,
}

pub(super) fn mint_identity_ids() -> Declared {
    declare(
        "mint_identity_ids",
        "Mint identity IDs",
        Effect::Reads,
        "New identity IDs for a `create_identity` operation. An ID that is never used \
         costs nothing. Changes nothing in the store.",
        |app, _, asked: MintIdentityIds| async move {
            let count = asked.count.unwrap_or(1);
            if count > MAX_MINTED {
                return Err(ApiError::invalid(format!(
                    "at most {MAX_MINTED} identity IDs are minted in one call"
                )));
            }
            let request = MintRequest {
                identities: count,
                changesets: 0,
            };
            app.mint_ids(request).await
        },
    )
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CommitChangeset {
    /// The operations, applied in order, all of them or none.
    ///
    /// `{"op":"merge","from":A,"into":B}` moves every record of identity A
    /// to identity B; A stands for B from then on.
    ///
    /// `{"op":"assign","record":R,"from":X,"to":Y}` moves record R from
    /// identity X to identity Y. `from` must be the identity R has now,
    /// `null` if it has none; `to` is `null` to leave R without an
    /// identity.
    ///
    /// `{"op":"create_identity","identity":I,"entity_type":T}` makes an
    /// empty identity. I comes from `mint_identity_ids`; `assign` records
    /// to it in the same changeset.
    ///
    /// `{"op":"constrain","kind":"cannot_link","a":R1,"b":R2}` records that
    /// two records must never share an identity; with `must_link`, that
    /// they always must. `unconstrain` with the same fields removes one.
    ///
    /// `{"op":"reinstate","identity":I}` makes an identity that was merged
    /// away active again, with no records.
    ops: Vec<Op>,
    /// Why, in a sentence. It is kept in the log beside the change.
    reason: String,
    /// The `head` of the store when you read what you are changing. With
    /// it, a merge is refused if either identity's records changed since.
    /// Left out, the change is made to the store as it is now.
    #[serde(default)]
    base: Option<Seq>,
}

pub(super) fn commit_changeset() -> Declared {
    declare(
        "commit_changeset",
        "Change identities",
        Effect::Changes,
        "Change identities: merge two, move a record to another identity, place a \
         record that has none, make a new identity, or record that two records must or \
         must not share an identity. The server checks every operation against the \
         store and commits all of them or none; a refusal lists the operations that \
         broke a rule and changes nothing. Placing the latest version of a row moves \
         its earlier versions along, as operations of the same change. The change is \
         recorded under this connection's name and can be taken back with \
         `undo_changeset`. Returns the \
         `seq` it was committed at.",
        |app, caller, asked: CommitChangeset| async move {
            let request = CommitRequest {
                id: None,
                base: asked.base,
                reason: Some(asked.reason),
                run: None,
                ops: asked.ops,
            };
            app.commit_changeset(caller, request).await
        },
    )
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SplitIdentity {
    /// The identity to divide.
    identity: IdentityId,
    /// The records of one side. The rest of the identity's records are the
    /// other side. The larger side keeps the identity's ID and the smaller
    /// gets a new one.
    records: Vec<RecordId>,
    /// Record that the two sides must not share an identity, so that a
    /// later match does not put them back together. True unless given.
    #[serde(default)]
    keep_apart: Option<bool>,
    /// Why, in a sentence. It is kept in the log beside the change.
    reason: String,
    /// The `head` of the store when you read the identity. With it, the
    /// split is refused if the identity's records changed since.
    #[serde(default)]
    base: Option<Seq>,
}

pub(super) fn split_identity() -> Declared {
    declare(
        "split_identity",
        "Split an identity",
        Effect::Changes,
        "Divide one identity in two when it holds records of two different things. Name \
         the records of one side; the rest stay. Unless `keep_apart` is false, the two \
         sides are also recorded as never to be merged. Returns the new identity and \
         the changeset that was committed.",
        |app, caller, asked: SplitIdentity| async move {
            let request = SplitRequest {
                records: asked.records,
                keep_apart: asked.keep_apart,
                base: asked.base,
                reason: Some(asked.reason),
            };
            app.split_identity(caller, asked.identity, request).await
        },
    )
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UndoChangeset {
    /// The log position of the changeset to take back: its `seq`.
    seq: Seq,
    /// Why, in a sentence. It is kept in the log beside the change.
    reason: String,
    /// False to see the changeset that would undo it without committing
    /// it. True unless given.
    #[serde(default)]
    commit: Option<bool>,
}

pub(super) fn undo_changeset() -> Declared {
    declare(
        "undo_changeset",
        "Undo a change",
        Effect::Changes,
        "Take back one committed changeset by committing its opposite, which is a new \
         entry in the log. Refused if later changes built on it. With `commit` false it \
         shows the changeset that would undo it and changes nothing.",
        |app, caller, asked: UndoChangeset| async move {
            let request = UndoRequest {
                reason: Some(asked.reason),
                commit: asked.commit,
            };
            app.undo_changeset(caller, asked.seq, request).await
        },
    )
}
