//! Tools for review items: listing them, the case for one, and dismissing
//! one.

use schemars::JsonSchema;
use serde::Deserialize;
use spool::core::{EntityType, RecordId};
use spool::{ReviewFilter, ReviewStatus};

use super::{Declared, Effect, declare_tool};
use crate::api::DismissRequest;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReviewItems {
    /// Only items in this state, `open` unless given.
    #[serde(default)]
    status: Option<ReviewStatus>,
    /// Only items about records of this entity type.
    #[serde(default)]
    entity_type: Option<EntityType>,
    /// Only items that hold this record back.
    #[serde(default)]
    record: Option<RecordId>,
}

pub(super) fn list_review_items() -> Declared {
    declare_tool(
        "list_review_items",
        "List review items",
        Effect::Reads,
        "The cases the server left for someone to decide. `possible_merge` and \
         `conflict` are records it could not place, because they match more than one \
         identity or because the evidence about them contradicts itself. \
         `possible_split` is an identity whose records nothing holds together since a \
         source took back one of its rows: `groups` are the parts that still hold. \
         Lists the open ones unless `status` says otherwise.",
        |app, _, asked: ReviewItems| async move {
            let filter = ReviewFilter {
                status: Some(asked.status.unwrap_or(ReviewStatus::Open)),
                entity_type: asked.entity_type,
                record: asked.record,
                ..ReviewFilter::default()
            };
            app.list_review(filter).await
        },
    )
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OneReviewItem {
    /// The `id` of the review item.
    item: u64,
}

pub(super) fn get_review_case() -> Declared {
    declare_tool(
        "get_review_case",
        "Get a review case",
        Effect::Reads,
        "One review item with what is needed to decide it: the records it holds back, \
         the identities they matched with their records, what was judged between them, \
         and the constraints that limit the decision. Decide it with `commit_changeset`, \
         by assigning each record to an identity or merging the identities, or close it \
         with `dismiss_review_item`. Leave out a record whose `retracted` is set: its \
         source took it back, it does not wait for a decision, and a changeset that \
         gives it an identity is refused. A `possible_split` is different: its records \
         share one identity, in `candidates`, and `taken_back` has the records whose \
         retraction left the question, with what was judged between them and the \
         records that remain. If the groups are different things, divide the identity \
         with `split_identity`, naming the records of one group. If they are one thing, \
         record a `must_link` between a record of each group with `commit_changeset`, \
         which closes the item, or close it with `dismiss_review_item`.",
        |app, _, asked: OneReviewItem| async move { app.review_case(asked.item).await },
    )
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DismissReviewItem {
    /// The `id` of the review item.
    item: u64,
    /// Why, in a sentence. It is kept with the item.
    reason: String,
}

pub(super) fn dismiss_review_item() -> Declared {
    declare_tool(
        "dismiss_review_item",
        "Dismiss a review item",
        Effect::Changes,
        "Close a review item without placing its records, when it needs no decision. \
         The records stay without an identity. Dismissing a `possible_split` leaves its \
         identity as it is. Recorded with who dismissed it and why.",
        |app, caller, asked: DismissReviewItem| async move {
            let request = DismissRequest {
                reason: Some(asked.reason),
            };
            app.dismiss_review(caller, asked.item, request).await
        },
    )
}
