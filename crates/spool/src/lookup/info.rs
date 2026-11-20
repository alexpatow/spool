//! What a store holds overall: its entity types, their sources and how
//! much there is of each.

use duckdb::Connection;
use serde::{Deserialize, Serialize};
use spool_core::{ChangesetId, EntityType, Seq};

use super::entity_type;
use crate::error::Result;
use crate::project::{Extractor, FeatureColumn, Manifest, Strategy};
use crate::schema;
use crate::store::parse_stored;
use crate::time::format_rfc3339;

/// What a store holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct StoreInfo {
    pub engine_version: String,
    pub format_version: String,
    /// The hash of the manifest the store was last applied with.
    pub manifest_hash: Option<String>,
    /// The position of the last committed changeset.
    pub head: Seq,
    pub entity_types: Vec<TypeInfo>,
    /// The sources the project declares, by name: what rows can be sent
    /// as. Empty when the engine runs without a manifest.
    #[serde(default)]
    pub sources: Vec<DeclaredSource>,
    /// The project's extractors. Empty when the engine runs without a
    /// manifest.
    #[serde(default)]
    pub extractors: Vec<Extractor>,
    /// Documents held, counting each version.
    #[serde(default)]
    pub documents: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TypeInfo {
    pub name: EntityType,
    /// How many times the declaration has been applied in a new form.
    pub declaration_version: u32,
    /// The columns of the feature table, as the manifest declares them.
    /// Empty when the engine runs without a manifest.
    pub features: Vec<FeatureColumn>,
    /// The project's user-code strategies for the type. The built-in
    /// `rules` is always there and is not listed.
    #[serde(default)]
    pub strategies: Vec<Strategy>,
    pub records: u64,
    /// Records that wait for an identity: no changeset has placed them,
    /// and their source has not taken them back.
    pub unassigned: u64,
    /// Records their source took back. They are counted in `records`.
    #[serde(default)]
    pub retracted: u64,
    /// The sources the records came from, by name.
    pub sources: Vec<SourceInfo>,
    /// Identities that hold a record. One whose records were all taken
    /// back is not counted: it is kept for them to come back to.
    pub identities: u64,
    pub open_review_items: u64,
    /// The draft that waits for a decision, if there is one.
    pub open_draft: Option<ChangesetId>,
}

/// A source as the project declares it, whether or not it has sent rows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DeclaredSource {
    pub name: String,
    /// The column that holds each row's key within the source, if the
    /// source names one.
    pub key: Option<String>,
    /// The column of JSON text that says where each row was found, if the
    /// source names one. It is a record's evidence, not part of its payload.
    #[serde(default)]
    pub evidence: Option<String>,
    /// The columns a row of it may have. A row with any other is refused.
    pub columns: Vec<String>,
    /// The entity types whose mapping reads the source, by name. Its rows
    /// can be sent as records of these types and of no other.
    pub entity_types: Vec<EntityType>,
}

/// What one source gave of an entity type.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SourceInfo {
    pub source: String,
    /// Counting each version of a row.
    pub records: u64,
    /// Those of them that wait for an identity.
    pub unassigned: u64,
    /// Those of them the source took back.
    #[serde(default)]
    pub retracted: u64,
    /// When the latest of them was loaded.
    pub last_loaded_at: String,
}

/// The sources of a manifest, each with the entity types that read it.
fn declared_sources(manifest: &Manifest) -> Vec<DeclaredSource> {
    let mut sources: Vec<DeclaredSource> = manifest
        .sources
        .iter()
        .map(|source| DeclaredSource {
            name: source.name.clone(),
            key: source.key.clone(),
            evidence: source.evidence.clone(),
            columns: source.columns.clone(),
            entity_types: manifest
                .entity_types
                .iter()
                .filter(|entry| entry.sources.contains(&source.name))
                .map(|entry| entry.name.clone())
                .collect(),
        })
        .collect();
    sources.sort_by(|a, b| a.name.cmp(&b.name));
    sources
}

