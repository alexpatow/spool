//! Identities: one identity now or as of a changeset, the lists of them
//! with what each is called, and what a text search matched in each.

use std::collections::{BTreeMap, BTreeSet};

use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};
use spool_core::{EntityType, IdentityId, RecordId, Seq, StateError};

use super::records::{RECORD_SEARCH_TEXT, contains_words};
use super::{clamp_limit, encode_search_words, entity_type, uuid_list};
use crate::error::{Error, Result};
use crate::features;
use crate::order::{Key, Keyed, Sort};
use crate::project::Declaration;
use crate::store::{parse_stored, read_head};

// Named by a doc comment only.
#[cfg(doc)]
use super::records::RecordFilter;

/// The label most of the given records have. Of labels that as many have,
/// the one that comes first: callers give the labels oldest first, so
/// that a label does not change because a row arrived that says the same
/// in other words.
fn common_label(labels: impl Iterator<Item = String>) -> Option<String> {
    let mut counted: Vec<(String, usize)> = Vec::new();
    for label in labels {
        match counted.iter_mut().find(|(known, _)| *known == label) {
            Some((_, count)) => *count += 1,
            None => counted.push((label, 1)),
        }
    }
    let most = counted.iter().map(|(_, count)| *count).max()?;
    counted
        .into_iter()
        .find(|(_, count)| *count == most)
        .map(|(label, _)| label)
}

/// The most members an identity's label is judged from: its oldest.
const LABEL_MEMBERS: u64 = 100;

/// An identity and the records that belong to it now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct IdentityView {
    /// The ID that was asked for.
    pub id: IdentityId,
    /// The active identity it stands for: itself, or the one it was merged
    /// into.
    pub current: IdentityId,
    pub entity_type: EntityType,
    /// What a person would call the current identity: the label most of its
    /// members have. `None` when no member's payload names it.
    #[serde(default)]
    pub label: Option<String>,
    /// The log position that created the identity that was asked for.
    pub created: Seq,
    /// The members of the current identity, ordered by record ID.
    pub members: Vec<Member>,
}

/// An identity as it stood immediately after the given changeset.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct IdentitySnapshot {
    #[serde(flatten)]
    pub identity: IdentityView,
    pub as_of_seq: Seq,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Member {
    pub record: RecordId,
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
    /// What a person would call it: the label most of its members have. Of
    /// labels that as many have, the one that was loaded first. `None`
    /// when no member's payload names it. An identity with very many
    /// members is judged by the hundred that were loaded first.
    #[serde(default)]
    pub label: Option<String>,
    pub created: Seq,
    pub members: u64,
    /// Different labels its members have, whatever their case. Members
    /// without a label are not counted. Many names in one identity is the
    /// first sign that different things were joined.
    #[serde(default)]
    pub names: u64,
    /// Pairs of its members that a link joins directly: a `match` of a
    /// run that finished, a `must_link` constraint, or one being a later
    /// version of the other.
    #[serde(default)]
    pub linked_pairs: u64,
    /// Pairs of its members that no link joins directly, out of
    /// `members × (members - 1) / 2`: those together only through a chain
    /// of others, and those that nothing holds together any more, which
    /// `spool explain <identity>` tells apart. An identity mostly held by
    /// chains is where rules that each hold joined things that are not one.
    #[serde(default)]
    pub unlinked_pairs: u64,
    /// The sources its members came from, the one with most members first.
    pub sources: Vec<SourceCount>,
    /// The strongest match for a text search.
    #[serde(default, rename = "match", skip_serializing_if = "Option::is_none")]
    pub search_match: Option<IdentityMatch>,
}

/// Why an identity appeared in a text search.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IdentityMatch {
    Label {
        value: String,
    },
    /// A member's label that differs from the identity's label.
    Alias {
        value: String,
        record: RecordId,
    },
    Record {
        record: RecordId,
        /// A payload field holding all the words, when there is one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        field: Option<String>,
    },
}

