//! What the attributes of the records hold, across all of them: for each
//! entity type and source, how often each attribute has a value, how many
//! values it takes, the ones most records have, and the features the
//! project computes from it.
//!
//! Only the records that stand count: the version of each row or mention
//! that nothing replaced, and not one its source took back.

use std::collections::BTreeMap;

use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};
use spool_core::{EntityType, StateError};

use crate::error::Result;
use crate::project::Manifest;

/// How many of an attribute's values are listed: those most records have.
pub const COMMON_VALUES: usize = 3;

/// The most characters of a value that are given. A value is often a long
/// text, and what it begins with says enough to recognise it.
const VALUE_CHARACTERS: i64 = 200;

/// Which records to describe. `None` is every entity type, or every source.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AttributeFilter {
    pub entity_type: Option<EntityType>,
    pub source: Option<String>,
}

/// The attributes of the records one source gave of one entity type.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AttributeGroup {
    pub entity_type: EntityType,
    pub source: String,
    /// The records that stand: the latest version of each row or mention,
    /// and not one its source took back.
    pub records: u64,
    /// The columns the source declares, in its order and without its
    /// evidence column, and then any other attribute a record has, by name.
    pub attributes: Vec<AttributeSummary>,
}

/// One attribute of a group's records.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AttributeSummary {
    pub name: String,
    /// Whether it is the column that holds each row's key in its source.
    pub key: bool,
    /// The records that have a value for it that is not empty text.
    pub filled: u64,
    /// How many different values those records have.
    pub distinct: u64,
    /// The values most records have, the most common first, and of values
    /// as common the first in text order. When every value is held by one
    /// record, these are examples. Each is cut at 200 characters.
    pub common: Vec<ValueCount>,
    /// The features the entity type computes from it, for records of this
    /// source.
    pub features: Vec<DerivedFeature>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ValueCount {
    pub value: String,
    pub records: u64,
}

/// A feature, and how the source's columns become it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DerivedFeature {
    pub name: String,
    /// Every column it is computed from, this one among them.
    pub from: Vec<String>,
    /// The transform, or the function, the columns go through.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform: Option<String>,
}

/// The records that stand, and each of their attributes as text. A JSON
/// `null` is no value, and a value that is not text is its JSON.
const CELLS: &str = "
    WITH standing AS (
      SELECT o.entity_type, o.source_id, o.payload
      FROM spool.record o
      WHERE ($1::VARCHAR IS NULL OR o.entity_type = $1::VARCHAR)
        AND ($2::VARCHAR IS NULL OR o.source_id = $2::VARCHAR)
        AND NOT EXISTS (
          SELECT 1 FROM spool.record later WHERE later.supersedes = o.record_id)
        AND NOT EXISTS (
          SELECT 1 FROM spool.retraction taken
          WHERE taken.record_id = o.record_id AND taken.restored_run IS NULL)
    ),
    cells AS (
      SELECT s.entity_type, s.source_id, e.key AS attribute,
             CASE e.type
               WHEN 'VARCHAR' THEN e.value ->> '$'
               WHEN 'NULL' THEN NULL
               ELSE e.value::VARCHAR
             END AS value
      FROM standing s, json_each(s.payload) e
    )";

type Group = (String, String);

