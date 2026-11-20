//! Benchmark scenarios.
//!
//! A scenario is an ordered list of steps with the truth about every
//! record. Each step ingests files, applies the corrections a reviewer
//! would make, and resolves, through the same calls any caller uses.
//! Committed state is then scored against the truth. Nothing is special-cased
//! for the benchmark.
//!
//! A scenario is a `scenario.toml`:
//!
//! ```toml
//! project = "."                 # the project directory, relative to this file
//!
//! [truth]
//! file = "truth.csv"            # columns: source, source_key, true_entity
//! # column = "cluster"          # or: a column of every ingested file
//!
//! [[step]]
//! name = "first batch"
//!
//! [[step.ingest]]
//! file = "batch-1.csv"          # or: dataset = "<name in datasets.toml>"
//! source = "crm"
//! entity_type = "company"
//! # member = "dir/part-1.csv"   # the file to read, when the dataset is an archive
//! # where = "region = 'north'"  # only the rows this holds for: SQL over the file's columns
//!
//! [[step]]
//! name = "review"               # a step may have corrections and no files
//!
//! [[step.correct]]
//! name = "wrong merge"          # optional: lets a later `undo` name it
//! merge = ["crm:7", "crm:9"]    # the identity of the first into that of the second
//!
//! [[step]]
//! name = "a better model"
//! strategy = "splink"           # the strategy this step resolves with
//! rematch = true                # and it judges every record again
//! ```
//!
//! Every column of a file is text, so a `where` that compares numbers casts
//! them: `id::BIGINT % 10 = 0` takes a tenth of the rows, and the same tenth
//! of every file that shares the IDs. The truth column may be the column the
//! source declares as its key, when the key is what says who a row is; any
//! other truth column is taken out of the rows before they are ingested.
//!
//! A step can also store documents and run extractors over them, before it
//! resolves:
//!
//! ```toml
//! [truth]
//! mentions = "mentions.csv"     # columns: source, document, start, end, entity_type, true_entity
//!
//! [[step]]
//! name = "the news"
//!
//! [[step.documents]]
//! dir = "documents"             # every file is a document, and its name is its key
//! source = "news"
//!
//! [[step.extract]]
//! extractor = "names"           # one of the project's extractors
//! ```
//!
//! A labelled mention says that a document mentions an entity at a place:
//! `start` and `end` count Unicode code points, and `end` is the position
//! after the last one. A `text` column, if the file has one, must hold the
//! document's text at that place, which catches a label that is off by
//! one. The labels are the truth twice over. Extraction is scored against
//! them on its own: a mention the extractor found is right when a label has
//! the same document, entity type, start and end. Resolution is scored as it
//! is for rows, over the records that have a true entity, which for an
//! extracted mention is the label at the same place. A mention the
//! extractor made up has no true entity and is not scored there, so the two
//! scores do not leak into each other. A third score is the two together,
//! end to end: the identities of everything the extractor found against
//! every labelled mention, where a mention that was missed is in no
//! identity and one that was made up is no entity.
//!
//! A step can take rows back, as a source does that no longer holds them:
//!
//! ```toml
//! [[step]]
//! name = "struck off"
//! rebuild = true                # compare what is left with resolving it afresh
//!
//! [[step.retract]]
//! source = "registry"
//! entity_type = "company"
//! keys = ["X0000017"]           # the keys of the rows
//!
//! [[step.retract]]
//! source = "crm"
//! entity_type = "company"
//! file = "closed-accounts.csv"  # or: the rows of a file or a `dataset`,
//! where = "closed = 'yes'"      # read as an ingest reads them, for their keys
//! ```
//!
//! Rows are taken back after the step's files are ingested and before its
//! corrections. From then on the scenario is scored on the rows the sources
//! still hold. A step with `rebuild = true` also runs the steps so far in a
//! store of its own, without the rows that are taken back as things stand
//! and without the corrections, and reports how its identities differ from
//! those of the store that held the rows and lost them. A mention that is
//! taken back is extracted there too, since it is in a document, and is
//! taken back before anything is matched with it.
//!
//! The store is verified after every step, and the scenario fails at the
//! first step that leaves it inconsistent.
//!
//! A step resolves with its own `strategy`, or else with the scenario's
//! top-level `strategy`, or else with the built-in `rules`. A step with
//! `rematch = true` matches everything again, which is what switching
//! strategy on the same data means, and reports upgrade stability.
//!
//! A correction names records as `source:key`, and is one of:
//!
//! | Field | What it does |
//! | --- | --- |
//! | `merge = [a, b]` | merges the identity of `a` into the identity of `b` |
//! | `assign = [a, b]` | puts `a` into the identity of `b` |
//! | `split = [a, ...]` | splits the records from the rest of their identity, and keeps the sides apart |
//! | `must_link = [a, b]` | records that the two share an identity |
//! | `cannot_link = [a, b]` | records that the two never share an identity |
//! | `undo = "name"` | undoes the named correction |
//!
//! Corrections are applied after the step's files are ingested and before
//! the step resolves, so a step of corrections alone shows what the next run
//! makes of them.

mod corrections;
mod datasets;
mod error;
mod metrics;
mod replay;
mod report;
mod rows;
mod run;
mod scenario;
mod score;
mod spec;
mod truth;

pub use error::{Error, Result};
pub use metrics::{
    EndToEndMetrics, ExtractionMetrics, OrderSensitivity, OverridePersistence, Rebuild,
    SplitRecovery, UpgradeStability,
};
pub use report::{
    BenchReport, CorrectionSummary, DocumentsSummary, ExtractSummary, IngestSummary,
    PossibleSplits, RetractSummary, StepReport,
};
pub use rows::take_column;
pub use scenario::Scenario;
