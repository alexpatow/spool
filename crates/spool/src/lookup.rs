//! Reads of single things: an observation, an identity and its members, the
//! changesets that touched an identity, and what the store holds overall.
//!
//! Everything here reads committed state from a connection of its own, so a
//! lookup does not wait for a run that is in progress.

use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};
use spool_core::{
    Actor, ChangesetId, DocumentId, EntityType, IdentityId, ObservationId, Op, RunId, Seq,
    StateError,
};

use crate::error::Result;
use crate::project::{Extractor, FeatureColumn, Manifest, Strategy};
use crate::schema;
use crate::store::{OpRow, op_from_row, parse};
use crate::time::rfc3339;

/// An observation as it was ingested, and the identity it belongs to now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ObservationRecord {
    pub id: ObservationId,
    pub entity_type: EntityType,
    pub source: String,
    /// The row's key within its source, if it had one.
    pub key: Option<String>,
    /// The attributes exactly as received.
    pub payload: serde_json::Value,
    /// Where in the source the observation was found, if the source said.
    pub evidence: Option<serde_json::Value>,
    pub ingested_at: String,
    pub ingest_run: RunId,
    /// `None` while no changeset has placed it.
    pub identity: Option<IdentityId>,
    /// The version of the row this one replaced: what the source held under
    /// the same key before.
    #[serde(default)]
    pub supersedes: Option<ObservationId>,
    /// The version that replaced this one, if the source has sent the row
    /// again with other content.
    #[serde(default)]
    pub superseded_by: Option<ObservationId>,
}

impl ObservationRecord {
    /// What to call the observation in a sentence: `source:key`, which is
    /// how the people who sent it know it. A mention is
    /// `source:document[start-end]` with the document's own key, since the
    /// key of a mention names its document by ID. An observation without a
    /// key is its ID.
    pub fn name(&self) -> String {
        let mention = self.evidence.as_ref().and_then(|evidence| {
            evidence.get("document")?;
            Some(format!(
                "{}:{}[{}-{}]",
                self.source,
                evidence.get("key")?.as_str()?,
                evidence.get("start")?.as_u64()?,
                evidence.get("end")?.as_u64()?
            ))
        });
        match (mention, &self.key) {
            (Some(mention), _) => mention,
            (None, Some(key)) => format!("{}:{key}", self.source),
            (None, None) => self.id.to_string(),
        }
    }
}

/// Which observations to list. Results are ordered by ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservationFilter {
    pub entity_type: Option<EntityType>,
    pub source: Option<String>,
    pub key: Option<String>,
    /// Only the mentions an extractor found in this document.
    pub document: Option<DocumentId>,
    /// Only those without an identity.
    pub unassigned: bool,
    /// Only those after this ID: the last one of the page before.
    pub after: Option<ObservationId>,
    pub limit: u64,
}

impl Default for ObservationFilter {
    fn default() -> Self {
        Self {
            entity_type: None,
            source: None,
            key: None,
            document: None,
            unassigned: false,
            after: None,
            limit: 100,
        }
    }
}

impl ObservationFilter {
    /// The observations a source holds under one key. There is more than one
    /// when the row changed between ingests.
    pub fn keyed(source: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            source: Some(source.into()),
            key: Some(key.into()),
            ..Self::default()
        }
    }
}

/// An identity and the observations that belong to it now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct IdentityView {
    /// The ID that was asked for.
    pub id: IdentityId,
    /// The active identity it stands for: itself, or the one it was merged
    /// into.
    pub current: IdentityId,
    pub entity_type: EntityType,
    /// The log position that created the identity that was asked for.
    pub created: Seq,
    /// The members of the current identity, ordered by observation ID.
    pub members: Vec<Member>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Member {
    pub observation: ObservationId,
    pub source: String,
    pub key: Option<String>,
    /// The log position that placed it here.
    pub since: Seq,
}