pub(crate) fn info(conn: &Connection, manifest: Option<&Manifest>) -> Result<StoreInfo> {
    let mut entity_types = Vec::new();
    let mut statement = conn.prepare(
        "SELECT t.entity_type, max(t.version),
                (SELECT count(*) FROM spool.record o WHERE o.entity_type = t.entity_type),
                (SELECT count(*) FROM spool.record o
                 WHERE o.entity_type = t.entity_type AND NOT EXISTS (
                   SELECT 1 FROM spool.membership m
                   WHERE m.record_id = o.record_id AND m.to_seq IS NULL)
                   AND o.record_id NOT IN (
                     SELECT record_id FROM spool.retraction WHERE restored_run IS NULL)),
                (SELECT count(*) FROM spool.identity i
                 WHERE i.entity_type = t.entity_type AND i.merged_into IS NULL
                   AND EXISTS (
                     SELECT 1 FROM spool.membership m
                     WHERE m.identity_id = i.identity_id AND m.to_seq IS NULL)),
                (SELECT count(*) FROM spool.review_item r
                 WHERE r.entity_type = t.entity_type AND r.status = 'open'),
                (SELECT max(d.changeset_id::VARCHAR) FROM spool.draft d
                 WHERE d.entity_type = t.entity_type AND d.status = 'open'),
                (SELECT count(*) FROM spool.record o
                 JOIN spool.retraction taken
                   ON taken.record_id = o.record_id AND taken.restored_run IS NULL
                 WHERE o.entity_type = t.entity_type)
         FROM spool.entity_type t
         GROUP BY t.entity_type
         ORDER BY t.entity_type",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let name = entity_type(row.get(0)?)?;
        entity_types.push(TypeInfo {
            declaration_version: row.get::<_, i32>(1)? as u32,
            features: manifest
                .and_then(|manifest| manifest.entity_type(name.as_str()))
                .map(|entry| entry.declaration.columns.clone())
                .unwrap_or_default(),
            strategies: manifest
                .and_then(|manifest| manifest.entity_type(name.as_str()))
                .map(|entry| entry.strategies.clone())
                .unwrap_or_default(),
            records: row.get::<_, i64>(2)? as u64,
            unassigned: row.get::<_, i64>(3)? as u64,
            retracted: row.get::<_, i64>(7)? as u64,
            sources: Vec::new(),
            identities: row.get::<_, i64>(4)? as u64,
            open_review_items: row.get::<_, i64>(5)? as u64,
            open_draft: row
                .get::<_, Option<String>>(6)?
                .as_deref()
                .map(parse_stored)
                .transpose()?,
            name,
        });
    }
    let mut statement = conn.prepare(
        "SELECT o.entity_type, o.source_id, count(*),
                count(*) FILTER (WHERE m.record_id IS NULL AND taken.record_id IS NULL),
                epoch_us(max(o.loaded_at)), count(taken.record_id)
         FROM spool.record o
         LEFT JOIN spool.membership m ON m.record_id = o.record_id AND m.to_seq IS NULL
         LEFT JOIN spool.retraction taken
           ON taken.record_id = o.record_id AND taken.restored_run IS NULL
         GROUP BY o.entity_type, o.source_id
         ORDER BY o.entity_type, o.source_id",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(0)?;
        if let Some(found) = entity_types
            .iter_mut()
            .find(|found| found.name.as_str() == name)
        {
            found.sources.push(SourceInfo {
                source: row.get(1)?,
                records: row.get::<_, i64>(2)? as u64,
                unassigned: row.get::<_, i64>(3)? as u64,
                retracted: row.get::<_, i64>(5)? as u64,
                last_loaded_at: format_rfc3339(row.get(4)?),
            });
        }
    }
    Ok(StoreInfo {
        engine_version: env!("CARGO_PKG_VERSION").to_owned(),
        format_version: schema::FORMAT_VERSION.to_owned(),
        manifest_hash: schema::read_meta(conn, "manifest_hash")?,
        head: crate::store::read_head(conn)?,
        entity_types,
        sources: manifest.map(declared_sources).unwrap_or_default(),
        extractors: manifest
            .map(|manifest| manifest.extractors.clone())
            .unwrap_or_default(),
        documents: crate::documents::count(conn)?,
    })
}
