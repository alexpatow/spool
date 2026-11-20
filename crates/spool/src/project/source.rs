//! A source's file: its YAML form, and the source it declares.

use std::collections::BTreeSet;

use serde::Deserialize;

use super::{Source, YamlFile, valid_name};

/// A source's file, as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceYaml {
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    evidence: Option<String>,
    columns: Vec<String>,
}

pub(super) fn source_of(file: &YamlFile) -> Result<Source, String> {
    if !valid_name(&file.name) {
        return Err(
            "a source's file name may contain only letters, digits, `_` and `-`".to_owned(),
        );
    }
    let written: SourceYaml =
        serde_saphyr::from_str(&file.yaml).map_err(|error| error.to_string())?;
    if written.columns.is_empty() {
        return Err("a source lists the columns its rows may have".to_owned());
    }
    let mut seen = BTreeSet::new();
    for column in &written.columns {
        if column.is_empty() || column.contains(['"', '\\']) {
            return Err(format!(
                "column `{column}`: a column needs a name without `\"` or `\\`"
            ));
        }
        if !seen.insert(column.as_str()) {
            return Err(format!("column `{column}` is listed twice"));
        }
    }
    for (what, column) in [("key", &written.key), ("evidence", &written.evidence)] {
        if let Some(column) = column
            && !seen.contains(column.as_str())
        {
            return Err(format!(
                "the {what} is `{column}`, which is not one of the columns"
            ));
        }
    }
    if written.key.is_some() && written.key == written.evidence {
        return Err("the key and the evidence are the same column".to_owned());
    }
    Ok(Source {
        name: file.name.clone(),
        key: written.key,
        evidence: written.evidence,
        columns: written.columns,
    })
}
