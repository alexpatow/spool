//! An entity type's `entity.yaml`: its YAML form, and the query that
//! computes the type's features.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use super::{Mapped, Source, Transform, YamlFile, identifier};
use crate::features;

/// An entity type's file, as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EntityYaml {
    #[serde(default)]
    label: Option<String>,
    features: Vec<String>,
    /// Source, then feature, then where the feature comes from.
    mapping: BTreeMap<String, BTreeMap<String, MappedYaml>>,
}

/// Where a feature comes from in one source: a column as it is, or columns
/// given to a transform.
#[derive(Deserialize)]
#[serde(untagged)]
enum MappedYaml {
    Column(String),
    Transformed(TransformedYaml),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TransformedYaml {
    from: FromYaml,
    #[serde(default)]
    transform: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum FromYaml {
    One(String),
    Many(Vec<String>),
}

/// What an `entity.yaml` compiles to.
pub(super) struct Entity {
    pub(super) query: String,
    pub(super) label: Option<String>,
    /// The transforms the query calls, in name order.
    pub(super) transforms: Vec<Transform>,
    /// The sources the mapping reads, in name order.
    pub(super) sources: Vec<String>,
    pub(super) mapping: BTreeMap<String, BTreeMap<String, Mapped>>,
}

/// Checks an `entity.yaml` against the sources and transforms it names, and
/// writes the query that computes its features: one row per record, each
/// feature from the columns its source's mapping says.
#[expect(clippy::too_many_lines)]
pub(super) fn entity_of(
    file: &YamlFile,
    sources: &[Source],
    transforms: &BTreeMap<String, Transform>,
) -> Result<Entity, String> {
    let written: EntityYaml =
        serde_saphyr::from_str(&file.yaml).map_err(|error| error.to_string())?;

    if written.features.is_empty() {
        return Err("an entity type lists its features".to_owned());
    }
    let mut features = BTreeSet::new();
    for feature in &written.features {
        if !identifier(feature) {
            return Err(format!(
                "`{feature}` cannot name a feature: use lowercase letters, digits and `_`"
            ));
        }
        if ["record_id", "declaration_version", "identity_id", "is_new"].contains(&feature.as_str())
        {
            return Err(format!(
                "`{feature}` is a column the engine adds; give the feature another name"
            ));
        }
        if !features.insert(feature.as_str()) {
            return Err(format!("feature `{feature}` is listed twice"));
        }
    }
    if let Some(label) = &written.label
        && !features.contains(label.as_str())
    {
        return Err(format!(
            "the label is `{label}`, which is not one of the features"
        ));
    }
    if written.mapping.is_empty() {
        return Err("the mapping names no source, so nothing can become a record".to_owned());
    }

    // Feature, then the expression each source computes it with.
    let mut expressions: BTreeMap<&str, Vec<(&str, String)>> = BTreeMap::new();
    let mut used: BTreeMap<&str, &Transform> = BTreeMap::new();
    let mut mapping: BTreeMap<String, BTreeMap<String, Mapped>> = BTreeMap::new();
    for (source_name, mapped) in &written.mapping {
        let source = sources
            .iter()
            .find(|source| &source.name == source_name)
            .ok_or_else(|| {
                format!("the mapping reads source `{source_name}`, and there is no sources/{source_name}.yaml")
            })?;
        for (feature, mapped) in mapped {
            let at = format!("mapping of `{feature}` from `{source_name}`");
            if !features.contains(feature.as_str()) {
                return Err(format!("{at}: `{feature}` is not one of the features"));
            }
            let (from, transform) = match mapped {
                MappedYaml::Column(column) => (vec![column.as_str()], None),
                MappedYaml::Transformed(mapped) => (
                    match &mapped.from {
                        FromYaml::One(column) => vec![column.as_str()],
                        FromYaml::Many(columns) => columns.iter().map(String::as_str).collect(),
                    },
                    mapped.transform.as_deref(),
                ),
            };
            let mut arguments = Vec::new();
            for column in &from {
                if !source.columns.iter().any(|known| known == column) {
                    return Err(format!(
                        "{at}: `{column}` is not a column of sources/{source_name}.yaml"
                    ));
                }
                // The column's value as text, whatever JSON type it has.
                arguments.push(format!(
                    "json_extract_string(payload, '$.\"{}\"')",
                    column.replace('\'', "''")
                ));
            }
            let expression = match transform {
                None => match &arguments[..] {
                    [one] => one.clone(),
                    _ => {
                        return Err(format!(
                            "{at}: {} columns need a transform to become one feature",
                            arguments.len()
                        ));
                    }
                },
                Some(name) => {
                    let function = match transforms.get(name) {
                        Some(transform) => {
                            if transform.parameters.len() != arguments.len() {
                                return Err(format!(
                                    "{at}: transform `{name}` takes {} ({}), and is given {}",
                                    transform.parameters.len(),
                                    transform.parameters.join(", "),
                                    arguments.len()
                                ));
                            }
                            used.insert(name, transform);
                            features::transform_function(name)
                        }
                        // A function DuckDB has, such as `upper`.
                        None if identifier(name) => name.to_owned(),
                        None => {
                            return Err(format!(
                                "{at}: `{name}` is not a file in transforms/ and cannot name a function"
                            ));
                        }
                    };
                    format!("{function}({})", arguments.join(", "))
                }
            };
            mapping.entry(source_name.clone()).or_default().insert(
                feature.clone(),
                Mapped {
                    from: from.iter().map(|column| (*column).to_owned()).collect(),
                    transform: transform.map(str::to_owned),
                },
            );
            expressions
                .entry(feature.as_str())
                .or_default()
                .push((source_name.as_str(), expression));
        }
    }

    let mut columns = Vec::new();
    for feature in &written.features {
        let Some(by_source) = expressions.get(feature.as_str()) else {
            return Err(format!("feature `{feature}` is not mapped from any source"));
        };
        let cases: Vec<String> = by_source
            .iter()
            .map(|(source, expression)| {
                format!("WHEN '{}' THEN {expression}", source.replace('\'', "''"))
            })
            .collect();
        columns.push(format!(
            "  CASE source_id\n    {}\n  END AS {feature}",
            cases.join("\n    ")
        ));
    }

    Ok(Entity {
        query: format!("SELECT\n  record_id,\n{}\nFROM record", columns.join(",\n")),
        label: written.label,
        transforms: used.into_values().cloned().collect(),
        sources: written.mapping.into_keys().collect(),
        mapping,
    })
}
