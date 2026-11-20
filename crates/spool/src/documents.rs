//! Documents, and the mentions an extractor finds in them.
//!
//! A document is text a source holds under a key. The store keeps it so
//! that a record extracted from it can show where it was found, and
//! so that an extractor can be run again without the source being asked
//! again. A mention becomes a record whose key is its locator:
//!
//! ```text
//! <document id>#<entity type>:<start>-<end>
//! ```
//!
//! `start` and `end` count Unicode code points from the start of the text,
//! and `end` is the position after the last one. The document ID covers the
//! text, so a locator never points into two versions of a document.

use serde::{Deserialize, Serialize};
use spool_core::{DocumentId, EntityType, IdentityId, RecordId, RunId};

use crate::project::Extractor;
use crate::review::ReviewEntry;

mod mentions;
mod read;
mod staging;

pub use mentions::locator;
pub(crate) use mentions::{Mention, RawMention, TaskDocument, stage_mentions};
pub use read::{DocumentFilter, DocumentSort};
pub(crate) use read::{count, get, list};
pub(crate) use staging::{insert, stage};

/// A document as a source hands it over.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct NewDocument {
    /// The document's key within its source: a path, a URL, a message ID.
    pub key: String,
    /// Where the source keeps the document, if that is worth showing to
    /// someone who reads a mention.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    /// The document as text. Turning a PDF or a web page into text is the
    /// caller's work.
    pub text: String,
}

/// The stored document and the placements of the records found in it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AddDocumentReport {
    pub document: DocumentId,
    /// One entry per mention, in text order.
    pub records: Vec<DocumentRecord>,
}

/// The placement of one mention in an added document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DocumentRecord {
    pub record: RecordId,
    pub entity_type: EntityType,
    /// The start and exclusive end, in Unicode code points.
    pub start: u64,
    pub end: u64,
    pub identity: Option<IdentityId>,
    pub review: Option<ReviewEntry>,
}

/// One version of a document as the store holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DocumentEntry {
    pub id: DocumentId,
    pub source: String,
    pub key: String,
    pub uri: Option<String>,
    /// How long the text is, in Unicode code points.
    pub length: u64,
    /// The text. Left out of a list of documents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    pub loaded_at: String,
    pub load_run: RunId,
    /// The version of the document this one replaced: what the source held
    /// under the same key before.
    pub supersedes: Option<DocumentId>,
    /// The version that replaced this one.
    pub superseded_by: Option<DocumentId>,
    /// Each time an extractor read this version, oldest first.
    pub extractions: Vec<DocumentExtraction>,
    /// The extract run that is reading this version now. What it finds is
    /// stored when the run ends, and nothing is if it fails or is cancelled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reading: Option<DocumentReading>,
}

/// An extractor that is reading a document now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DocumentReading {
    pub extractor: String,
    /// The extract run.
    pub run: RunId,
}

/// One reading of a document by an extractor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DocumentExtraction {
    pub extractor: String,
    /// The hash of the extractor's files when it ran.
    pub version: String,
    /// The extract run.
    pub run: RunId,
    /// How many mentions it found in the document.
    pub mentions: u64,
    /// Why the extractor could not read the document. A document whose
    /// readings all failed is not read, and the next extraction reads it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
}

/// Which documents an extractor is to read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExtractOptions {
    /// The name of one of the project's extractors.
    pub extractor: String,
    /// Read only the documents of this source.
    pub source: Option<String>,
    /// Read the documents this extractor has read before as well. Without
    /// it an extraction reads only what the extractor has not read, whatever
    /// version of the extractor read it: a document it could not read is
    /// read again.
    pub again: bool,
    /// Leave out the documents this extractor read, or could not read, in
    /// this run or a later one. Reading in batches, each batch after the
    /// first names the first one's run, so that no document is read twice
    /// and one that could not be read is not tried again at once.
    pub since: Option<RunId>,
    /// Read at most this many documents, oldest first.
    pub limit: Option<u64>,
}

impl ExtractOptions {
    pub fn with(extractor: impl Into<String>) -> Self {
        Self {
            extractor: extractor.into(),
            ..Self::default()
        }
    }
}

/// Names the extractor of an extract run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ExtractorRef {
    pub name: String,
    /// The hash of the extractor's files.
    pub version: String,
}

impl From<&Extractor> for ExtractorRef {
    fn from(extractor: &Extractor) -> Self {
        Self {
            name: extractor.name.clone(),
            version: extractor.hash.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ExtractReport {
    pub run: RunId,
    /// Documents the extractor was given.
    pub documents: u64,
    /// Mentions it found.
    pub mentions: u64,
    /// Records created.
    pub inserted: u64,
    /// Mentions that were already known: the same place in the same
    /// document, with the same attributes.
    pub duplicates: u64,
    /// Records created as a new version of a mention that was known
    /// with other attributes.
    pub superseded: u64,
    /// The load run the records were stored under. There is none
    /// when nothing was found.
    pub load_run: Option<RunId>,
    /// Documents the extraction would have read but for its `limit`: what
    /// the same extraction reads next.
    #[serde(default)]
    pub left: u64,
    /// The documents the extractor could not read, with why. They count
    /// among `documents`, nothing found in them is stored, and they are not
    /// read.
    #[serde(default)]
    pub failed: Vec<FailedDocument>,
    /// Mentions taken back: found in a document before, and found by no
    /// extractor's latest reading of it now.
    #[serde(default)]
    pub taken_back: u64,
    /// The retract runs that took them back: one for each source and
    /// entity type.
    #[serde(default)]
    pub retract_runs: Vec<RunId>,
    /// What the extractor wrote to standard error, a line each: the last
    /// 100 lines.
    #[serde(default)]
    pub stderr: Vec<String>,
}

/// A document an extractor could not read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct FailedDocument {
    pub document: DocumentId,
    pub source: String,
    pub key: String,
    pub reason: String,
}

impl ExtractReport {
    pub(crate) fn empty(run: RunId) -> Self {
        Self {
            run,
            documents: 0,
            mentions: 0,
            inserted: 0,
            duplicates: 0,
            superseded: 0,
            load_run: None,
            left: 0,
            failed: Vec::new(),
            taken_back: 0,
            retract_runs: Vec::new(),
            stderr: Vec::new(),
        }
    }
}
