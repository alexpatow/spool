//! `spool show attributes`: for each entity type and source, the attributes
//! its records have.

use anstream::println;
use spool::{AttributeGroup, AttributeSummary, DerivedFeature};
use spool_cli::text::count;

use super::{AttributesArgs, Query, View};
use crate::Failure;
use crate::commands::remote::typed;
use crate::output::{Cell, Table, hint};

impl View<'_> {
    pub(super) fn attributes(&self, what: &AttributesArgs) -> Result<(), Failure> {
        let query = Query::default()
            .add("entity_type", what.entity_type.as_ref())
            .add("source", what.source.as_ref())
            .0;
        let answer = self.remote.list("/attributes", &query)?;
        let groups: Vec<AttributeGroup> = typed(&answer["groups"])?;
        if !self.out.is_table() {
            let mut table = Table::new("attributes").columns([
                "entity type",
                "source",
                "records",
                "attribute",
                "key",
                "filled",
                "distinct",
                "most common",
                "features",
            ]);
            for group in &groups {
                for attribute in &group.attributes {
                    table.row(vec![
                        group.entity_type.to_string().into(),
                        group.source.clone().into(),
                        group.records.to_string().into(),
                        attribute.name.clone().into(),
                        attribute.key.to_string().into(),
                        attribute.filled.to_string().into(),
                        attribute.distinct.to_string().into(),
                        Cell::shortened(
                            common_values(attribute, true),
                            common_values(attribute, false),
                        ),
                        format_derived_features(&attribute.features).into(),
                    ]);
                }
            }
            self.out.items(&answer, "groups", &table)?;
            return Ok(());
        }
        if groups.is_empty() {
            match query.is_empty() {
                true => {
                    println!("no records");
                    hint(
                        "`spool load <file>` sends rows, and `spool extract` finds them in documents",
                    );
                }
                false => println!("no records of that entity type and source"),
            }
            return Ok(());
        }
        for (number, group) in groups.iter().enumerate() {
            if number > 0 {
                println!();
            }
            println!(
                "entity type `{}`, source `{}`: {}",
                group.entity_type,
                group.source,
                match group.records {
                    0 => "no records yet".to_owned(),
                    records => count(records, "record"),
                }
            );
            let mut table = Table::new("attributes").columns([
                "attribute",
                "filled",
                "distinct",
                "most common",
                "features",
            ]);
            for attribute in &group.attributes {
                table.row(vec![
                    match attribute.key {
                        true => format!("{} (key)", attribute.name),
                        false => attribute.name.clone(),
                    }
                    .into(),
                    format_filled_share(attribute.filled, group.records).into(),
                    attribute.distinct.to_string().into(),
                    common_values(attribute, true).into(),
                    format_derived_features(&attribute.features).into(),
                ]);
            }
            table.print();
        }
        let source = match &groups[..] {
            [only] => format!(" --source {}", only.source),
            _ => String::new(),
        };
        hint(format!(
            "`spool show records{source} --attributes` lists the values record by record"
        ));
        Ok(())
    }
}

/// How many records have a value, and what part of all of them that is.
/// A part that is not all is never written as 100%, nor some as 0%.
fn format_filled_share(filled: u64, records: u64) -> String {
    if records == 0 {
        return filled.to_string();
    }
    let percent = filled * 100 / records;
    let percent = match (filled, percent) {
        (0, _) => "0%".to_owned(),
        (_, 0) => "<1%".to_owned(),
        (filled, 100) if filled < records => "99%".to_owned(),
        (_, percent) => format!("{percent}%"),
    };
    format!("{filled} ({percent})")
}

/// The most characters of a value the list of attributes shows.
const VALUE_CHARACTERS: usize = 24;

/// The values most records have, with how many have each. When each is
/// held by one record they are examples, and said to be. A table cuts a
/// long value short, and CSV has it whole.
fn common_values(attribute: &AttributeSummary, cut: bool) -> String {
    if attribute.common.is_empty() {
        return "-".to_owned();
    }
    let short = |value: &str| -> String {
        let line: String = value
            .chars()
            .map(|character| match character {
                '\n' | '\r' | '\t' => ' ',
                other => other,
            })
            .collect();
        match cut && line.chars().count() > VALUE_CHARACTERS {
            true => {
                let kept: String = line.chars().take(VALUE_CHARACTERS - 1).collect();
                format!("{}…", kept.trim_end())
            }
            false => line,
        }
    };
    let examples = attribute.common.iter().all(|value| value.records == 1);
    let mut values: Vec<String> = attribute
        .common
        .iter()
        .map(|value| match examples {
            true => short(&value.value),
            false => format!("{} ({})", short(&value.value), value.records),
        })
        .collect();
    if attribute.distinct > attribute.common.len() as u64 {
        values.push("…".to_owned());
    }
    match examples && attribute.distinct > 1 {
        true => format!("e.g. {}", values.join(", ")),
        false => values.join(", "),
    }
}

/// The features made from an attribute: a feature that is the column as it
/// is by its name, and one computed from it as the feature, `=`, and what
/// computes it.
fn format_derived_features(features: &[DerivedFeature]) -> String {
    if features.is_empty() {
        return "-".to_owned();
    }
    features
        .iter()
        .map(|feature| match &feature.transform {
            None => feature.name.clone(),
            Some(transform) => format!(
                "{} = {transform}({})",
                feature.name,
                feature.from.join(", ")
            ),
        })
        .collect::<Vec<_>>()
        .join(", ")
}