/// An active identity in a list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct IdentitySummary {
    pub id: IdentityId,
    pub entity_type: EntityType,
    pub created: Seq,
    pub members: u64,
}

/// Which active identities to list. Results are ordered by ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdentityFilter {
    pub entity_type: Option<EntityType>,
    /// Only those after this ID: the last one of the page before.
    pub after: Option<IdentityId>,
    pub limit: u64,
}

impl Default for IdentityFilter {
    fn default() -> Self {
        Self {
            entity_type: None,
            after: None,
            limit: 100,
        }
    }
}

/// A committed changeset as far as it concerns one identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct HistoryEntry {
    pub seq: Seq,
    pub changeset: ChangesetId,
    pub actor: Actor,
    pub reason: Option<String>,
    /// The reconcile run that drafted it, if one did.
    pub run: Option<RunId>,
    pub committed_at: String,
    /// The operations that name the identity, in the order they were applied.
    pub ops: Vec<Op>,
}

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
    pub observations: u64,
    /// Observations no changeset has placed.
    pub unassigned: u64,
    pub identities: u64,
    pub open_review_items: u64,
    /// The draft that waits for a decision, if there is one.
    pub open_draft: Option<ChangesetId>,
}

fn entity_type(name: String) -> Result<EntityType> {
    Ok(EntityType::new(name).map_err(StateError::new)?)
}

fn json(text: String) -> Result<serde_json::Value> {
    Ok(serde_json::from_str(&text).map_err(StateError::new)?)
}

const OBSERVATION: &str = "
    SELECT o.observation_id::VARCHAR, o.entity_type, o.source_id, o.source_key,
           o.payload::VARCHAR, o.evidence::VARCHAR, epoch_us(o.ingested_at), o.ingest_run,
           m.identity_id::VARCHAR, o.supersedes::VARCHAR, later.observation_id::VARCHAR
    FROM spool.observation o
    LEFT JOIN spool.membership m ON m.observation_id = o.observation_id AND m.to_seq IS NULL
    LEFT JOIN spool.observation later ON later.supersedes = o.observation_id";

