//! The tool that says why two records share an identity or do not.

use schemars::JsonSchema;
use serde::Deserialize;

use super::{Declared, Effect, declare};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Pair {
    /// One record: its ID, or `<source>:<key>`, such as `crm:c-1`, which
    /// names the row its source holds under that key now. What an extractor
    /// found in a document is also named by its place, as
    /// `<source>:<document key>[<start>-<end>]`. The order of the two does
    /// not matter.
    a: String,
    /// The other record, named the same way.
    b: String,
}

pub(super) fn explain_pair() -> Declared {
    declare(
        "explain_pair",
        "Explain two records",
        Effect::Reads,
        "Why two records share an identity, or why they do not: what the project's \
         rules say about the pair, what each match run judged, and the constraints \
         between them. Each record is named by its ID or as `<source>:<key>`. Use this \
         before merging or splitting, and when asked why two things were or were not \
         treated as the same.",
        |app, _, asked: Pair| async move { app.explain_pair(&asked.a, &asked.b).await },
    )
}
