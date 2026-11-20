//! Mentions: what an extractor wrote, checked against the documents it
//! was given and turned into records.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::{Value, json};
use spool_core::{DocumentId, RecordId};

use crate::error::{Error, Result};
use crate::project::Extractor;

/// A document an extractor was given.
pub(crate) struct TaskDocument {
    pub source: String,
    pub key: String,
    pub uri: Option<String>,
    pub text: String,
}

/// A row of what an extractor wrote, before anything about it is known to
/// be right.
pub(crate) struct RawMention {
    pub document: Option<String>,
    pub entity_type: Option<String>,
    pub start: Option<i64>,
    pub end: Option<i64>,
    /// JSON text.
    pub attributes: Option<String>,
}

/// A mention that passed every check, as the record it becomes.
#[derive(Debug)]
pub(crate) struct Mention {
    pub document: DocumentId,
    pub entity_type: String,
    pub source: String,
    pub record: RecordId,
    /// The locator.
    pub key: String,
    /// The attributes in canonical form.
    pub payload: String,
    pub evidence: String,
}

/// The key of the record a mention becomes.
pub fn locator(document: DocumentId, entity_type: &str, start: u64, end: u64) -> String {
    format!("{document}#{entity_type}:{start}-{end}")
}

/// Checks what an extractor wrote against the documents it was given, and
/// turns each mention into a record. One mention that is wrong
/// refuses them all: an extractor that points outside its document, or
/// names a type the project does not declare, is not to be half believed.
pub(crate) fn stage_mentions(
    raw: &[RawMention],
    documents: &HashMap<DocumentId, TaskDocument>,
    declared: &dyn Fn(&str) -> bool,
    extractor: &Extractor,
) -> Result<Vec<Mention>> {
    // Where each code point of a document begins, worked out for the
    // documents that have mentions.
    let mut boundaries: HashMap<DocumentId, Vec<usize>> = HashMap::new();
    let mut seen = HashSet::new();
    let mut staged = Vec::with_capacity(raw.len());

    for (index, mention) in raw.iter().enumerate() {
        let fail = |message: String| Error::Mentions(format!("row {}: {message}", index + 1));
        let need = |name: &str| fail(format!("it has no `{name}`"));

        let named = mention
            .document
            .as_deref()
            .ok_or_else(|| need("document_id"))?;
        let found = named
            .parse::<DocumentId>()
            .ok()
            .and_then(|id| documents.get(&id).map(|document| (id, document)));
        let Some((id, document)) = found else {
            return Err(fail(format!(
                "`{named}` is not a document the extractor was given"
            )));
        };
        let entity_type = mention
            .entity_type
            .as_deref()
            .ok_or_else(|| need("entity_type"))?;
        if !declared(entity_type) {
            return Err(fail(format!(
                "the project declares no entity type `{entity_type}`"
            )));
        }

        let starts = boundaries.entry(id).or_insert_with(|| {
            document
                .text
                .char_indices()
                .map(|(at, _)| at)
                .chain(std::iter::once(document.text.len()))
                .collect()
        });
        let length = (starts.len() - 1) as i64;
        let start = mention.start.ok_or_else(|| need("start"))?;
        let end = mention.end.ok_or_else(|| need("end"))?;
        if start < 0 || start >= end || end > length {
            return Err(fail(format!(
                "{start}-{end} is not a span of document `{}`, which is {length} long; \
                 `start` and `end` count Unicode code points, and `end` comes after the last one",
                document.key
            )));
        }
        let (start, end) = (start as usize, end as usize);
        if !seen.insert((id, entity_type, start, end)) {
            return Err(fail(format!(
                "document `{}` already has a mention of a `{entity_type}` at {start}-{end}",
                document.key
            )));
        }

        let payload = canonicalize_attributes(mention.attributes.as_deref()).map_err(fail)?;
        let key = locator(id, entity_type, start as u64, end as u64);
        let mut evidence = json!({
            "document": id,
            "key": document.key,
            "start": start,
            "end": end,
            "text": &document.text[starts[start]..starts[end]],
            "extractor": extractor.name,
            "version": extractor.hash,
        });
        if let Some(uri) = &document.uri {
            evidence["uri"] = json!(uri);
        }
        staged.push(Mention {
            document: id,
            entity_type: entity_type.to_owned(),
            source: document.source.clone(),
            record: RecordId::derive(&document.source, Some(&key), &payload),
            key,
            payload,
            evidence: evidence.to_string(),
        });
    }
    Ok(staged)
}