/// How many records a source gave.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SourceCount {
    pub source: String,
    pub records: u64,
}

/// Which active identities to list. Results are ordered by search relevance
/// with `q` and by ID otherwise, unless `sort` is given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdentityFilter {
    pub entity_type: Option<EntityType>,
    /// Only those with a member whose label or record holds every word
    /// of this text, as [`RecordFilter::q`] finds records.
    pub q: Option<String>,
    /// Only those among these IDs. One that names no active identity finds
    /// nothing: an identity that was merged away is not in the list.
    pub ids: Option<Vec<IdentityId>>,
    /// Only those that come after this one in the list: the last one of
    /// the page before.
    pub after: Option<IdentityId>,
    pub limit: u64,
    pub sort: Option<Sort<IdentitySort>>,
}

/// What a list of identities can be sorted by. Each ends with the ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentitySort {
    Id,
    /// The label, whatever its case. Those without one last.
    Label,
    EntityType,
    /// How many records it has.
    Records,
    /// How many sources its records came from.
    Sources,
    /// How many different labels its records have, whatever their case.
    Names,
    /// The log position that created it.
    Created,
}

impl IdentitySort {
    fn keys(self) -> Vec<Key> {
        let mut keys = match self {
            Self::Id => Vec::new(),
            Self::Label => Key::optional_text("il.label").into(),
            Self::EntityType => Key::text("i.entity_type").into(),
            Self::Records => vec![Key::of(
                "(SELECT count(*) FROM spool.membership m
                  WHERE m.identity_id = i.identity_id AND m.to_seq IS NULL)",
            )],
            Self::Sources => vec![Key::of(
                "(SELECT count(DISTINCT o.source_id) FROM spool.membership m
                  JOIN spool.record o ON o.record_id = m.record_id
                  WHERE m.identity_id = i.identity_id AND m.to_seq IS NULL)",
            )],
            Self::Names => vec![Key::of(
                "(SELECT count(DISTINCT lower(named.label)) FROM spool.membership m
                  JOIN spool.record_label named ON named.record_id = m.record_id
                  WHERE m.identity_id = i.identity_id AND m.to_seq IS NULL)",
            )],
            Self::Created => vec![Key::of("i.created_seq")],
        };
        keys.push(Key::of("i.identity_id"));
        keys
    }
}

impl Default for IdentityFilter {
    fn default() -> Self {
        Self {
            entity_type: None,
            q: None,
            ids: None,
            after: None,
            limit: 100,
            sort: None,
        }
    }
}

/// What the label of each identity is, as [`read_identity_labels`] finds it, as a common
/// table expression `identity_label` of `identity_id` and `label`. Only
/// an identity that has one is in it.
pub(super) fn identity_label(filter_members: &str) -> String {
    format!(
        "identity_label AS (
    SELECT identity_id, label FROM (
      SELECT identity_id, label,
             row_number() OVER (PARTITION BY identity_id ORDER BY count(*) DESC, min(nth)) AS rank
      FROM (
        SELECT m.identity_id, named.label,
               row_number() OVER (
                 PARTITION BY m.identity_id
                 ORDER BY o.loaded_at, o.load_run, o.record_id) AS nth
        FROM spool.membership m
        JOIN spool.record o ON o.record_id = m.record_id
        LEFT JOIN spool.record_label named ON named.record_id = o.record_id
        WHERE m.to_seq IS NULL {filter_members})
      WHERE nth <= {LABEL_MEMBERS} AND label IS NOT NULL
      GROUP BY identity_id, label)
    WHERE rank = 1)"
    )
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
        Some(survivor) => parse_stored(&survivor)?,
        None => id,
    };
    Ok(Some(IdentityView {
        id,
        current,
        entity_type: entity_type(entity_type_name)?,
        label: read_identity_labels(conn, &[current])?.remove(&current),
        created: Seq(created as u64),
        members: identity_members(conn, current, None)?,
    }))
}

