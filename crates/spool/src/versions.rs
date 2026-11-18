//! The versions of what a source holds under a key: a row, or a document.
//!
//! What a source sends under a key it has used before is the next version
//! of that row, and names the version it supersedes. The versions of a key
//! are a chain, and the last of them is what the source holds now.
//!
//! An ID is derived from content, so content alone cannot say where a
//! version stands. A row that goes from A to B and back to A has three
//! versions and two contents. The third is told from the first by what it
//! follows: its ID is derived from the content's ID and from the ID of the
//! version before it. Content that a key holds for the first time keeps the
//! ID derived from the content alone.
//!
//! What a source sends is ignored when it is what the key holds already:
//! the content of its latest version.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::str::FromStr;
use std::sync::Arc;

use duckdb::arrow::array::{ArrayRef, AsArray, StringArray, UInt32Array};
use duckdb::arrow::compute::take;
use duckdb::arrow::datatypes::Int64Type;
use duckdb::arrow::record_batch::RecordBatch;

use crate::error::Result;
use crate::store::parse;

/// What the store holds that bears on one thing a source sent.
pub(crate) struct Held<Id> {
    /// The ID derived from its content is taken.
    pub known: bool,
    /// The latest version under its key.
    pub latest: Option<Latest<Id>>,
}

pub(crate) struct Latest<Id> {
    pub id: Id,
    /// The version before it.
    pub supersedes: Option<Id>,
}

/// One thing to store.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Version<Id> {
    /// Its place among what was sent.
    pub at: usize,
    pub id: Id,
    pub supersedes: Option<Id>,
}

/// Decides what to store of what a source sent, in the order it was sent.
///
/// `sent` is the key and the ID derived from the content of each thing,
/// `held` what the store has for those it has something for, by place, and
/// `again` the ID of a version that holds a content again after the version
/// named.
///
/// Something without a key has no versions: it is stored once.
pub(crate) fn place<'a, Id: Copy + Eq + Hash>(
    sent: impl Iterator<Item = (Option<&'a str>, Id)>,
    held: &HashMap<usize, Held<Id>>,
    again: impl Fn(Id, Id) -> Id,
) -> Vec<Version<Id>> {
    // The latest version of each key that was sent, and the ID derived from
    // its content.
    let mut latest: HashMap<&str, (Id, Id)> = HashMap::new();
    let mut taken: HashSet<Id> = HashSet::new();
    let mut versions = Vec::new();

    for (at, (key, content)) in sent.enumerate() {
        let held = held.get(&at);
        let known = held.is_some_and(|held| held.known) || taken.contains(&content);
        let Some(key) = key else {
            if !known {
                taken.insert(content);
                versions.push(Version {
                    at,
                    id: content,
                    supersedes: None,
                });
            }
            continue;
        };

        let before = match latest.get(key) {
            Some((id, holds)) => Some((*id, *holds == content)),
            None => held.and_then(|held| held.latest.as_ref()).map(|latest| {
                let same = latest.id == content
                    || latest
                        .supersedes
                        .is_some_and(|before| again(content, before) == latest.id);
                (latest.id, same)
            }),
        };
        let id = match before {
            // What the key holds already.
            Some((before, true)) => {
                latest.insert(key, (before, content));
                continue;
            }
            Some((before, false)) if known => again(content, before),
            // The content is taken and its key has no version. A store
            // that is in order has no such record, since the content's ID
            // covers the key. It is stored once all the same.
            None if known => continue,
            _ => content,
        };
        taken.insert(id);
        latest.insert(key, (id, content));
        versions.push(Version {
            at,
            id,
            supersedes: before.map(|(before, _)| before),
        });
    }
    versions
}

/// The staged rows that are to be stored, each with its ID and the version
/// it supersedes.
///
/// `staged` has a column `ord`, a column `id` of the IDs derived from
/// content, and `source_key`. `held` is what a query found in the store for
/// the staged rows it has something for: `ord`, whether the ID is taken,
/// the latest version under the key, and the version before that.
pub(crate) fn of_staged<Id>(
    staged: &RecordBatch,
    id: &str,
    mut held: duckdb::Rows<'_>,
    again: impl Fn(Id, Id) -> Id,
) -> Result<Vec<Version<Id>>>
where
    Id: Copy + Eq + Hash + FromStr,
    Id::Err: std::error::Error + Send + Sync + 'static,
{
    let ords = column(staged, "ord").as_primitive::<Int64Type>();
    let at: HashMap<i64, usize> = ords.values().iter().copied().zip(0..).collect();
    let mut found = HashMap::new();
    while let Some(row) = held.next()? {
        let read = |index: usize| -> Result<Option<Id>> {
            row.get::<_, Option<String>>(index)?
                .map(|id| parse(&id))
                .transpose()
        };
        let latest = match read(2)? {
            Some(latest) => Some(Latest {
                id: latest,
                supersedes: read(3)?,
            }),
            None => None,
        };
        found.insert(
            at[&row.get::<_, i64>(0)?],
            Held {
                known: row.get(1)?,
                latest,
            },
        );
    }

    let keys = column(staged, "source_key").as_string::<i32>();
    let mut contents = Vec::with_capacity(staged.num_rows());
    for id in column(staged, id).as_string::<i32>().iter() {
        contents.push(parse::<Id>(id.unwrap_or_default())?);
    }
    Ok(place(keys.iter().zip(contents), &found, again))
}

