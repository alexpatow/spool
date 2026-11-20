//! What a tool returns: the answer as text, and what is left of an answer
//! or an error that is too large for a model to read.

use axum::http::StatusCode;
use rmcp::model::{CallToolResult, ContentBlock, JsonObject};
use serde_json::Value;

use crate::api::{ErrorBody, ErrorCode, ErrorResponse};
use crate::server::error::{ApiError, ApiResult};

/// The largest result a tool returns. A result goes into what the model
/// reads, where a megabyte of JSON displaces everything else, and a client
/// cuts off what it cannot hold without saying where.
pub(super) const MAX_RESULT_BYTES: usize = 256 * 1024;

/// The largest field kept in an answer that was too large to return whole.
const MAX_FIELD_BYTES: usize = 4 * 1024;

/// The most the rejections of an error take when they cannot all be given.
/// The rest of an error is its message, which is then no longer than a
/// field, and as JSON no more than six times that.
const MAX_REJECTIONS_BYTES: usize = MAX_RESULT_BYTES - 8 * MAX_FIELD_BYTES;

/// The result of a tool: the JSON the API answers with, as text. A failure
/// is a result too, with the API's error in it, so that the model reads why
/// and can put it right.
pub(super) fn answer(result: ApiResult<String>, in_parts: bool) -> CallToolResult {
    let body = match result {
        Ok(body) => body,
        Err(error) => return refused(error),
    };
    let body = match body.len() > MAX_RESULT_BYTES {
        false => body,
        true if in_parts => {
            return refused(ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                ErrorCode::TooLarge,
                format!(
                    "the answer is {} bytes and a tool returns at most {MAX_RESULT_BYTES}: ask \
                     for less, with a smaller `limit` or a narrower search",
                    body.len()
                ),
            ));
        }
        // What a tool changed is changed, and saying that the call failed
        // would be untrue. The answer is returned without its large parts,
        // and so is a run, which cannot be asked for in parts.
        true => shortened(&body),
    };
    CallToolResult::success(vec![ContentBlock::text(body)])
}

/// An answer too large to return, without the parts that make it so: the
/// receipt of a change is small and the changeset it committed is not, and a
/// run's counts are small and the list of what it left for review is not.
fn shortened(body: &str) -> String {
    let whole = match serde_json::from_str(body) {
        Ok(Value::Object(answer)) => answer,
        _ => JsonObject::new(),
    };
    let mut left_out = Vec::new();
    let mut answer = whole.clone();
    prune(&mut answer, "", true, &mut left_out);
    // An object can be large by the number of its fields. Then nothing of
    // a large field is kept.
    let room = MAX_RESULT_BYTES - 2 * MAX_FIELD_BYTES;
    if size(&answer) + size(&left_out) > room {
        left_out.clear();
        answer = whole;
        prune(&mut answer, "", false, &mut left_out);
    }
    if size(&answer) + size(&left_out) > room {
        answer.clear();
        left_out = vec![Value::String("every field".to_owned())];
    }
    answer.insert("left_out".to_owned(), Value::Array(left_out));
    answer.insert(
        "note".to_owned(),
        Value::String(
            "The call succeeded. Its answer was too large to return whole, so the parts named \
             in `left_out` are missing. `get_identity`, `search_records` and \
             `list_review_items` read what the store holds now."
                .to_owned(),
        ),
    );
    serde_json::to_string(&Value::Object(answer)).unwrap_or_default()
}

/// Takes the large fields out of an object and names each in `left_out` by
/// its path. With `within`, a large object is kept, without its own large
/// fields.
pub(super) fn prune(object: &mut JsonObject, path: &str, within: bool, left_out: &mut Vec<Value>) {
    object.retain(|name, field| {
        if size(field) <= MAX_FIELD_BYTES {
            return true;
        }
        let at = format!("{path}{name}");
        match field {
            Value::Object(inner) if within => {
                prune(inner, &format!("{at}."), within, left_out);
                true
            }
            _ => {
                left_out.push(Value::String(at));
                false
            }
        }
    });
}

fn refused(error: ApiError) -> CallToolResult {
    let mut error = ErrorResponse { error: error.body };
    if size(&error) > MAX_RESULT_BYTES {
        error.error = shortened_error(error.error);
    }
    let body = serde_json::to_string(&error).unwrap_or_else(|_| "the call failed".to_owned());
    CallToolResult::error(vec![ContentBlock::text(body)])
}

/// An error too large to return, cut to what a tool returns. A changeset can
/// be rejected for every one of its operations, and its error then repeats
/// each of them, twice: in `rejections` and in the message. Any other error
/// is large only by repeating at length what the caller sent.
fn shortened_error(mut error: ErrorBody) -> ErrorBody {
    if let Some(rejections) = &mut error.rejections {
        let all = rejections.len();
        let mut room = MAX_REJECTIONS_BYTES;
        let kept = rejections
            .iter()
            .take_while(|rejection| {
                // One more for the comma between two of them.
                let takes = size(rejection).saturating_add(1);
                let fits = takes <= room;
                room = room.saturating_sub(takes);
                fits
            })
            .count();
        rejections.truncate(kept);
        error.message = format!(
            "changeset rejected for {all} reasons, which is more than a tool returns: \
             `rejections` has the first {kept}. Nothing was changed. Put these right, or commit \
             fewer operations at a time, to see the rest."
        );
    }
    let kept = beginning(&error.message).len();
    error.message.truncate(kept);
    error
}

/// As much of the beginning of a text as a field holds.
pub(super) fn beginning(text: &str) -> &str {
    let end = (0..=text.len().min(MAX_FIELD_BYTES))
        .rev()
        .find(|&end| text.is_char_boundary(end))
        .unwrap_or(0);
    &text[..end]
}

/// How many bytes a value is as JSON.
pub(super) fn size(value: &impl serde::Serialize) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX, |json| json.len())
}