pub(crate) fn identity_at(conn: &Connection, id: IdentityId, seq: Seq) -> Result<IdentitySnapshot> {
    let head = read_head(conn)?;
    if seq > head {
        return Err(Error::FutureIdentitySnapshot { seq, head });
    }
    let (entity_type_name, created) = {
        let mut statement = conn.prepare(
            "SELECT entity_type, created_seq FROM spool.identity WHERE identity_id = ?::UUID",
        )?;
        let mut rows = statement.query([id.to_string()])?;
        let row = rows.next()?.ok_or(Error::UnknownIdentity(id))?;
        (row.get::<_, String>(0)?, Seq(row.get::<_, i64>(1)? as u64))
    };
    if created > seq {
        return Err(Error::IdentityNotYetCreated { id, seq, created });
    }

    let current = identity_current_at(conn, id, seq)?;
    let members = identity_members(conn, current, Some(seq))?;
    let entity_type = entity_type(entity_type_name)?;
    let label = if members.is_empty() {
        None
    } else {
        label_at(conn, &entity_type, current, seq)?
    };
    Ok(IdentitySnapshot {
        identity: IdentityView {
            id,
            current,
            entity_type,
            label,
            created,
            members,
        },
        as_of_seq: seq,
    })
}

/// Follows the last direct merge of each identity as of `seq`. A reinstate
/// ends a direct merge. The current projection only holds today's survivor.
fn identity_current_at(conn: &Connection, id: IdentityId, seq: Seq) -> Result<IdentityId> {
    let mut statement = conn.prepare(
        "SELECT CASE WHEN kind = 'merge' THEN identity_id::VARCHAR END
         FROM spool.changeset_op
         WHERE seq <= $2 AND (
             (kind = 'merge' AND from_identity_id = $1::UUID)
             OR (kind = 'reinstate' AND identity_id = $1::UUID))
         ORDER BY seq DESC, op_index DESC LIMIT 1",
    )?;
    let mut current = id;
    let mut seen = BTreeSet::new();
    while seen.insert(current) {
        let mut rows = statement.query(params![current.to_string(), seq.0 as i64])?;
        let Some(row) = rows.next()? else {
            return Ok(current);
        };
        let Some(survivor) = row.get::<_, Option<String>>(0)? else {
            return Ok(current);
        };
        current = parse_stored(&survivor)?;
    }
    Err(StateError::new(format!(
        "identity {id} has a merge cycle at changeset {seq}"
    ))
    .into())
}

/// An opening sequence is inclusive and a closing sequence is exclusive.
const MEMBER_AT: &str = "m.from_seq <= $2 AND (m.to_seq IS NULL OR m.to_seq > $2)";

fn identity_members(
    conn: &Connection,
    current: IdentityId,
    seq: Option<Seq>,
) -> Result<Vec<Member>> {
    let mut members = Vec::new();
    let visibility = if seq.is_some() {
        MEMBER_AT
    } else {
        "m.to_seq IS NULL"
    };
    let mut statement = conn.prepare(&format!(
        "SELECT m.record_id::VARCHAR, o.source_id, o.source_key, m.from_seq
         FROM spool.membership m
         JOIN spool.record o ON o.record_id = m.record_id
         WHERE m.identity_id = $1::UUID AND {visibility}
         ORDER BY m.record_id",
    ))?;
    let current = current.to_string();
    let mut rows = match seq {
        Some(seq) => statement.query(params![current, seq.0 as i64])?,
        None => statement.query([current])?,
    };
    while let Some(row) = rows.next()? {
        members.push(Member {
            record: parse_stored(&row.get::<_, String>(0)?)?,
            source: row.get(1)?,
            key: row.get(2)?,
            since: Seq(row.get::<_, i64>(3)? as u64),
        });
    }
    Ok(members)
}