/// A mention's attributes in the form a row's payload has: an object of
/// text, booleans and numbers, ordered by name, without the attributes that
/// have no value. A whole number is an integer however it was written.
fn canonicalize_attributes(attributes: Option<&str>) -> Result<String, String> {
    let parsed: Value = match attributes {
        None => Value::Null,
        Some(text) => serde_json::from_str(text)
            .map_err(|error| format!("`attributes` is not valid JSON: {error}"))?,
    };
    let object = match parsed {
        Value::Null => serde_json::Map::new(),
        Value::Object(object) => object,
        _ => return Err("`attributes` must be a JSON object".to_owned()),
    };
    let mut payload = BTreeMap::new();
    for (name, value) in object {
        let value = match value {
            Value::Null => continue,
            Value::String(_) | Value::Bool(_) => value,
            Value::Number(number) => match number.as_f64() {
                Some(float)
                    if !number.is_i64()
                        && !number.is_u64()
                        && float.fract() == 0.0
                        && float.abs() < 9_007_199_254_740_992.0 =>
                {
                    Value::from(float as i64)
                }
                _ => Value::Number(number),
            },
            Value::Array(_) | Value::Object(_) => {
                return Err(format!(
                    "attribute `{name}` is nested; an attribute is text, a boolean or a number"
                ));
            }
        };
        payload.insert(name, value);
    }
    serde_json::to_string(&payload)
        .map_err(|error| format!("the attributes cannot be encoded: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::Runtime;

    fn extractor() -> Extractor {
        Extractor {
            name: "names".to_owned(),
            runtime: Runtime::Python,
            file: "extractors/names.py".to_owned(),
            hash: "abc".to_owned(),
        }
    }

    fn given(text: &str) -> (DocumentId, HashMap<DocumentId, TaskDocument>) {
        let id = DocumentId::derive("news", "a.txt", text);
        let document = TaskDocument {
            source: "news".to_owned(),
            key: "a.txt".to_owned(),
            uri: Some("https://example.test/a".to_owned()),
            text: text.to_owned(),
        };
        (id, HashMap::from([(id, document)]))
    }

    fn raw(document: DocumentId, start: i64, end: i64, attributes: &str) -> RawMention {
        RawMention {
            document: Some(document.to_string()),
            entity_type: Some("company".to_owned()),
            start: Some(start),
            end: Some(end),
            attributes: Some(attributes.to_owned()),
        }
    }

    fn stage_one(text: &str, mention: impl Fn(DocumentId) -> RawMention) -> Result<Vec<Mention>> {
        let (id, documents) = given(text);
        stage_mentions(
            &[mention(id)],
            &documents,
            &|name| name == "company",
            &extractor(),
        )
    }

    fn refused(text: &str, mention: impl Fn(DocumentId) -> RawMention) -> String {
        match stage_one(text, mention) {
            Err(Error::Mentions(message)) => message,
            other => panic!("expected the mention to be refused, got {other:?}"),
        }
    }

    #[test]
    fn a_span_counts_code_points_and_the_evidence_quotes_it() {
        // An accent is two bytes, and the rocket is four and two UTF-16 units.
        let text = "🚀 Société Générale a ouvert un bureau.";
        let staged =
            stage_one(text, |id| raw(id, 2, 18, r#"{"name":"Société Générale"}"#)).unwrap();
        let mention = &staged[0];
        let (id, _) = given(text);
        assert_eq!(mention.key, format!("{id}#company:2-18"));
        assert_eq!(mention.payload, r#"{"name":"Société Générale"}"#);
        let evidence: Value = serde_json::from_str(&mention.evidence).unwrap();
        assert_eq!(
            evidence,
            json!({
                "document": id,
                "key": "a.txt",
                "uri": "https://example.test/a",
                "start": 2,
                "end": 18,
                "text": "Société Générale",
                "extractor": "names",
                "version": "abc",
            })
        );
        assert_eq!(
            mention.record,
            RecordId::derive("news", Some(&mention.key), &mention.payload)
        );
        // The whole text is a span, and its end is the text's length.
        let length = text.chars().count() as i64;
        assert!(stage_one(text, |id| raw(id, 0, length, "{}")).is_ok());
    }

    #[test]
    fn a_mention_outside_its_document_is_refused() {
        let text = "Acme opened an office.";
        for (start, end) in [(-1, 4), (4, 4), (5, 4), (0, 23), (22, 23)] {
            let message = refused(text, |id| raw(id, start, end, "{}"));
            assert!(
                message.starts_with("row 1: ") && message.contains("is not a span"),
                "{message}"
            );
        }
    }

    #[test]
    fn a_mention_must_name_a_document_of_the_task_and_a_declared_type() {
        let text = "Acme opened an office.";
        let other = DocumentId::derive("news", "b.txt", text);
        let message = refused(text, |_| raw(other, 0, 4, "{}"));
        assert!(
            message.contains("not a document the extractor was given"),
            "{message}"
        );
        let message = refused(text, |id| RawMention {
            document: Some("somewhere".to_owned()),
            ..raw(id, 0, 4, "{}")
        });
        assert!(
            message.contains("not a document the extractor was given"),
            "{message}"
        );

        let message = refused(text, |id| RawMention {
            entity_type: Some("person".to_owned()),
            ..raw(id, 0, 4, "{}")
        });
        assert!(message.contains("no entity type `person`"), "{message}");
        let message = refused(text, |id| RawMention {
            start: None,
            ..raw(id, 0, 4, "{}")
        });
        assert!(message.contains("no `start`"), "{message}");
    }

    #[test]
    fn attributes_have_the_form_of_a_rows_payload() {
        let text = "Acme opened an office.";
        let payload = |attributes: &str| {
            stage_one(text, |id| raw(id, 0, 4, attributes)).unwrap()[0]
                .payload
                .clone()
        };
        assert_eq!(
            payload(
                r#"{"name": "Acme", "employees": 12.0, "listed": true, "ratio": 0.5, "note": null}"#
            ),
            r#"{"employees":12,"listed":true,"name":"Acme","ratio":0.5}"#
        );
        // The order of the names and the spelling of a whole number do not
        // change the record.
        assert_eq!(
            payload(r#"{"employees": 12, "name": "Acme"}"#),
            payload(r#"{"name": "Acme", "employees": 1.2e1}"#)
        );
        assert_eq!(payload("null"), "{}");

        let message = refused(text, |id| raw(id, 0, 4, r#"{"names": ["Acme"]}"#));
        assert!(message.contains("attribute `names` is nested"), "{message}");
        let message = refused(text, |id| raw(id, 0, 4, r#"["Acme"]"#));
        assert!(message.contains("must be a JSON object"), "{message}");
        let message = refused(text, |id| raw(id, 0, 4, "{"));
        assert!(message.contains("not valid JSON"), "{message}");
    }

    #[test]
    fn one_place_holds_one_mention_of_a_type() {
        let text = "Acme opened an office.";
        let (id, documents) = given(text);
        let error = stage_mentions(
            &[
                raw(id, 0, 4, r#"{"name":"Acme"}"#),
                raw(id, 0, 4, r#"{"name":"ACME"}"#),
            ],
            &documents,
            &|_| true,
            &extractor(),
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("row 2: document `a.txt` already has a mention"),
            "{error}"
        );
    }
}