pub(crate) fn attributes(
    conn: &Connection,
    manifest: Option<&Manifest>,
    filter: &AttributeFilter,
) -> Result<Vec<AttributeGroup>> {
    let entity_type = filter.entity_type.as_ref().map(EntityType::as_str);
    let source = filter.source.as_deref();

    let mut records: BTreeMap<Group, u64> = BTreeMap::new();
    // Every pair the project maps is listed, with no records or with some.
    for declared in manifest.iter().flat_map(|manifest| &manifest.entity_types) {
        for mapped in &declared.sources {
            let wanted = entity_type.is_none_or(|name| name == declared.name.as_str())
                && source.is_none_or(|name| name == mapped);
            if wanted {
                records.insert((declared.name.to_string(), mapped.clone()), 0);
            }
        }
    }
    {
        let mut statement = conn.prepare(&format!(
            "{CELLS}
             SELECT entity_type, source_id, count(*) FROM standing GROUP BY ALL"
        ))?;
        let mut rows = statement.query(params![entity_type, source])?;
        while let Some(row) = rows.next()? {
            records.insert((row.get(0)?, row.get(1)?), row.get::<_, i64>(2)? as u64);
        }
    }

    let mut found: BTreeMap<Group, BTreeMap<String, (u64, u64)>> = BTreeMap::new();
    {
        let mut statement = conn.prepare(&format!(
            "{CELLS}
             SELECT entity_type, source_id, attribute,
                    count(nullif(value, '')), count(DISTINCT nullif(value, ''))
             FROM cells GROUP BY ALL"
        ))?;
        let mut rows = statement.query(params![entity_type, source])?;
        while let Some(row) = rows.next()? {
            found.entry((row.get(0)?, row.get(1)?)).or_default().insert(
                row.get(2)?,
                (row.get::<_, i64>(3)? as u64, row.get::<_, i64>(4)? as u64),
            );
        }
    }

    let mut common: BTreeMap<(Group, String), Vec<ValueCount>> = BTreeMap::new();
    {
        let mut statement = conn.prepare(&format!(
            "{CELLS}
             SELECT entity_type, source_id, attribute, left(value, {VALUE_CHARACTERS}), records
             FROM (
               SELECT entity_type, source_id, attribute, value, count(*) AS records,
                      row_number() OVER (
                        PARTITION BY entity_type, source_id, attribute
                        ORDER BY count(*) DESC, value) AS nth
               FROM cells
               WHERE value <> ''
               GROUP BY entity_type, source_id, attribute, value)
             WHERE nth <= {COMMON_VALUES}
             ORDER BY entity_type, source_id, attribute, nth"
        ))?;
        let mut rows = statement.query(params![entity_type, source])?;
        while let Some(row) = rows.next()? {
            common
                .entry(((row.get(0)?, row.get(1)?), row.get(2)?))
                .or_default()
                .push(ValueCount {
                    value: row.get(3)?,
                    records: row.get::<_, i64>(4)? as u64,
                });
        }
    }

    let mut groups = Vec::new();
    for (group, records) in records {
        let declared_source = manifest.and_then(|manifest| manifest.source(&group.1));
        let declared_type = manifest.and_then(|manifest| manifest.entity_type(&group.0));
        // The features in the order the type declares them.
        let mapped: Vec<(&String, &crate::project::Mapped)> = declared_type
            .and_then(|declared| {
                let mapping = declared.mapping.get(&group.1)?;
                Some(
                    declared
                        .declaration
                        .columns
                        .iter()
                        .filter_map(|column| mapping.get_key_value(&column.name))
                        .collect(),
                )
            })
            .unwrap_or_default();
        let mut held = found.remove(&group).unwrap_or_default();
        // The evidence column is a record's evidence, never an attribute.
        let mut names: Vec<String> = declared_source
            .map(|declared| {
                declared
                    .columns
                    .iter()
                    .filter(|column| declared.evidence.as_ref() != Some(*column))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let others: Vec<String> = held
            .keys()
            .filter(|name| !names.contains(name))
            .cloned()
            .collect();
        names.extend(others);
        let attributes = names
            .into_iter()
            .map(|name| {
                let (filled, distinct) = held.remove(&name).unwrap_or((0, 0));
                AttributeSummary {
                    key: declared_source
                        .is_some_and(|declared| declared.key.as_ref() == Some(&name)),
                    filled,
                    distinct,
                    common: common
                        .remove(&(group.clone(), name.clone()))
                        .unwrap_or_default(),
                    features: mapped
                        .iter()
                        .filter(|(_, mapped)| mapped.from.contains(&name))
                        .map(|(feature, mapped)| DerivedFeature {
                            name: feature.to_string(),
                            from: mapped.from.clone(),
                            transform: mapped.transform.clone(),
                        })
                        .collect(),
                    name,
                }
            })
            .collect();
        groups.push(AttributeGroup {
            entity_type: EntityType::new(group.0).map_err(StateError::new)?,
            source: group.1,
            records,
            attributes,
        });
    }
    Ok(groups)
}
