//! `spool show` without a subcommand: the server, its entity types and how
//! much there is of each.

use anstream::println;
use spool_cli::api::Info;
use spool_cli::text::count;

use super::View;
use crate::Failure;
use crate::commands::remote::typed;
use crate::output::{fields, indented};

impl View<'_> {
    pub(super) fn overview(&self) -> Result<(), Failure> {
        let answer = self.remote.list("/info", &[])?;
        if !self.out.is_table() {
            self.out.json(&answer);
            return Ok(());
        }
        let info: Info = typed(&answer)?;
        let store = &info.store;
        fields(&[
            ("server", self.remote.url.clone()),
            (
                "api",
                format!(
                    "{}, engine {}, store format {}",
                    info.api, store.engine_version, store.format_version
                ),
            ),
            (
                "manifest",
                store
                    .manifest_hash
                    .clone()
                    .unwrap_or_else(|| "none applied".to_owned()),
            ),
            ("head", format!("log position {}", store.head)),
            ("documents", store.documents.to_string()),
        ]);
        for entity_type in &store.entity_types {
            println!("\nentity type `{}`", entity_type.name);
            let mut rows = vec![
                (
                    "records",
                    match entity_type.retracted {
                        0 => format!(
                            "{}, {} without an identity",
                            entity_type.records, entity_type.unassigned
                        ),
                        retracted => format!(
                            "{}, {} without an identity, {retracted} taken back by their source",
                            entity_type.records, entity_type.unassigned
                        ),
                    },
                ),
                ("identities", entity_type.identities.to_string()),
                (
                    "review",
                    format!("{} open", count(entity_type.open_review_items, "item")),
                ),
            ];
            if let Some(draft) = entity_type.open_draft {
                rows.push(("draft", format!("{draft} waits for a decision")));
            }
            if !entity_type.strategies.is_empty() {
                rows.push((
                    "strategies",
                    entity_type
                        .strategies
                        .iter()
                        .map(|strategy| format!("{} ({})", strategy.name, strategy.runtime))
                        .collect::<Vec<_>>()
                        .join(", "),
                ));
            }
            indented(&rows);
        }
        if !store.extractors.is_empty() {
            println!();
        }
        for extractor in &store.extractors {
            println!(
                "extractor `{}` ({}, {})",
                extractor.name, extractor.runtime, extractor.file
            );
        }
        Ok(())
    }
}
