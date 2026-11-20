//! Explaining an identity: what holds it together, and where it holds
//! least. The links between its members are put in words here, for the
//! explanation of a pair as well, which names its chain and its cut by them.

use std::collections::BTreeMap;

use duckdb::{Connection, params};
use serde::{Deserialize, Serialize};
use spool_core::{EntityType, IdentityId, RecordId, RunId, StateError};

use super::feature_row;
use super::prose::identity_summary;
use crate::error::{Error, Result};
use crate::features;
use crate::links::{Graph, Parting};
use crate::lookup;
use crate::project::Manifest;
use crate::store::parse;

// Named by a doc comment only.
#[cfg(doc)]
use crate::lookup::RecordEntry;

/// One link between two members of an identity: what joins them directly.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Link {
    pub from: RecordId,
    pub to: RecordId,
    /// `from` and `to` as a person knows them.
    pub from_record: Named,
    pub to_record: Named,
    pub kind: LinkKind,
    /// With `match`: the latest run that judged so.
    pub run: Option<RunId>,
    /// With `match`: the strategy of that run.
    #[serde(default)]
    pub strategy: Option<String>,
    /// With `match`: the `match` rules that held, as the run recorded them.
    /// Empty for a strategy that keeps no rule names.
    #[serde(default)]
    pub rules: Vec<String>,
    /// With `match`: the score the run gave, on its strategy's own scale.
    #[serde(default)]
    pub score: Option<f64>,
    /// The features those rules read, with the values `from` and `to` have
    /// now: what satisfied the rules, unless the features changed since.
    #[serde(default)]
    pub values: Vec<LinkValue>,
}

/// A record as a person knows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Named {
    /// Its value of the feature its type names as the label.
    pub label: Option<String>,
    /// `source:key`, or for a mention `source:document[start-end]`, as
    /// [`RecordEntry::name`] says it.
    pub name: String,
}

/// One feature a rule of a link reads, at both ends of the link.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LinkValue {
    pub feature: String,
    pub from: serde_json::Value,
    pub to: serde_json::Value,
}

/// The fewest links whose removal would part an identity in two, and the
/// two sides.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Split {
    pub links: Vec<Link>,
    /// The two sides: for a pair, `left`'s first; for an identity, the
    /// smaller first. Members no link holds to either are on neither.
    pub sides: Vec<Side>,
}

/// One side of a [`Split`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Side {
    pub records: u64,
    /// Its most common labels, the most common first, at most five.
    pub names: Vec<NameCount>,
}

/// How many records have a label.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct NameCount {
    pub name: String,
    pub records: u64,
}

/// What one rule holds of an identity together.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RuleHold {
    /// A `match` rule, as the runs that judged the links recorded it.
    pub rule: String,
    /// The links it held for, alone or with other rules.
    pub links: u64,
    /// Of those, the links no other rule held for.
    pub alone: u64,
    /// How many pieces the identity would be in without the links only
    /// this rule made: 1 when the other links hold it together.
    pub pieces: u64,
    /// The largest of those pieces, the largest first, at most five.
    pub largest: Vec<Side>,
}

/// What holds an identity together, and where it holds least.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct IdentityExplanation {
    /// The ID that was asked for.
    pub id: IdentityId,
    /// The active identity it stands for: itself, or the one it was merged
    /// into. Everything below is of this one.
    pub current: IdentityId,
    pub entity_type: EntityType,
    pub label: Option<String>,
    pub records: u64,
    /// Different labels its records have, whatever their case. Records
    /// without a label are not counted.
    pub names: u64,
    /// Its most common labels, the most common first, at most five.
    pub top_names: Vec<NameCount>,
    /// Pairs of its records, `records × (records - 1) / 2`.
    pub pairs: u64,
    /// Pairs a link joins directly.
    pub linked_pairs: u64,
    /// Pairs that are together only through a chain of other records: no
    /// link joins them, and a chain does.
    pub chained_pairs: u64,
    /// Groups of records that links hold together. More than one when
    /// what joined them has been removed since, or a changeset placed a
    /// record directly.
    pub parts: u64,
    /// The most links a chain has between two records of the largest part,
    /// at the least: found from two of its records far apart, not from
    /// every pair.
    pub longest_chain: u64,
    /// The places where fewest links hold it together, where a wrong merge
    /// entered: each a set of links whose removal would part it, with the
    /// two sides, the smaller first. They are the links that alone hold a
    /// part on, and the fewest that part the records of its most common
    /// name from those of each of the next eight (or, without names, its
    /// two records farthest apart). Fewest links for each record they part
    /// off first; at most five, and none of more than twenty links.
    pub weakest: Vec<Split>,
    /// For each `match` rule its links were made by, what it holds
    /// together: the pieces the identity would be in without the links only
    /// that rule made. A rule that holds many pieces together is the one
    /// that chains different things. The rule that holds most pieces
    /// together first.
    pub rules: Vec<RuleHold>,
    /// The explanation in sentences.
    pub summary: Vec<String>,
}

