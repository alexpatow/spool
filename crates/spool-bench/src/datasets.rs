//! Public datasets: finding one in `datasets.toml`, downloading it,
//! checking it against its checksum and unpacking it.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

/// An entry of `datasets.toml`: a public dataset that is downloaded and
/// checked when a scenario needs it, and never committed.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Dataset {
    name: String,
    url: String,
    sha256: String,
    #[allow(dead_code)]
    licence: String,
    /// `tar` when the download is a tar archive, compressed or not, of
    /// which an ingest names one file. Without it the download is the file.
    archive: Option<Archive>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Archive {
    Tar,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Datasets {
    dataset: Vec<Dataset>,
}

/// Returns the local copy of a dataset named in the nearest `datasets.toml`
/// above the scenario, downloading it if the copy is missing or does not
/// match its checksum. Copies are kept in `.cache` beside `datasets.toml`.
///
/// A dataset that is an archive is unpacked beside its download, once, and
/// `member` names the file in it that is wanted.
pub(crate) fn fetch_dataset(
    scenario_dir: &Path,
    name: &str,
    member: Option<&str>,
) -> Result<PathBuf> {
    let (index, dataset) = find_dataset(scenario_dir, name, member)?;
    let cache = index.with_file_name(".cache");
    let Some(member) = member else {
        let path = cache.join(format!("{name}.csv"));
        if !(path.is_file() && sha256_of(&path)? == dataset.sha256) {
            download(&cache, name, &dataset, &path)?;
        }
        return Ok(path);
    };
    unpacked(&cache, name, &dataset, member)
}

/// Finds a dataset in the `datasets.toml` nearest above the scenario, and
/// that file. An ingest names a `member` of an archive, and of nothing else.
pub(crate) fn find_dataset(
    scenario_dir: &Path,
    name: &str,
    member: Option<&str>,
) -> Result<(PathBuf, Dataset)> {
    let start = std::fs::canonicalize(scenario_dir).map_err(Error::io(scenario_dir))?;
    let index = start
        .ancestors()
        .map(|dir| dir.join("datasets.toml"))
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| {
            Error::Scenario(format!(
                "dataset `{name}`: no datasets.toml in {} or above it",
                start.display()
            ))
        })?;
    let text = std::fs::read_to_string(&index).map_err(Error::io(&index))?;
    let datasets: Datasets = toml::from_str(&text)
        .map_err(|error| Error::Scenario(format!("{}: {error}", index.display())))?;
    let dataset = datasets
        .dataset
        .into_iter()
        .find(|dataset| dataset.name == name)
        .ok_or_else(|| {
            Error::Scenario(format!("{}: no dataset named `{name}`", index.display()))
        })?;
    match (dataset.archive, member) {
        (None, None) | (Some(Archive::Tar), Some(_)) => Ok((index, dataset)),
        (None, Some(_)) => Err(Error::Scenario(format!(
            "dataset `{name}` is one file, and the ingest names a `member` of it"
        ))),
        (Some(Archive::Tar), None) => Err(Error::Scenario(format!(
            "dataset `{name}` is an archive: the ingest names the file to read with `member`"
        ))),
    }
}

/// A path in the cache that no other run uses: two runs that fetch the same
/// dataset at once each write their own file, and the one that is whole
/// takes the dataset's name.
fn scratch(cache: &Path, name: &str, kind: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    cache.join(format!(
        "{name}.{}-{}.{kind}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Downloads a dataset to `path`, which is kept only if it has the checksum
/// `datasets.toml` names.
fn download(cache: &Path, name: &str, dataset: &Dataset, path: &Path) -> Result<()> {
    std::fs::create_dir_all(cache).map_err(Error::io(cache))?;
    let partial = scratch(cache, name, "part");
    let status = Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--output",
        ])
        .arg(&partial)
        .arg(&dataset.url)
        .status()
        .map_err(|error| {
            Error::Scenario(format!(
                "dataset `{name}`: curl is needed to download it and could not be run: {error}"
            ))
        })?;
    if !status.success() {
        let _ = std::fs::remove_file(&partial);
        return Err(Error::Scenario(format!(
            "dataset `{name}`: downloading {} failed",
            dataset.url
        )));
    }
    let found = sha256_of(&partial)?;
    if found != dataset.sha256 {
        let _ = std::fs::remove_file(&partial);
        return Err(Error::Scenario(format!(
            "dataset `{name}`: the download has SHA-256 {found}, and datasets.toml expects {}",
            dataset.sha256
        )));
    }
    std::fs::rename(&partial, path).map_err(Error::io(path))
}

/// The file `member` of a dataset that is an archive. The archive is
/// downloaded, checked and unpacked into a directory named for the dataset,
/// and only the directory is kept. It records the checksum of the archive it
/// came from, so a dataset whose checksum changed is fetched again.
fn unpacked(cache: &Path, name: &str, dataset: &Dataset, member: &str) -> Result<PathBuf> {
    let inside = Path::new(member);
    let plain = inside
        .components()
        .all(|part| matches!(part, std::path::Component::Normal(_)));
    if !plain || member.is_empty() {
        return Err(Error::Scenario(format!(
            "dataset `{name}`: `{member}` does not name a file within the archive"
        )));
    }
    let dir = cache.join(name);
    let current = |dir: &Path| {
        std::fs::read_to_string(dir.join(".sha256")).is_ok_and(|kept| kept == dataset.sha256)
    };
    if !current(&dir) {
        let archive = scratch(cache, name, "tar");
        download(cache, name, dataset, &archive)?;
        // Unpacked beside where it goes and then given its name, so that
        // nobody reads a directory that is half there.
        let unpacking = scratch(cache, name, "unpacking");
        let unpack = || -> Result<()> {
            std::fs::create_dir_all(&unpacking).map_err(Error::io(&unpacking))?;
            let status = Command::new("tar")
                .arg("-xf")
                .arg(&archive)
                .arg("-C")
                .arg(&unpacking)
                .status()
                .map_err(|error| {
                    Error::Scenario(format!(
                        "dataset `{name}`: tar is needed to unpack it and could not be run: {error}"
                    ))
                })?;
            if !status.success() {
                return Err(Error::Scenario(format!(
                    "dataset `{name}`: the download could not be unpacked with tar"
                )));
            }
            let stamp = unpacking.join(".sha256");
            std::fs::write(&stamp, &dataset.sha256).map_err(Error::io(&stamp))?;
            if dir.exists() && !current(&dir) {
                std::fs::remove_dir_all(&dir).map_err(Error::io(&dir))?;
            }
            match std::fs::rename(&unpacking, &dir) {
                // Another run put the same files there first.
                Err(_) if current(&dir) => Ok(()),
                renamed => renamed.map_err(Error::io(&dir)),
            }
        };
        let unpacked = unpack();
        let _ = std::fs::remove_file(&archive);
        if unpacking.exists() {
            let _ = std::fs::remove_dir_all(&unpacking);
        }
        unpacked?;
    }
    let file = dir.join(inside);
    if !file.is_file() {
        return Err(Error::Scenario(format!(
            "dataset `{name}`: the archive has no file `{member}`"
        )));
    }
    Ok(file)
}

fn sha256_of(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path).map_err(Error::io(path))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(Error::io(path))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}