/// The versions as a batch to insert: `id` and `supersedes`, and the
/// columns `kept` of the staged rows they came from.
pub(crate) fn to_store<Id: Copy + ToString>(
    staged: &RecordBatch,
    id: &str,
    versions: &[Version<Id>],
    kept: &[&str],
) -> Result<RecordBatch> {
    let text = |value: fn(&Version<Id>) -> Option<Id>| -> ArrayRef {
        Arc::new(StringArray::from_iter(
            versions
                .iter()
                .map(|version| value(version).map(|id| id.to_string())),
        ))
    };
    let chosen = UInt32Array::from_iter_values(versions.iter().map(|version| version.at as u32));
    let mut columns = vec![
        (id, text(|version| Some(version.id))),
        ("supersedes", text(|version| version.supersedes)),
    ];
    for name in kept {
        columns.push((name, take(column(staged, name), &chosen, None)?));
    }
    Ok(RecordBatch::try_from_iter_with_nullable(
        columns
            .into_iter()
            .map(|(name, values)| (name, values, true)),
    )?)
}

fn column<'a>(staged: &'a RecordBatch, name: &str) -> &'a ArrayRef {
    staged
        .column_by_name(name)
        .unwrap_or_else(|| panic!("a staged batch has a column `{name}`"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A content is a small number, and a version that holds it again is
    /// a number no content has.
    fn again(content: u32, follows: u32) -> u32 {
        1000 + content * 100 + follows % 100
    }

    fn version(at: usize, id: u32, supersedes: Option<u32>) -> Version<u32> {
        Version { at, id, supersedes }
    }

    #[test]
    fn what_a_key_holds_for_the_first_time_keeps_the_id_of_its_content() {
        let sent = [(Some("a"), 1), (Some("b"), 2), (Some("a"), 3)];
        assert_eq!(
            place(sent.into_iter(), &HashMap::new(), again),
            vec![
                version(0, 1, None),
                version(1, 2, None),
                version(2, 3, Some(1))
            ]
        );
    }

    #[test]
    fn what_a_key_holds_already_is_ignored() {
        let held = HashMap::from([(
            0,
            Held {
                known: true,
                latest: Some(Latest {
                    id: 1,
                    supersedes: None,
                }),
            },
        )]);
        let sent = [
            (Some("a"), 1),
            (Some("a"), 1),
            (Some("b"), 2),
            (Some("b"), 2),
        ];
        assert_eq!(
            place(sent.into_iter(), &held, again),
            vec![version(2, 2, None)]
        );
    }

    #[test]
    fn a_key_that_holds_again_what_it_held_has_a_version_more() {
        // The store has 1 and then 2 under the key, and 1 is sent again.
        let held = HashMap::from([(
            0,
            Held {
                known: true,
                latest: Some(Latest {
                    id: 2,
                    supersedes: Some(1),
                }),
            },
        )]);
        let third = again(1, 2);
        assert_eq!(
            place([(Some("a"), 1)].into_iter(), &held, again),
            vec![version(0, third, Some(2))]
        );

        // Sent once more, it is what the key holds: the latest version is
        // the one that held it again.
        let held = HashMap::from([(
            0,
            Held {
                known: true,
                latest: Some(Latest {
                    id: third,
                    supersedes: Some(2),
                }),
            },
        )]);
        assert_eq!(place([(Some("a"), 1)].into_iter(), &held, again), vec![]);

        // And 2 after it is a version of its own again.
        let held = HashMap::from([(
            0,
            Held {
                known: true,
                latest: Some(Latest {
                    id: third,
                    supersedes: Some(2),
                }),
            },
        )]);
        assert_eq!(
            place([(Some("a"), 2)].into_iter(), &held, again),
            vec![version(0, again(2, third), Some(third))]
        );
    }

    #[test]
    fn versions_sent_together_follow_each_other() {
        let sent = [
            (Some("a"), 1),
            (Some("a"), 2),
            (Some("a"), 1),
            (Some("a"), 1),
            (Some("a"), 2),
        ];
        let third = again(1, 2);
        let fourth = again(2, third);
        assert_eq!(
            place(sent.into_iter(), &HashMap::new(), again),
            vec![
                version(0, 1, None),
                version(1, 2, Some(1)),
                version(2, third, Some(2)),
                version(4, fourth, Some(third)),
            ]
        );
    }

    #[test]
    fn what_has_no_key_is_stored_once() {
        let held = HashMap::from([(
            1,
            Held {
                known: true,
                latest: None,
            },
        )]);
        let sent = [(None, 1), (None, 2), (None, 1), (None, 3)];
        assert_eq!(
            place(sent.into_iter(), &held, again),
            vec![version(0, 1, None), version(3, 3, None)]
        );
    }
}