/// What joins two records of one identity directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    /// A `must_link` constraint in force.
    MustLink,
    /// One is a later version of the other: the same row of the same
    /// source, sent again with other content.
    Version,
    /// A run that finished judged them `match`.
    Match,
}

/// The most labels a side or an identity is described by.
const TOP_NAMES: usize = 5;

/// The most splits an identity's explanation names.
const WEAKEST: usize = 5;

/// How many names after its most common an identity is cut from.
const NAME_CUTS: usize = 8;

/// What the links of one identity are, in words a person reads: each
/// record by its label and name, and each `match` by the rules that held
/// and the values they read. What it reads is kept for the next link.
pub(super) struct Describe<'a> {
    conn: &'a Connection,
    graph: &'a Graph,
    table: String,
    /// The `match` rules of the type, by name, when the store was applied
    /// with the manifest the engine has: only then are the feature rows
    /// those the rules read.
    rules: BTreeMap<&'a str, &'a str>,
    named: BTreeMap<RecordId, Named>,
    rows: BTreeMap<RecordId, Option<serde_json::Map<String, serde_json::Value>>>,
}

impl<'a> Describe<'a> {
    pub(super) fn new(
        conn: &'a Connection,
        manifest: Option<&'a Manifest>,
        entity_type: &EntityType,
        graph: &'a Graph,
    ) -> Self {
        let rules = manifest
            .and_then(|manifest| manifest.entity_type(entity_type.as_str()))
            .map(|declared| {
                declared
                    .rules
                    .match_
                    .iter()
                    .map(|rule| (rule.name.as_str(), rule.predicate.as_str()))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            conn,
            graph,
            table: features::table(entity_type),
            rules,
            named: BTreeMap::new(),
            rows: BTreeMap::new(),
        }
    }

    fn named(&mut self, id: RecordId) -> Result<Named> {
        if let Some(named) = self.named.get(&id) {
            return Ok(named.clone());
        }
        let record = lookup::record(self.conn, id)?.ok_or(Error::UnknownRecord(id))?;
        let named = Named {
            name: record.name(),
            label: record.label,
        };
        self.named.insert(id, named.clone());
        Ok(named)
    }

    fn row(&mut self, id: RecordId) -> Result<Option<serde_json::Map<String, serde_json::Value>>> {
        if !self.rows.contains_key(&id) {
            let row = feature_row(self.conn, &self.table, id)?;
            self.rows.insert(id, row);
        }
        Ok(self.rows[&id].clone())
    }

    /// The link of an edge, from the member numbered `from` to the other.
    pub(super) fn link(&mut self, edge: u32, from: u32) -> Result<Link> {
        let found = self.graph.edges[edge as usize];
        let to = if found.a == from { found.b } else { found.a };
        let (from, to) = (
            self.graph.members[from as usize],
            self.graph.members[to as usize],
        );
        let mut link = Link {
            from,
            to,
            from_record: self.named(from)?,
            to_record: self.named(to)?,
            kind: found.kind,
            run: found.run,
            strategy: None,
            rules: Vec::new(),
            score: None,
            values: Vec::new(),
        };
        if let (LinkKind::Match, Some(run)) = (found.kind, found.run) {
            let (left, right) = (from.min(to), from.max(to));
            let mut statement = self.conn.prepare(
                "SELECT r.strategy, j.score, j.detail::VARCHAR
                 FROM spool.judgement j
                 JOIN spool.run r ON r.run_id = j.run_id
                 WHERE j.run_id = ? AND j.left_id = ?::UUID AND j.right_id = ?::UUID
                   AND j.verdict = 'match'
                 LIMIT 1",
            )?;
            let mut rows =
                statement.query(params![run.0 as i64, left.to_string(), right.to_string()])?;
            if let Some(row) = rows.next()? {
                link.strategy = row.get(0)?;
                link.score = row.get(1)?;
                let detail: Option<serde_json::Value> = row
                    .get::<_, Option<String>>(2)?
                    .map(|detail| serde_json::from_str(&detail).map_err(StateError::new))
                    .transpose()?;
                link.rules = detail
                    .as_ref()
                    .and_then(|detail| detail.get("match"))
                    .and_then(serde_json::Value::as_array)
                    .map(|names| {
                        names
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default();
            }
        }
        let mut read: Vec<String> = Vec::new();
        for rule in &link.rules {
            if let Some(predicate) = self.rules.get(rule.as_str()) {
                for feature in features_read(predicate) {
                    if !read.contains(&feature) {
                        read.push(feature);
                    }
                }
            }
        }
        if !read.is_empty() {
            let (one, other) = (self.row(from)?, self.row(to)?);
            if let (Some(one), Some(other)) = (one, other) {
                for feature in read {
                    if let (Some(from), Some(to)) = (one.get(&feature), other.get(&feature)) {
                        link.values.push(LinkValue {
                            feature,
                            from: from.clone(),
                            to: to.clone(),
                        });
                    }
                }
            }
        }
        Ok(link)
    }

    /// A parting in words. Each of its links runs from the first side to
    /// the second.
    pub(super) fn split(&mut self, parting: &Parting, labels: &[Option<String>]) -> Result<Split> {
        let one: std::collections::HashSet<u32> = parting.one.iter().copied().collect();
        let mut links = Vec::new();
        for &edge in &parting.edges {
            let found = self.graph.edges[edge as usize];
            let from = if one.contains(&found.a) {
                found.a
            } else {
                found.b
            };
            links.push(self.link(edge, from)?);
        }
        Ok(Split {
            links,
            sides: vec![side(&parting.one, labels), side(&parting.other, labels)],
        })
    }
}

/// The features a rule reads: each `l.<name>` and `r.<name>` in its SQL,
/// in the order they first appear. A name in quotes is read too. What else
/// the SQL does with them is not looked at, so a rule that compares a
/// feature only in a function still names it.
pub(crate) fn features_read(predicate: &str) -> Vec<String> {
    let bytes = predicate.as_bytes();
    let word = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    let mut found: Vec<String> = Vec::new();
    let mut at = 0;
    while at + 2 < bytes.len() {
        let starts = matches!(bytes[at], b'l' | b'r' | b'L' | b'R')
            && bytes[at + 1] == b'.'
            && (at == 0 || !word(bytes[at - 1]));
        if !starts {
            at += 1;
            continue;
        }
        let begin = at + 2;
        let (name, end) = if bytes[begin] == b'"' {
            match predicate[begin + 1..].find('"') {
                Some(length) => (
                    &predicate[begin + 1..begin + 1 + length],
                    begin + 2 + length,
                ),
                None => break,
            }
        } else {
            let length = bytes[begin..]
                .iter()
                .take_while(|&&byte| word(byte))
                .count();
            (&predicate[begin..begin + length], begin + length)
        };
        if !name.is_empty() && !found.iter().any(|known| known == name) {
            found.push(name.to_owned());
        }
        at = end.max(at + 1);
    }
    found
}

/// The label of each member of an identity, by its number in the graph.
pub(super) fn labels(
    conn: &Connection,
    identity: IdentityId,
    graph: &Graph,
) -> Result<Vec<Option<String>>> {
    let mut labels = vec![None; graph.len()];
    let mut statement = conn.prepare(
        "SELECT m.record_id::VARCHAR, named.label
         FROM spool.membership m
         JOIN spool.record_label named ON named.record_id = m.record_id
         WHERE m.to_seq IS NULL AND m.identity_id = ?::UUID AND named.label IS NOT NULL",
    )?;
    let mut rows = statement.query([identity.to_string()])?;
    while let Some(row) = rows.next()? {
        let id: RecordId = parse(&row.get::<_, String>(0)?)?;
        if let Some(at) = graph.number(id) {
            labels[at as usize] = row.get(1)?;
        }
    }
    Ok(labels)
}

/// The labels of some members, the most common first, as many as `most`.
/// Labels that differ only in case are one, written as the most common
/// spelling has it.
fn common_names(members: &[u32], labels: &[Option<String>], most: usize) -> (u64, Vec<NameCount>) {
    let mut counted: BTreeMap<String, BTreeMap<&str, u64>> = BTreeMap::new();
    for &member in members {
        if let Some(label) = &labels[member as usize] {
            *counted
                .entry(label.to_lowercase())
                .or_default()
                .entry(label.as_str())
                .or_default() += 1;
        }
    }
    let distinct = counted.len() as u64;
    let mut names: Vec<NameCount> = counted
        .into_values()
        .map(|spellings| {
            let records = spellings.values().sum();
            let (name, _) = spellings
                .iter()
                .max_by_key(|(spelling, count)| (**count, std::cmp::Reverse(**spelling)))
                .expect("a label is counted");
            NameCount {
                name: (*name).to_owned(),
                records,
            }
        })
        .collect();
    names.sort_by(|one, other| {
        other
            .records
            .cmp(&one.records)
            .then_with(|| one.name.cmp(&other.name))
    });
    names.truncate(most);
    (distinct, names)
}

/// What each rule holds together. See [`IdentityExplanation::rules`].
fn rule_holds(graph: &Graph, labels: &[Option<String>]) -> Vec<RuleHold> {
    let mut holds = Vec::new();
    for (rule, name) in graph.rule_names.iter().enumerate() {
        let rule = rule as u32;
        let (mut links, mut alone) = (0, 0);
        for edge in &graph.edges {
            let rules = &graph.rule_sets[edge.rules as usize];
            if rules.contains(&rule) {
                links += 1;
                if rules.len() == 1 {
                    alone += 1;
                }
            }
        }
        let mut pieces = graph.pieces_without(rule);
        let count = pieces.len() as u64;
        pieces.truncate(TOP_NAMES);
        holds.push(RuleHold {
            rule: name.clone(),
            links,
            alone,
            pieces: count,
            largest: pieces.iter().map(|piece| side(piece, labels)).collect(),
        });
    }
    holds.sort_by(|one, other| {
        other
            .pieces
            .cmp(&one.pieces)
            .then_with(|| other.links.cmp(&one.links))
            .then_with(|| one.rule.cmp(&other.rule))
    });
    holds
}

/// The members of each label, whatever its case, the most numerous first.
fn name_groups(labels: &[Option<String>]) -> Vec<Vec<u32>> {
    let mut groups: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    for (at, label) in labels.iter().enumerate() {
        if let Some(label) = label {
            groups
                .entry(label.to_lowercase())
                .or_default()
                .push(at as u32);
        }
    }
    let mut groups: Vec<Vec<u32>> = groups.into_values().collect();
    groups.sort_by_key(|members| std::cmp::Reverse(members.len()));
    groups
}

fn side(members: &[u32], labels: &[Option<String>]) -> Side {
    Side {
        records: members.len() as u64,
        names: common_names(members, labels, TOP_NAMES).1,
    }
}

pub(crate) fn explain_identity(
    conn: &Connection,
    manifest: Option<&Manifest>,
    id: IdentityId,
) -> Result<IdentityExplanation> {
    let view = lookup::identity(conn, id)?.ok_or(Error::UnknownIdentity(id))?;
    let graph = Graph::read(conn, view.current)?;
    let labels = labels(conn, view.current, &graph)?;
    let everyone: Vec<u32> = (0..graph.len() as u32).collect();
    let (names, top_names) = common_names(&everyone, &labels, TOP_NAMES);
    let records = graph.len() as u64;
    let parts = graph.parts();
    let together: u64 = parts
        .iter()
        .map(|part| {
            let size = part.len() as u64;
            size * size.saturating_sub(1) / 2
        })
        .sum();
    let linked_pairs = graph.linked_pairs();
    let far = parts.first().and_then(|part| graph.far_apart(part));

    // Where a wrong merge entered: the links that alone hold a part on,
    // and the fewest that part the records of its most common name from
    // those of each of the next. Without names, the fewest that part the
    // two records of the largest part farthest apart.
    let mut partings = graph.bridges(WEAKEST);
    let groups = name_groups(&labels);
    let mut cuts = Vec::new();
    match groups.split_first() {
        Some((most, next)) if !next.is_empty() => {
            for group in next.iter().take(NAME_CUTS) {
                cuts.extend(graph.cut(most, group));
            }
        }
        _ => {
            if let Some((one, other, _)) = far {
                cuts.extend(graph.cut(&[one], &[other]));
            }
        }
    }
    for cut in cuts {
        let (one, other) = match cut.one.len() <= cut.other.len() {
            true => (cut.one, cut.other),
            false => (cut.other, cut.one),
        };
        partings.push(Parting {
            edges: cut.edges,
            one,
            other,
        });
    }
    for parting in &mut partings {
        parting.edges.sort();
    }
    // Fewest links for each record they part off first: two links that
    // part five records are weaker than sixteen that part seven, and than
    // one that parts one.
    partings.sort_by(|one, other| {
        (one.edges.len() * other.one.len())
            .cmp(&(other.edges.len() * one.one.len()))
            .then_with(|| other.one.len().cmp(&one.one.len()))
            .then_with(|| one.edges.cmp(&other.edges))
    });
    partings.dedup_by(|one, other| one.edges == other.edges);
    partings.truncate(WEAKEST);

    let mut describe = Describe::new(conn, manifest, &view.entity_type, &graph);
    let mut weakest = Vec::new();
    for parting in &partings {
        weakest.push(describe.split(parting, &labels)?);
    }
    let rules = rule_holds(&graph, &labels);

    let mut explanation = IdentityExplanation {
        id,
        current: view.current,
        entity_type: view.entity_type,
        label: view.label,
        records,
        names,
        top_names,
        pairs: records * records.saturating_sub(1) / 2,
        linked_pairs,
        chained_pairs: together.saturating_sub(linked_pairs),
        parts: parts.len() as u64,
        longest_chain: far.map_or(0, |(_, _, steps)| u64::from(steps)),
        weakest,
        rules,
        summary: Vec::new(),
    };
    explanation.summary = identity_summary(&explanation);
    Ok(explanation)
}
