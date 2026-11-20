//! Hashes: of a strategy's files, of texts, and of a value's JSON form.

use std::path::Path;

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{file_name, format_relative_path, list_dir_entries};
use crate::error::{Error, Result};

/// SHA-256 of a strategy: its file, and every file in the directory beside
/// it that shares its name, which is where a strategy keeps what belongs to
/// it. What tools leave behind (`__pycache__`, `node_modules`, hidden files)
/// is not part of it.
pub(crate) fn strategy_hash(file: &Path) -> Result<String> {
    fn hash_file(hasher: &mut Sha256, name: &str, path: &Path) -> Result<()> {
        let contents = std::fs::read(path).map_err(Error::io(path))?;
        hasher.update((name.len() as u64).to_le_bytes());
        hasher.update(name.as_bytes());
        hasher.update((contents.len() as u64).to_le_bytes());
        hasher.update(&contents);
        Ok(())
    }
    fn hash_dir(hasher: &mut Sha256, root: &Path, dir: &Path) -> Result<()> {
        for entry in list_dir_entries(dir)? {
            let name = file_name(&entry);
            if entry.is_dir() {
                if name != "__pycache__" && name != "node_modules" {
                    hash_dir(hasher, root, &entry)?;
                }
            } else {
                hash_file(hasher, &format_relative_path(root, &entry), &entry)?;
            }
        }
        Ok(())
    }
    let mut hasher = Sha256::new();
    hash_file(&mut hasher, &file_name(file), file)?;
    let owned = file.with_extension("");
    if owned.is_dir() {
        hash_dir(&mut hasher, &owned, &owned)?;
    }
    Ok(hex::encode(hasher.finalize()))
}

/// SHA-256 of several texts, each with its length, as lowercase hex.
pub(crate) fn hash_text(texts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for text in texts {
        hasher.update((text.len() as u64).to_le_bytes());
        hasher.update(text.as_bytes());
    }
    hex::encode(hasher.finalize())
}

/// SHA-256 of a value's JSON form, as lowercase hex.
pub(crate) fn hash_json(value: &impl Serialize) -> String {
    let json = serde_json::to_vec(value).expect("manifest types serialise to JSON");
    hex::encode(Sha256::digest(&json))
}
