//! What is read from a store to score it: how its records are grouped,
//! what each truly is, and what its review items ask.

use std::collections::{BTreeSet, HashMap, HashSet};

use spool::arrow::array::AsArray;
use spool::arrow::datatypes::Int64Type;
use spool::core::RecordId;
use spool::{Engine, NOT_RETRACTED, ReviewFilter, ReviewKind, Rows};

use crate::error::Result;
use crate::metrics::{EndToEndMetrics, ExtractionMetrics};
use crate::report::PossibleSplits;
use crate::truth::Truth;

/// The pairs of records the open `possible_split` review items ask about,
/// each with its item: two records in different groups of one item, the
/// smaller ID first.
pub(crate) fn asked(engine: &Engine) -> Result<Vec<(u64, RecordId, RecordId)>> {
    let mut pairs = Vec::new();
    for item in engine.review_items(&ReviewFilter::open())? {
        if item.kind != ReviewKind::PossibleSplit {
            continue;
        }
        for (index, group) in item.groups.iter().enumerate() {
            for other in item.groups[index + 1..].iter().flatten() {
                for record in group {
                    pairs.push((item.id, *record.min(other), *record.max(other)));
                }
            }
        }
    }
    Ok(pairs)
}

/// The open `possible_split` review items against the truth, or `None` in
/// a store where no retraction has raised one.
pub(crate) fn possible_splits(
    engine: &Engine,
    truth: &HashMap<(String, String), String>,
) -> Result<Option<PossibleSplits>> {
    let count = |sql: &str| -> Result<u64> {
        let batches = engine.query(sql)?;
        Ok(batches.first().map_or(0, |batch| {
            batch.column(0).as_primitive::<Int64Type>().value(0)
        }) as u64)
    };
    let about = "FROM spool.review_item WHERE kind = 'possible_split'";
    if count(&format!("SELECT count(*) {about}"))? == 0 {
        return Ok(None);
    }
    let open = format!("{about} AND status = 'open'");
    // What each record the open items name truly is, where the truth says.
    let batches = engine.query(&format!(
        "SELECT record_id::VARCHAR, source_id, source_key FROM spool.record
         WHERE source_key IS NOT NULL
           AND record_id IN (SELECT unnest(record_ids) {open})"
    ))?;
    let mut entity_of: HashMap<String, &str> = HashMap::new();
    for batch in &batches {
        let column = |index: usize| batch.column(index).as_string::<i32>();
        let (ids, sources, keys) = (column(0), column(1), column(2));
        for row in 0..batch.num_rows() {
            let key = (sources.value(row).to_owned(), keys.value(row).to_owned());
            if let Some(entity) = truth.get(&key) {
                entity_of.insert(ids.value(row).to_owned(), entity);
            }
        }
    }
    let pairs = asked(engine)?;
    let mut wrong = BTreeSet::new();
    let mut wrong_pairs = 0;
    for (item, one, other) in &pairs {
        let entities = (
            entity_of.get(&one.to_string()),
            entity_of.get(&other.to_string()),
        );
        if let (Some(one), Some(other)) = entities
            && one != other
        {
            wrong_pairs += 1;
            wrong.insert(*item);
        }
    }
    Ok(Some(PossibleSplits {
        open: count(&format!("SELECT count(*) {open}"))?,
        records: count(&format!(
            "SELECT count(DISTINCT record_id) FROM (SELECT unnest(record_ids) AS record_id {open})"
        ))?,
        pairs: pairs.len() as u64,
        wrong_pairs,
        wrong: wrong.len() as u64,
    }))
}

/// Every record its source has not taken back, with the group it is in:
/// its identity, or itself when it has none.
pub(crate) fn grouping(engine: &Engine) -> Result<Vec<(String, String)>> {
    let batches = engine.query(&format!(
        "SELECT o.record_id::VARCHAR,
                coalesce(m.identity_id::VARCHAR, 'unassigned:' || o.record_id::VARCHAR)
         FROM (SELECT record_id FROM spool.record WHERE {NOT_RETRACTED}) o
         LEFT JOIN spool.membership m ON m.record_id = o.record_id AND m.to_seq IS NULL
         ORDER BY o.record_id"
    ))?;
    let mut groups = Vec::new();
    for batch in &batches {
        let column = |index: usize| batch.column(index).as_string::<i32>();
        let (records, identities) = (column(0), column(1));
        for row in 0..batch.num_rows() {
            groups.push((
                records.value(row).to_owned(),
                identities.value(row).to_owned(),
            ));
        }
    }
    Ok(groups)
}

