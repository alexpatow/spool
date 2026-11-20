//! The file format of a scenario: what a `scenario.toml` holds.

use std::path::PathBuf;

use serde::Deserialize;
use spool::core::ConstraintKind;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Spec {
    #[serde(default = "here")]
    pub(crate) project: PathBuf,
    /// The strategy of every step that does not name its own.
    pub(crate) strategy: Option<String>,
    pub(crate) truth: TruthSpec,
    #[serde(rename = "step")]
    pub(crate) steps: Vec<StepSpec>,
}

fn here() -> PathBuf {
    PathBuf::from(".")
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TruthSpec {
    pub(crate) file: Option<PathBuf>,
    pub(crate) column: Option<String>,
    /// Labelled mentions: the truth about what the documents mention.
    pub(crate) mentions: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StepSpec {
    pub(crate) name: String,
    pub(crate) strategy: Option<String>,
    #[serde(default)]
    pub(crate) rematch: bool,
    #[serde(default)]
    pub(crate) ingest: Vec<IngestSpec>,
    #[serde(default)]
    pub(crate) documents: Vec<DocumentsSpec>,
    #[serde(default)]
    pub(crate) extract: Vec<ExtractSpec>,
    #[serde(default)]
    pub(crate) retract: Vec<RetractSpec>,
    #[serde(default, rename = "correct")]
    pub(crate) corrections: Vec<CorrectionSpec>,
    /// Whether what the step leaves is compared with resolving the rows
    /// the sources still hold in a store of their own.
    #[serde(default)]
    pub(crate) rebuild: bool,
}

impl StepSpec {
    /// Whether the step brings anything new into the store.
    pub(crate) fn feeds(&self) -> bool {
        !self.ingest.is_empty() || !self.documents.is_empty() || !self.extract.is_empty()
    }
}

/// Rows a source takes back: their keys, or the rows of a file, which are
/// read as an ingest reads them and name the rows by the source's key
/// column.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RetractSpec {
    pub(crate) keys: Option<Vec<String>>,
    pub(crate) file: Option<PathBuf>,
    pub(crate) dataset: Option<String>,
    /// The file to read within a dataset that is an archive.
    pub(crate) member: Option<String>,
    /// A SQL condition on the file's columns. Only the rows it holds for
    /// are taken back.
    #[serde(rename = "where")]
    pub(crate) only: Option<String>,
    pub(crate) source: String,
    pub(crate) entity_type: String,
}

impl RetractSpec {
    pub(crate) fn check(&self) -> std::result::Result<(), String> {
        let named = [
            self.keys.is_some(),
            self.file.is_some(),
            self.dataset.is_some(),
        ];
        if named.iter().filter(|named| **named).count() != 1 {
            return Err(
                "a retraction needs exactly one of `keys`, `file` and `dataset`".to_owned(),
            );
        }
        if self.member.is_some() && self.dataset.is_none() {
            return Err(
                "`member` names a file within a `dataset`, and the retraction has none".to_owned(),
            );
        }
        if self.only.is_some() && self.keys.is_some() {
            return Err(
                "`where` chooses among the rows of a `file` or a `dataset`, and the retraction \
                 names `keys`"
                    .to_owned(),
            );
        }
        Ok(())
    }
}

/// A directory of documents: every file in it is one, keyed by its name.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocumentsSpec {
    pub(crate) dir: PathBuf,
    pub(crate) source: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExtractSpec {
    pub(crate) extractor: String,
    pub(crate) source: Option<String>,
    #[serde(default)]
    pub(crate) again: bool,
}

/// One thing a reviewer does. Exactly one of the fields after `reason` is
/// set, which is checked when the scenario is loaded.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CorrectionSpec {
    pub(crate) name: Option<String>,
    pub(crate) reason: Option<String>,
    pub(crate) merge: Option<[String; 2]>,
    pub(crate) assign: Option<[String; 2]>,
    pub(crate) split: Option<Vec<String>>,
    pub(crate) must_link: Option<[String; 2]>,
    pub(crate) cannot_link: Option<[String; 2]>,
    pub(crate) undo: Option<String>,
}

pub(crate) enum Correction<'a> {
    Merge(&'a [String; 2]),
    Assign(&'a [String; 2]),
    Split(&'a [String]),
    Constrain(ConstraintKind, &'a [String; 2]),
    Undo(&'a str),
}

impl CorrectionSpec {
    pub(crate) fn kind(&self) -> std::result::Result<Correction<'_>, String> {
        let mut kinds = Vec::new();
        if let Some(pair) = &self.merge {
            kinds.push(Correction::Merge(pair));
        }
        if let Some(pair) = &self.assign {
            kinds.push(Correction::Assign(pair));
        }
        if let Some(records) = &self.split {
            kinds.push(Correction::Split(records));
        }
        if let Some(pair) = &self.must_link {
            kinds.push(Correction::Constrain(ConstraintKind::MustLink, pair));
        }
        if let Some(pair) = &self.cannot_link {
            kinds.push(Correction::Constrain(ConstraintKind::CannotLink, pair));
        }
        if let Some(name) = &self.undo {
            kinds.push(Correction::Undo(name));
        }
        let kind = kinds.pop();
        match kind {
            Some(kind) if kinds.is_empty() => Ok(kind),
            _ => Err(
                "a correction needs exactly one of `merge`, `assign`, `split`, `must_link`, \
                 `cannot_link` and `undo`"
                    .to_owned(),
            ),
        }
    }
}

impl Correction<'_> {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Correction::Merge(_) => "merge",
            Correction::Assign(_) => "assign",
            Correction::Split(_) => "split",
            Correction::Constrain(ConstraintKind::MustLink, _) => "must_link",
            Correction::Constrain(ConstraintKind::CannotLink, _) => "cannot_link",
            Correction::Undo(_) => "undo",
        }
    }

    pub(crate) fn records(&self) -> &[String] {
        match self {
            Correction::Merge(pair) | Correction::Assign(pair) | Correction::Constrain(_, pair) => {
                &pair[..]
            }
            Correction::Split(records) => records,
            Correction::Undo(_) => &[],
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct IngestSpec {
    pub(crate) file: Option<PathBuf>,
    pub(crate) dataset: Option<String>,
    /// The file to read within a dataset that is an archive.
    pub(crate) member: Option<String>,
    /// A SQL condition on the file's columns. Only the rows it holds for
    /// are ingested.
    #[serde(rename = "where")]
    pub(crate) only: Option<String>,
    pub(crate) source: String,
    pub(crate) entity_type: String,
}