fn label_at(
    conn: &Connection,
    entity_type: &EntityType,
    identity: IdentityId,
    seq: Seq,
) -> Result<Option<String>> {
    let declaration: Declaration = {
        let mut statement = conn.prepare(
            "SELECT d.declaration::VARCHAR FROM spool.entity_type d
             JOIN spool.changeset c ON c.seq = ? AND d.declared_at <= c.committed_at
             WHERE d.entity_type = ? ORDER BY d.version DESC LIMIT 1",
        )?;
        let mut rows = statement.query(params![seq.0 as i64, entity_type.as_str()])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        serde_json::from_str(&row.get::<_, String>(0)?).map_err(StateError::new)?
    };
    let Some(label) = declaration.label.as_deref() else {
        return Ok(None);
    };
    let mut statement = conn.prepare(&format!(
        "SELECT o.record_id::VARCHAR FROM spool.membership m
         JOIN spool.record o ON o.record_id = m.record_id
         WHERE m.identity_id = $1::UUID AND {MEMBER_AT}
         ORDER BY o.loaded_at, o.load_run, o.record_id LIMIT {LABEL_MEMBERS}",
    ))?;
    let mut rows = statement.query(params![identity.to_string(), seq.0 as i64])?;
    let mut oldest = Vec::new();
    while let Some(row) = rows.next()? {
        oldest.push(parse_stored(&row.get::<_, String>(0)?)?);
    }
    let names = features::snapshot_labels(conn, &declaration, label, &oldest)?;
    Ok(common_label(names.into_iter()))
}

/// What to call each of the identities: the label most of its members
/// have, judged from the members that were loaded first. An identity
/// whose members name nothing is left out.
fn read_identity_labels(
    conn: &Connection,
    ids: &[IdentityId],
) -> Result<BTreeMap<IdentityId, String>> {
    let mut labels: BTreeMap<IdentityId, Vec<String>> = BTreeMap::new();
    if ids.is_empty() {
        return Ok(BTreeMap::new());
    }
    let mut statement = conn.prepare(&format!(
        "SELECT identity_id::VARCHAR, label FROM (
           SELECT m.identity_id, named.label,
                  row_number() OVER (
                    PARTITION BY m.identity_id
                    ORDER BY o.loaded_at, o.load_run, o.record_id) AS nth
           FROM spool.membership m
           JOIN spool.record o ON o.record_id = m.record_id
           LEFT JOIN spool.record_label named ON named.record_id = o.record_id
           WHERE m.to_seq IS NULL AND list_contains({}, m.identity_id))
         WHERE nth <= {LABEL_MEMBERS} AND label IS NOT NULL
         ORDER BY identity_id, nth",
        uuid_list(ids)
    ))?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        labels
            .entry(parse_stored(&row.get::<_, String>(0)?)?)
            .or_default()
            .push(row.get(1)?);
    }
    Ok(labels
        .into_iter()
        .filter_map(|(id, labels)| Some((id, common_label(labels.into_iter())?)))
        .collect())
}