/// The truth relation `(record_id, true_entity)` for the records
/// the store holds, joined to the truth by source and key.
pub(crate) fn truth_relation(
    engine: &Engine,
    truth: &HashMap<(String, String), String>,
) -> Result<Rows> {
    // A record its source took back is no longer something to be right
    // or wrong about.
    let records = engine.query(&format!(
        "SELECT record_id::VARCHAR, source_id, source_key
         FROM spool.record WHERE source_key IS NOT NULL AND {NOT_RETRACTED}"
    ))?;
    let mut rows: Vec<(String, &str)> = Vec::new();
    for batch in &records {
        let column = |index: usize| batch.column(index).as_string::<i32>();
        let (ids, sources, keys) = (column(0), column(1), column(2));
        for row in 0..batch.num_rows() {
            let key = (sources.value(row).to_owned(), keys.value(row).to_owned());
            if let Some(entity) = truth.get(&key) {
                rows.push((ids.value(row).to_owned(), entity));
            }
        }
    }
    let rows: Vec<Vec<Option<&str>>> = rows
        .iter()
        .map(|(id, entity)| vec![Some(id.as_str()), Some(*entity)])
        .collect();
    Ok(Rows::from_text(&["record_id", "true_entity"], &rows)?)
}

/// How the mentions extracted so far compare with the labelled mentions of
/// the documents stored so far. A mention is one place in one document,
/// however many versions of the record there are.
pub(crate) fn extraction(engine: &Engine, truth: &Truth) -> Result<Option<ExtractionMetrics>> {
    if !truth.labelled {
        return Ok(None);
    }
    let found = engine.query(
        "SELECT DISTINCT source_id, source_key FROM spool.record
         WHERE json_extract_string(evidence, '$.document') IS NOT NULL",
    )?;
    let (mut extracted, mut correct) = (0, 0);
    for batch in &found {
        let column = |index: usize| batch.column(index).as_string::<i32>();
        let (sources, keys) = (column(0), column(1));
        for row in 0..batch.num_rows() {
            extracted += 1;
            let key = (sources.value(row).to_owned(), keys.value(row).to_owned());
            if truth.gold.contains(&key) {
                correct += 1;
            }
        }
    }
    Ok(Some(ExtractionMetrics::new(
        truth.gold.len() as u64,
        extracted,
        correct,
    )))
}

/// The identities of the mentions extracted so far against every labelled
/// mention of the documents stored so far. A mention its source took back
/// is not one the store holds, and only the latest version of a mention
/// counts.
pub(crate) fn end_to_end(engine: &Engine, truth: &Truth) -> Result<Option<EndToEndMetrics>> {
    if !truth.labelled {
        return Ok(None);
    }
    let found = engine.query(&format!(
        "SELECT o.source_id, o.source_key,
                coalesce(m.identity_id::VARCHAR, 'unassigned:' || o.record_id::VARCHAR)
         FROM (SELECT * FROM spool.record WHERE {NOT_RETRACTED}) o
         LEFT JOIN spool.membership m ON m.record_id = o.record_id AND m.to_seq IS NULL
         WHERE json_extract_string(o.evidence, '$.document') IS NOT NULL
           AND NOT EXISTS (SELECT 1 FROM spool.record later WHERE later.supersedes = o.record_id)"
    ))?;
    let mut mentions: Vec<((String, String), String)> = Vec::new();
    for batch in &found {
        let column = |index: usize| batch.column(index).as_string::<i32>();
        let (sources, keys, groups) = (column(0), column(1), column(2));
        for row in 0..batch.num_rows() {
            mentions.push((
                (sources.value(row).to_owned(), keys.value(row).to_owned()),
                groups.value(row).to_owned(),
            ));
        }
    }
    // A labelled mention that was found and taken back is not one that was
    // missed.
    let taken_back = engine.query(&format!(
        "SELECT DISTINCT source_id, source_key FROM spool.record
         WHERE source_key IS NOT NULL AND NOT ({NOT_RETRACTED})
           AND json_extract_string(evidence, '$.document') IS NOT NULL"
    ))?;
    let mut gone = HashSet::new();
    for batch in &taken_back {
        let column = |index: usize| batch.column(index).as_string::<i32>();
        let (sources, keys) = (column(0), column(1));
        for row in 0..batch.num_rows() {
            gone.insert((sources.value(row).to_owned(), keys.value(row).to_owned()));
        }
    }
    let labelled: Vec<&str> = truth
        .gold
        .iter()
        .filter(|place| !gone.contains(*place))
        .filter_map(|place| truth.entities.get(place))
        .map(String::as_str)
        .collect();
    let extracted: Vec<(Option<&str>, &str)> = mentions
        .iter()
        .map(|(place, group)| {
            let entity = match truth.gold.contains(place) {
                true => truth.entities.get(place).map(String::as_str),
                false => None,
            };
            (entity, group.as_str())
        })
        .collect();
    Ok(Some(EndToEndMetrics::new(&labelled, &extracted)))
}