fn observation_record(row: &duckdb::Row<'_>) -> Result<ObservationRecord> {
    fn id<T>(row: &duckdb::Row<'_>, at: usize) -> Result<Option<T>>
    where
        T: std::str::FromStr,
        T::Err: std::error::Error + Send + Sync + 'static,
    {
        row.get::<_, Option<String>>(at)?
            .as_deref()
            .map(parse)
            .transpose()
    }
    Ok(ObservationRecord {
        id: parse(&row.get::<_, String>(0)?)?,
        entity_type: entity_type(row.get(1)?)?,
        source: row.get(2)?,
        key: row.get(3)?,
        payload: json(row.get(4)?)?,
        evidence: row.get::<_, Option<String>>(5)?.map(json).transpose()?,
        ingested_at: rfc3339(row.get(6)?),
        ingest_run: RunId(row.get::<_, i64>(7)? as u64),
        identity: id(row, 8)?,
        supersedes: id(row, 9)?,
        superseded_by: id(row, 10)?,
    })
}

pub(crate) fn observation(
    conn: &Connection,
    id: ObservationId,
) -> Result<Option<ObservationRecord>> {
    let mut statement = conn.prepare(&format!("{OBSERVATION} WHERE o.observation_id = ?::UUID"))?;
    let mut rows = statement.query([id.to_string()])?;
    match rows.next()? {
        Some(row) => Ok(Some(observation_record(row)?)),
        None => Ok(None),
    }
}

pub(crate) fn observations(
    conn: &Connection,
    filter: &ObservationFilter,
) -> Result<Vec<ObservationRecord>> {
    let mut statement = conn.prepare(&format!(
        "{OBSERVATION}
         WHERE ($1::VARCHAR IS NULL OR o.entity_type = $1::VARCHAR)
           AND ($2::VARCHAR IS NULL OR o.source_id = $2::VARCHAR)
           AND ($3::VARCHAR IS NULL OR o.source_key = $3::VARCHAR)
           AND ($4::VARCHAR IS NULL OR o.observation_id > $4::UUID)
           AND (NOT $5::BOOLEAN OR m.identity_id IS NULL)
           AND ($7::VARCHAR IS NULL
                OR json_extract_string(o.evidence, '$.document') = $7::VARCHAR)
         ORDER BY o.observation_id
         LIMIT $6"
    ))?;
    let mut rows = statement.query(params![
        filter.entity_type.as_ref().map(EntityType::as_str),
        filter.source,
        filter.key,
        filter.after.map(|id| id.to_string()),
        filter.unassigned,
        limit(filter.limit),
        filter.document.map(|id| id.to_string()),
    ])?;
    let mut found = Vec::new();
    while let Some(row) = rows.next()? {
        found.push(observation_record(row)?);
    }
    Ok(found)
}

fn limit(limit: u64) -> i64 {
    limit.min(i64::MAX as u64) as i64
}

pub(crate) fn identity(conn: &Connection, id: IdentityId) -> Result<Option<IdentityView>> {
    let found: Option<(String, i64, Option<String>)> = {
        let mut statement = conn.prepare(
            "SELECT entity_type, created_seq, merged_into::VARCHAR
             FROM spool.identity WHERE identity_id = ?::UUID",
        )?;
        let mut rows = statement.query([id.to_string()])?;
        match rows.next()? {
            Some(row) => Some((row.get(0)?, row.get(1)?, row.get(2)?)),
            None => None,
        }
    };
    let Some((entity_type_name, created, merged_into)) = found else {
        return Ok(None);
    };
    let current = match merged_into {
        Some(survivor) => parse(&survivor)?,
        None => id,
    };

    let mut members = Vec::new();
    let mut statement = conn.prepare(
        "SELECT m.observation_id::VARCHAR, o.source_id, o.source_key, m.from_seq
         FROM spool.membership m
         JOIN spool.observation o ON o.observation_id = m.observation_id
         WHERE m.to_seq IS NULL AND m.identity_id = ?::UUID
         ORDER BY m.observation_id",
    )?;
    let mut rows = statement.query([current.to_string()])?;
    while let Some(row) = rows.next()? {
        members.push(Member {
            observation: parse(&row.get::<_, String>(0)?)?,
            source: row.get(1)?,
            key: row.get(2)?,
            since: Seq(row.get::<_, i64>(3)? as u64),
        });
    }
    Ok(Some(IdentityView {
        id,
        current,
        entity_type: entity_type(entity_type_name)?,
        created: Seq(created as u64),
        members,
    }))
}

pub(crate) fn identities(
    conn: &Connection,
    filter: &IdentityFilter,
) -> Result<Vec<IdentitySummary>> {
    let mut statement = conn.prepare(
        "SELECT i.identity_id::VARCHAR, i.entity_type, i.created_seq,
                (SELECT count(*) FROM spool.membership m
                 WHERE m.identity_id = i.identity_id AND m.to_seq IS NULL)
         FROM spool.identity i
         WHERE i.merged_into IS NULL
           AND ($1::VARCHAR IS NULL OR i.entity_type = $1::VARCHAR)
           AND ($2::VARCHAR IS NULL OR i.identity_id > $2::UUID)
         ORDER BY i.identity_id
         LIMIT $3",
    )?;
    let mut rows = statement.query(params![
        filter.entity_type.as_ref().map(EntityType::as_str),
        filter.after.map(|id| id.to_string()),
        limit(filter.limit),
    ])?;
    let mut found = Vec::new();
    while let Some(row) = rows.next()? {
        found.push(IdentitySummary {
            id: parse(&row.get::<_, String>(0)?)?,
            entity_type: entity_type(row.get(1)?)?,
            created: Seq(row.get::<_, i64>(2)? as u64),
            members: row.get::<_, i64>(3)? as u64,
        });
    }
    Ok(found)
}

/// The changesets that name an identity, oldest first, each with only the
/// operations that name it.
pub(crate) fn history(conn: &Connection, id: IdentityId) -> Result<Vec<HistoryEntry>> {
    let mut entries: Vec<HistoryEntry> = Vec::new();
    let mut statement = conn.prepare(
        "SELECT c.seq, c.changeset_id::VARCHAR, c.actor, c.reason, c.run_id,
                epoch_us(c.committed_at),
                o.kind, o.identity_id::VARCHAR, o.from_identity_id::VARCHAR,
                o.observation_id::VARCHAR, o.other_observation_id::VARCHAR,
                o.entity_type, o.constraint_kind
         FROM spool.changeset_op o
         JOIN spool.changeset c ON c.seq = o.seq
         WHERE o.identity_id = $1::UUID OR o.from_identity_id = $1::UUID
         ORDER BY o.seq, o.op_index",
    )?;
    let mut rows = statement.query([id.to_string()])?;
    while let Some(row) = rows.next()? {
        let seq = Seq(row.get::<_, i64>(0)? as u64);
        let op = op_from_row(OpRow {
            kind: row.get(6)?,
            identity: row.get(7)?,
            from_identity: row.get(8)?,
            observation: row.get(9)?,
            other_observation: row.get(10)?,
            entity_type: row.get(11)?,
            constraint_kind: row.get(12)?,
        })?;
        match entries.last_mut() {
            Some(entry) if entry.seq == seq => entry.ops.push(op),
            _ => entries.push(HistoryEntry {
                seq,
                changeset: parse(&row.get::<_, String>(1)?)?,
                actor: parse(&row.get::<_, String>(2)?)?,
                reason: row.get(3)?,
                run: row.get::<_, Option<i64>>(4)?.map(|run| RunId(run as u64)),
                committed_at: rfc3339(row.get(5)?),
                ops: vec![op],
            }),
        }
    }
    Ok(entries)
}

pub(crate) fn info(conn: &Connection, manifest: Option<&Manifest>) -> Result<StoreInfo> {
    let mut entity_types = Vec::new();
    let mut statement = conn.prepare(
        "SELECT t.entity_type, max(t.version),
                (SELECT count(*) FROM spool.observation o WHERE o.entity_type = t.entity_type),
                (SELECT count(*) FROM spool.observation o
                 WHERE o.entity_type = t.entity_type AND NOT EXISTS (
                   SELECT 1 FROM spool.membership m
                   WHERE m.observation_id = o.observation_id AND m.to_seq IS NULL)),
                (SELECT count(*) FROM spool.identity i
                 WHERE i.entity_type = t.entity_type AND i.merged_into IS NULL),
                (SELECT count(*) FROM spool.review_item r
                 WHERE r.entity_type = t.entity_type AND r.status = 'open'),
                (SELECT max(d.changeset_id::VARCHAR) FROM spool.draft d
                 WHERE d.entity_type = t.entity_type AND d.status = 'open')
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
            observations: row.get::<_, i64>(2)? as u64,
            unassigned: row.get::<_, i64>(3)? as u64,
            identities: row.get::<_, i64>(4)? as u64,
            open_review_items: row.get::<_, i64>(5)? as u64,
            open_draft: row
                .get::<_, Option<String>>(6)?
                .as_deref()
                .map(parse)
                .transpose()?,
            name,
        });
    }
    Ok(StoreInfo {
        engine_version: env!("CARGO_PKG_VERSION").to_owned(),
        format_version: schema::FORMAT_VERSION.to_owned(),
        manifest_hash: schema::meta(conn, "manifest_hash")?,
        head: crate::store::read_head(conn)?,
        entity_types,
        extractors: manifest
            .map(|manifest| manifest.extractors.clone())
            .unwrap_or_default(),
        documents: crate::documents::count(conn)?,
    })
}
