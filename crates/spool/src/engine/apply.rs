//! Applying a manifest: declaring its entity types in the store and
//! computing their feature tables.

use std::sync::Arc;

use duckdb::params;
use serde::{Deserialize, Serialize};
use serde_json::json;
use spool_core::EntityType;

use super::{Engine, Inner};
use crate::error::{Error, Result};
use crate::features;
use crate::project::{EntityTypeManifest, Manifest};
use crate::schema;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ApplyReport {
    /// Entity types declared by this call.
    pub declared: Vec<EntityType>,
    /// Entity types the store already held with the same declaration.
    pub unchanged: Vec<EntityType>,
    /// Entity types whose declaration changed. Their feature tables were
    /// computed again from the stored records.
    #[serde(default)]
    pub rebuilt: Vec<EntityType>,
}

impl Engine {
    /// Declares the manifest's entity types in the store, generating a
    /// feature table for each new one, and makes the manifest's rules the
    /// ones this engine runs. Applying the same manifest again changes
    /// nothing in the store.
    ///
    /// A type whose declaration differs from the one in the store gets a new
    /// declaration version, and its feature table is computed again from the
    /// stored records. Identities and the log are not touched. Match
    /// runs of the type that were waiting for judgements are failed, because
    /// their candidates were generated from the old features.
    pub fn apply(&self, manifest: &Manifest) -> Result<ApplyReport> {
        let mut inner = self.lock();
        let report = inner.apply(manifest)?;
        self.set_manifest(inner.manifest.clone());
        Ok(report)
    }

    /// Makes `manifest` the one this engine runs on without changing the
    /// store. Refused unless it is the manifest the store was last applied
    /// with: a server must not run rules against feature tables built for
    /// another declaration.
    pub fn adopt(&self, manifest: &Manifest) -> Result<()> {
        let mut inner = self.lock();
        let applied = schema::meta(&inner.store.conn, "manifest_hash")?;
        if applied.as_deref() != Some(manifest.hash.as_str()) {
            return Err(Error::ManifestMismatch {
                applied,
                manifest: manifest.hash.clone(),
            });
        }
        inner.manifest = Some(Arc::new(manifest.clone()));
        self.set_manifest(inner.manifest.clone());
        Ok(())
    }
}

impl Inner {
    fn apply(&mut self, manifest: &Manifest) -> Result<ApplyReport> {
        let now = self.store.clock.now_micros();
        let mut report = ApplyReport {
            declared: Vec::new(),
            unchanged: Vec::new(),
            rebuilt: Vec::new(),
        };
        let tx = self.store.conn.transaction()?;
        for entity_type in &manifest.entity_types {
            let name = &entity_type.name;
            let stored: Option<(i32, String)> = {
                let mut statement = tx.prepare(
                    "SELECT version, declaration_hash FROM spool.entity_type
                     WHERE entity_type = ? ORDER BY version DESC LIMIT 1",
                )?;
                let mut rows = statement.query([name.as_str()])?;
                match rows.next()? {
                    Some(row) => Some((row.get(0)?, row.get(1)?)),
                    None => None,
                }
            };
            let version = match stored {
                Some((_, hash)) if hash == entity_type.declaration_hash => {
                    report.unchanged.push(name.clone());
                    continue;
                }
                Some((version, _)) => version as u32 + 1,
                None => 1,
            };
            tx.execute(
                "INSERT INTO spool.entity_type VALUES (?, ?, ?, ?, make_timestamptz(?))",
                params![
                    name.as_str(),
                    version as i32,
                    json!(entity_type.declaration).to_string(),
                    entity_type.declaration_hash,
                    now
                ],
            )?;
            if version == 1 {
                features::create_table(&tx, name, &entity_type.declaration, version)?;
                report.declared.push(name.clone());
                continue;
            }

            // The declaration changed. The payloads are all still there, so
            // the feature table is computed again from them.
            features::drop_table(&tx, name)?;
            features::create_table(&tx, name, &entity_type.declaration, version)?;
            let problem = features::fill(
                &tx,
                name,
                &entity_type.declaration,
                version,
                &features::of_type(name),
            )?;
            if let Some(problem) = problem {
                return Err(Error::Apply(problem));
            }
            // Candidates of a run that waits for judgements were generated
            // from the old features, and its judgements would be about them.
            // That is a run a caller holds open, and a run whose user code
            // is at work: nothing else is unfinished while this call has the
            // writer.
            tx.execute(
                "UPDATE spool.run
                 SET status = 'failed', finished_at = make_timestamptz(?), error = ?
                 WHERE kind = 'match' AND status IN ('open', 'running')
                   AND json_extract_string(config, '$.entity_type') = ?",
                params![
                    now,
                    format!("the declaration of `{name}` changed while the run was open"),
                    name.as_str()
                ],
            )?;
            report.rebuilt.push(name.clone());
        }
        features::label_view(
            &tx,
            manifest
                .entity_types
                .iter()
                .map(|entity_type| (&entity_type.name, &entity_type.declaration)),
        )?;
        schema::set_meta(&tx, "manifest_hash", &manifest.hash)?;
        tx.commit()?;
        self.manifest = Some(Arc::new(manifest.clone()));
        Ok(report)
    }

    /// The applied manifest's entry for a type, with the version the store
    /// holds for it.
    pub(super) fn declared(
        &self,
        entity_type: &str,
    ) -> Result<(&Manifest, &EntityTypeManifest, u32)> {
        let manifest = self.manifest.as_deref().ok_or(Error::NoManifest)?;
        let entry = manifest
            .entity_type(entity_type)
            .ok_or_else(|| Error::UndeclaredType(entity_type.to_owned()))?;
        let version: Option<i32> = self.store.conn.query_row(
            "SELECT max(version) FROM spool.entity_type WHERE entity_type = ?",
            [entity_type],
            |row| row.get(0),
        )?;
        let version = version.ok_or_else(|| Error::UndeclaredType(entity_type.to_owned()))?;
        Ok((manifest, entry, version as u32))
    }
}