/// Fills in what a list of identities says of each beyond its row: its
/// label and the sources of its members.
fn describe_identities(conn: &Connection, identities: &mut [IdentitySummary]) -> Result<()> {
    if identities.is_empty() {
        return Ok(());
    }
    let ids: Vec<IdentityId> = identities.iter().map(|identity| identity.id).collect();
    let mut labels = read_identity_labels(conn, &ids)?;
    let mut sources: BTreeMap<IdentityId, Vec<SourceCount>> = BTreeMap::new();
    let mut statement = conn.prepare(&format!(
        "SELECT m.identity_id::VARCHAR, o.source_id, count(*) AS records
         FROM spool.membership m
         JOIN spool.record o ON o.record_id = m.record_id
         WHERE m.to_seq IS NULL AND list_contains({}, m.identity_id)
         GROUP BY m.identity_id, o.source_id
         ORDER BY m.identity_id, records DESC, o.source_id",
        uuid_list(&ids)
    ))?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        sources
            .entry(parse_stored(&row.get::<_, String>(0)?)?)
            .or_default()
            .push(SourceCount {
                source: row.get(1)?,
                records: row.get::<_, i64>(2)? as u64,
            });
    }
    let list = uuid_list(&ids);
    let mut names: BTreeMap<IdentityId, u64> = BTreeMap::new();
    let mut statement = conn.prepare(&format!(
        "SELECT m.identity_id::VARCHAR, count(DISTINCT lower(named.label))
         FROM spool.membership m
         JOIN spool.record_label named ON named.record_id = m.record_id
         WHERE m.to_seq IS NULL AND list_contains({list}, m.identity_id)
         GROUP BY m.identity_id"
    ))?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        names.insert(
            parse_stored(&row.get::<_, String>(0)?)?,
            row.get::<_, i64>(1)? as u64,
        );
    }
    // The links of `spool explain <identity>`, counted once a pair.
    let mut linked: BTreeMap<IdentityId, u64> = BTreeMap::new();
    let mut statement = conn.prepare(&format!(
        "WITH member AS (
           SELECT record_id, identity_id FROM spool.membership
           WHERE to_seq IS NULL AND list_contains({list}, identity_id)),
         link AS (
           SELECT record_a AS one, record_b AS other FROM spool.constraint_link
           WHERE to_seq IS NULL AND kind = 'must_link'
           UNION ALL
           SELECT supersedes, record_id FROM spool.record WHERE supersedes IS NOT NULL
           UNION ALL
           SELECT j.left_id, j.right_id FROM spool.judgement j
           JOIN spool.run r ON r.run_id = j.run_id AND r.status = 'succeeded'
           WHERE j.verdict = 'match' AND j.left_id IN (SELECT record_id FROM member)),
         pair AS (SELECT DISTINCT least(one, other) AS one, greatest(one, other) AS other
                  FROM link WHERE one <> other)
         SELECT a.identity_id::VARCHAR, count(*)
         FROM pair
         JOIN member a ON a.record_id = pair.one
         JOIN member b ON b.record_id = pair.other AND b.identity_id = a.identity_id
         GROUP BY a.identity_id"
    ))?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        linked.insert(
            parse_stored(&row.get::<_, String>(0)?)?,
            row.get::<_, i64>(1)? as u64,
        );
    }
    for identity in identities {
        identity.label = labels.remove(&identity.id);
        identity.sources = sources.remove(&identity.id).unwrap_or_default();
        identity.names = names.remove(&identity.id).unwrap_or_default();
        identity.linked_pairs = linked.remove(&identity.id).unwrap_or_default();
        let pairs = identity.members * identity.members.saturating_sub(1) / 2;
        identity.unlinked_pairs = pairs.saturating_sub(identity.linked_pairs);
    }
    Ok(())
}

const IDENTITY_COLUMNS: &str = "
    i.identity_id::VARCHAR, i.entity_type, i.created_seq,
           (SELECT count(*) FROM spool.membership m
            WHERE m.identity_id = i.identity_id AND m.to_seq IS NULL)";

fn identity_summary(row: &duckdb::Row<'_>) -> Result<IdentitySummary> {
    Ok(IdentitySummary {
        id: parse_stored(&row.get::<_, String>(0)?)?,
        entity_type: entity_type(row.get(1)?)?,
        label: None,
        created: Seq(row.get::<_, i64>(2)? as u64),
        members: row.get::<_, i64>(3)? as u64,
        names: 0,
        linked_pairs: 0,
        unlinked_pairs: 0,
        sources: Vec::new(),
        search_match: None,
    })
}

