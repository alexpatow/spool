//! The order a list is read in, when its caller chooses one, and how a page
//! of it continues from the last entry of the page before.
//!
//! A list sorted by a column is still read in pages that continue after an
//! entry: the next page holds what stands after that entry's own values in
//! the order, so the place does not move when entries are added before it.
//! Each order ends with the entry's ID, so that no two entries stand in the
//! same place.

use duckdb::vtab::arrow::ArrowBatchRegistration;
use serde::{Deserialize, Serialize};
use spool_core::Actor;

use crate::batch::{Column, build_batch};
use crate::error::Result;

/// Which way a list runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Direction {
    /// The smallest, the earliest or the first in the alphabet first.
    #[default]
    #[serde(rename = "asc")]
    Ascending,
    #[serde(rename = "desc")]
    Descending,
}

/// What a list is sorted by, and which way.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Sort<By> {
    pub by: By,
    pub direction: Direction,
}

impl<By> Sort<By> {
    pub fn ascending(by: By) -> Self {
        Self {
            by,
            direction: Direction::Ascending,
        }
    }

    pub fn descending(by: By) -> Self {
        Self {
            by,
            direction: Direction::Descending,
        }
    }
}

/// One value an entry is sorted by, as SQL that is never null.
#[derive(Clone)]
pub(crate) struct Key {
    sql: String,
    /// Whether it turns with the direction of the list. Whether a value is
    /// missing does not: what has none comes last either way.
    turns: bool,
}

impl Key {
    pub(crate) fn of(sql: impl Into<String>) -> Self {
        Self {
            sql: sql.into(),
            turns: true,
        }
    }

    /// Whether a value that may be null is missing, which puts the entries
    /// that have none after the others whichever way the list runs. The
    /// keys after it compare the value with `coalesce`.
    pub(crate) fn missing_last(sql: &str) -> Self {
        Self {
            sql: format!("{sql} IS NULL"),
            turns: false,
        }
    }

    /// Text in the order a reader expects: whatever the case, and words
    /// that differ only in case the same way each time. `sql` is never null.
    pub(crate) fn text(sql: &str) -> [Self; 2] {
        [Self::of(format!("lower({sql})")), Self::of(sql)]
    }

    /// Text that may be null: missing last, then as [`text`](Self::text).
    pub(crate) fn optional_text(sql: &str) -> [Self; 3] {
        let [lower, exact] = Self::text(&format!("coalesce({sql}, '')"));
        [Self::missing_last(sql), lower, exact]
    }
}

/// The values every entry of a list is sorted by, and where in that order
/// the page continues.
///
/// It is two common table expressions: `keyed`, with the ID of each entry
/// as `id` and its values as `sort_0`, `sort_1`, …, and `anchor`, the row
/// of `keyed` for the entry the page continues after. The list's own query
/// joins `keyed` on its ID and is ordered by [`order_by`](Self::order_by).
pub(crate) struct Keyed {
    /// More common table expressions that `from` reads, without `WITH`.
    pub before: Option<String>,
    /// The tables the values are read from, with their joins.
    pub from: String,
    /// The SQL of an entry's ID in `from`.
    pub id: String,
    /// The values, the most significant first. The last is the ID.
    pub keys: Vec<Key>,
    pub direction: Direction,
}

impl Keyed {
    /// `WITH` and the two expressions. `after` is the SQL of the ID the
    /// page continues after, null for the first page.
    pub(crate) fn with(&self, after: &str) -> String {
        let values: Vec<String> = self
            .keys
            .iter()
            .enumerate()
            .map(|(at, key)| format!("{} AS sort_{at}", key.sql))
            .collect();
        let before = self
            .before
            .as_deref()
            .map(|before| format!("{before},\n"))
            .unwrap_or_default();
        format!(
            "WITH {before}keyed AS (SELECT {} AS id, {} FROM {}),
             anchor AS (SELECT * FROM keyed WHERE keyed.id = {after})",
            self.id,
            values.join(", "),
            self.from,
        )
    }

    /// The joins that give a row of the list its values, and the anchor.
    pub(crate) fn join(&self, id: &str) -> String {
        format!("JOIN keyed ON keyed.id = {id} LEFT JOIN anchor ON true")
    }

    fn descending(&self, key: &Key) -> bool {
        key.turns && self.direction == Direction::Descending
    }

    /// Whether a row stands after the anchor. `continues` is the SQL of
    /// whether there is one to stand after.
    pub(crate) fn after(&self, continues: &str) -> String {
        let mut either = Vec::new();
        for (at, key) in self.keys.iter().enumerate() {
            let mut all: Vec<String> = (0..at)
                .map(|before| format!("keyed.sort_{before} = anchor.sort_{before}"))
                .collect();
            let beyond = if self.descending(key) { "<" } else { ">" };
            all.push(format!("keyed.sort_{at} {beyond} anchor.sort_{at}"));
            either.push(format!("({})", all.join(" AND ")));
        }
        format!("(NOT ({continues}) OR {})", either.join(" OR "))
    }

    pub(crate) fn order_by(&self) -> String {
        let keys: Vec<String> = self
            .keys
            .iter()
            .enumerate()
            .map(|(at, key)| {
                let way = if self.descending(key) { "DESC" } else { "ASC" };
                format!("keyed.sort_{at} {way}")
            })
            .collect();
        keys.join(", ")
    }
}

/// What the people named by their account are called: `user:<id>` and a
/// name. A list sorted by who did something sorts them by that, and an
/// actor not named here by the actor itself, as each is shown.
pub type ActorNames = Vec<(Actor, String)>;

/// A common table expression `called`, of `actor` and `name`, read from the
/// table that the parameter `param` is bound to with [`names_table`].
pub(crate) fn actor_names_cte(param: &str) -> String {
    format!("called AS (SELECT actor, name FROM arrow({param}))")
}

/// The names as a table to bind to the parameter of [`actor_names_cte`]. With none,
/// it is a row of nulls, which names nobody.
pub(crate) fn names_table(names: &[(Actor, String)]) -> Result<ArrowBatchRegistration> {
    let (actors, called): (Vec<Option<String>>, Vec<Option<String>>) = if names.is_empty() {
        (vec![None], vec![None])
    } else {
        names
            .iter()
            .map(|(actor, name)| (Some(actor.to_string()), Some(name.clone())))
            .unzip()
    };
    Ok(ArrowBatchRegistration::new(build_batch(vec![
        ("actor", Column::Text(actors)),
        ("name", Column::Text(called)),
    ])?))
}