/// The active identities among `ids`, ordered by ID, as a list of
/// identities gives them.
pub(crate) fn identities_by_id(
    conn: &Connection,
    ids: &[IdentityId],
) -> Result<Vec<IdentitySummary>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut statement = conn.prepare(&format!(
        "SELECT {IDENTITY_COLUMNS} FROM spool.identity i
         WHERE i.merged_into IS NULL AND list_contains({}, i.identity_id)
         ORDER BY i.identity_id",
        uuid_list(ids)
    ))?;
    let mut rows = statement.query([])?;
    let mut found = Vec::new();
    while let Some(row) = rows.next()? {
        found.push(identity_summary(row)?);
    }
    describe_identities(conn, &mut found)?;
    Ok(found)
}

pub(crate) fn identities(
    conn: &Connection,
    filter: &IdentityFilter,
) -> Result<Vec<IdentitySummary>> {
    let search = encode_search_words(filter.q.as_deref());
    let filters = format!(
        "i.merged_into IS NULL
           AND ($1::VARCHAR IS NULL OR i.entity_type = $1::VARCHAR)
           AND {}",
        match &filter.ids {
            Some(ids) => format!("list_contains({}, i.identity_id)", uuid_list(ids)),
            None => "true".to_owned(),
        }
    );
    let select = if search.is_some() {
        format!(
            "SELECT {IDENTITY_COLUMNS}, im.tier, im.label, im.record_id::VARCHAR
             FROM spool.identity i JOIN identity_match im ON im.identity_id = i.identity_id"
        )
    } else {
        format!("SELECT {IDENTITY_COLUMNS} FROM spool.identity i")
    };
    let sql = match (filter.sort, search.is_some()) {
        (None, false) => format!(
            "{select}
             WHERE {filters}
               AND ($2::VARCHAR IS NULL OR i.identity_id > $2::UUID)
             ORDER BY i.identity_id
             LIMIT $3"
        ),
        (sort, searching) => {
            let labelled = sort.is_some_and(|sort| sort.by == IdentitySort::Label);
            let before = match (searching, labelled) {
                (true, _) => Some(identity_matches(&filters)),
                (false, true) => Some(identity_label("")),
                _ => None,
            };
            let from = match (sort, labelled) {
                (None, _) => "identity_match i",
                (_, true) => {
                    "spool.identity i LEFT JOIN identity_label il ON il.identity_id = i.identity_id"
                }
                _ => "spool.identity i",
            };
            let keyed = Keyed {
                before,
                from: from.to_owned(),
                id: "i.identity_id".to_owned(),
                keys: sort.map(|sort| sort.by.keys()).unwrap_or_else(|| {
                    ["i.tier", "i.quality", "i.identity_id"]
                        .into_iter()
                        .map(Key::of)
                        .collect()
                }),
                direction: sort.map(|sort| sort.direction).unwrap_or_default(),
            };
            format!(
                "{} {select} {}
                 WHERE {filters} AND {}
                 ORDER BY {}
                 LIMIT $3",
                keyed.with("$2::UUID"),
                keyed.join("i.identity_id"),
                keyed.after("$2::VARCHAR IS NOT NULL"),
                keyed.order_by(),
            )
        }
    };
    let mut statement = conn.prepare(&sql)?;
    let entity_type = filter.entity_type.as_ref().map(EntityType::as_str);
    let after = filter.after.map(|id| id.to_string());
    let most = clamp_limit(filter.limit);
    let mut rows = match &search {
        Some(search) => statement.query(params![entity_type, after, most, search])?,
        None => statement.query(params![entity_type, after, most])?,
    };
    let mut found = Vec::new();
    while let Some(row) = rows.next()? {
        let mut identity = identity_summary(row)?;
        if search.is_some() {
            let evidence = match row.get::<_, i32>(4)? {
                0 => IdentityMatch::Label { value: row.get(5)? },
                1 => IdentityMatch::Alias {
                    value: row.get(5)?,
                    record: parse_stored(&row.get::<_, String>(6)?)?,
                },
                _ => IdentityMatch::Record {
                    record: parse_stored(&row.get::<_, String>(6)?)?,
                    field: None,
                },
            };
            identity.search_match = Some(evidence);
        }
        found.push(identity);
    }
    if let Some(search) = search {
        describe_record_matches(conn, &mut found, &search)?;
    }
    describe_identities(conn, &mut found)?;
    Ok(found)
}

/// A label match outranks an alias, which outranks a match in a record's
/// other values. For labels and aliases, prefer exact text, then a prefix,
/// then the remaining matches. Each identity contributes its best match.
fn identity_matches(filters: &str) -> String {
    let record_holds = contains_words(RECORD_SEARCH_TEXT, "query.words");
    let label_holds = contains_words("named.label", "query.words");
    let labels = identity_label(
        "AND (m.identity_id IN (SELECT identity_id FROM matching_members)
              OR m.identity_id = $2::UUID)",
    );
    format!(
        "search_query AS (
           SELECT from_json($4::VARCHAR, '[\"VARCHAR\"]') AS words,
                  lower(strip_accents(array_to_string(words, ' '))) AS phrase
         ),
         matching_members AS (
           SELECT m.identity_id, m.record_id, named.label,
                  {label_holds} AS label_matches,
                  lower(strip_accents(regexp_replace(named.label, '[[:space:]]+', ' ', 'g'))) AS normalized_label,
                  CASE WHEN normalized_label = query.phrase THEN 0
                    WHEN starts_with(normalized_label, query.phrase) THEN 1
                    ELSE 2 END AS label_quality
           FROM spool.identity i
           JOIN spool.membership m ON m.identity_id = i.identity_id AND m.to_seq IS NULL
           JOIN spool.record o ON o.record_id = m.record_id
           LEFT JOIN spool.record_label named ON named.record_id = m.record_id
           CROSS JOIN search_query query
           WHERE {filters} AND (label_matches OR {record_holds})
         ),
         {labels},
         identity_match AS (
           SELECT member.identity_id, member.record_id, member.label,
                  CASE WHEN NOT member.label_matches THEN 2
                    WHEN member.label = il.label THEN 0 ELSE 1 END AS tier,
                  CASE WHEN member.label_matches THEN member.label_quality ELSE 0 END AS quality
           FROM matching_members member
           LEFT JOIN identity_label il ON il.identity_id = member.identity_id
           QUALIFY row_number() OVER (
             PARTITION BY member.identity_id ORDER BY tier, quality, member.record_id
           ) = 1
         )"
    )
}

fn describe_record_matches(
    conn: &Connection,
    identities: &mut [IdentitySummary],
    search: &str,
) -> Result<()> {
    let records: Vec<RecordId> = identities
        .iter()
        .filter_map(|identity| match identity.search_match {
            Some(IdentityMatch::Record { record, .. }) => Some(record),
            _ => None,
        })
        .collect();
    if records.is_empty() {
        return Ok(());
    }
    let field_holds = contains_words(
        "json_extract_string(field.value, '$')",
        "from_json($1::VARCHAR, '[\"VARCHAR\"]')",
    );
    let mut statement = conn.prepare(&format!(
        "SELECT o.record_id::VARCHAR, min(field.key)
         FROM spool.record o, json_each(o.payload) field
         WHERE list_contains({}, o.record_id)
           AND {field_holds}
         GROUP BY o.record_id",
        uuid_list(&records)
    ))?;
    let mut rows = statement.query([search])?;
    let mut fields = BTreeMap::new();
    while let Some(row) = rows.next()? {
        fields.insert(
            parse_stored::<RecordId>(&row.get::<_, String>(0)?)?,
            row.get(1)?,
        );
    }
    for identity in identities {
        if let Some(IdentityMatch::Record { record, field }) = &mut identity.search_match {
            *field = fields.remove(record);
        }
    }
    Ok(())
}
