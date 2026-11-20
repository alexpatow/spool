//! Reading the files and folders that were named, each file as a
//! document under its key.

use std::collections::HashMap;
use std::path::Path;

use super::File;
use crate::Failure;

/// Reads the files the paths name. A file is one document, stored under its
/// name, and a folder stands for the files under it, each stored under its
/// path from the folder.
pub(super) fn read_files(paths: &[String]) -> Result<Vec<File>, Failure> {
    let mut found: Vec<(String, String)> = Vec::new();
    for path in paths {
        let metadata =
            std::fs::metadata(path).map_err(|error| Failure::failed(format!("{path}: {error}")))?;
        if metadata.is_dir() {
            let before = found.len();
            collect_files_under(Path::new(path), "", &mut found)?;
            if found.len() == before {
                return Err(Failure::failed(format!(
                    "{path} is a folder with no files in it, other than ones whose names start \
                     with a dot"
                )));
            }
            continue;
        }
        let name = Path::new(path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .ok_or_else(|| Failure::failed(format!("{path} does not name a file")))?;
        found.push((path.clone(), name));
    }

    // A second document under the same key would be taken for a new
    // version of the first.
    let mut keys: HashMap<&str, &str> = HashMap::new();
    for (path, name) in &found {
        if let Some(first) = keys.insert(name, path) {
            return Err(Failure::failed(format!(
                "{first} and {path} would both be stored as `{name}`: a file is stored under its \
                 name, or under its path from the folder that was named, and a second document \
                 under one key is taken for a new version of the first. Name the folder that \
                 holds both, or extract them into sources of their own."
            )));
        }
    }

    let mut files = Vec::with_capacity(found.len());
    for (path, name) in found {
        let bytes =
            std::fs::read(&path).map_err(|error| Failure::failed(format!("{path}: {error}")))?;
        let text = String::from_utf8(bytes).map_err(|_| {
            Failure::failed(format!(
                "{path} is not text: a document is stored as UTF-8 text. Turn a PDF or a web \
                 page into text first."
            ))
        })?;
        if text.trim().is_empty() {
            return Err(Failure::failed(format!("{path} is empty")));
        }
        files.push(File { path, name, text });
    }
    Ok(files)
}

/// Adds the files under `folder` to `found`, in the order of their names,
/// each with its key: its path from the folder that was named, after
/// `prefix`. What starts with a dot is left out. A link to a file is read
/// and a link to a folder is not followed, since it can lead back up.
fn collect_files_under(
    folder: &Path,
    prefix: &str,
    found: &mut Vec<(String, String)>,
) -> Result<(), Failure> {
    let unreadable =
        |error: std::io::Error| Failure::failed(format!("{}: {error}", folder.display()));
    let mut entries = std::fs::read_dir(folder)
        .and_then(|entries| entries.collect::<Result<Vec<_>, _>>())
        .map_err(unreadable)?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let key = match prefix {
            "" => name,
            prefix => format!("{prefix}/{name}"),
        };
        let kind = entry.file_type().map_err(unreadable)?;
        let is_file = if kind.is_dir() {
            collect_files_under(&path, &key, found)?;
            false
        } else if kind.is_symlink() {
            std::fs::metadata(&path)
                .map_err(|error| Failure::failed(format!("{}: {error}", path.display())))?
                .is_file()
        } else {
            kind.is_file()
        };
        if is_file {
            found.push((path.display().to_string(), key));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{File, read_files};

    /// The files the paths name, which have to be read.
    fn read_all(paths: &[String]) -> Vec<File> {
        read_files(paths).unwrap_or_else(|failure| panic!("{}", failure.message()))
    }

    #[test]
    fn a_folder_is_its_files_by_their_paths_from_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("documents");
        let write = |path: &str, text: &str| {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write("train/b.txt", "B");
        write("dev/a.txt", "A");
        write("dev/deeper/c.md", "C");
        write(".git/HEAD", "ref");
        write("dev/.DS_Store", "\0");
        let elsewhere = dir.path().join("elsewhere.txt");
        std::fs::write(&elsewhere, "E").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&elsewhere, root.join("linked.txt")).unwrap();
            // A link to a folder above would lead round and round.
            std::os::unix::fs::symlink(dir.path(), root.join("dev/up")).unwrap();
        }

        let files = read_all(&[root.display().to_string()]);
        let keys: Vec<&str> = files.iter().map(|file| file.name.as_str()).collect();
        #[cfg(unix)]
        assert_eq!(
            keys,
            ["dev/a.txt", "dev/deeper/c.md", "linked.txt", "train/b.txt"]
        );
        assert_eq!(files[0].text, "A");
        assert_eq!(files[0].path, root.join("dev/a.txt").display().to_string());

        // A file named on its own is stored under its name.
        let files = read_all(&[root.join("dev/a.txt").display().to_string()]);
        assert_eq!(files[0].name, "a.txt");
        // A hidden file that is named is read.
        let files = read_all(&[root.join(".git/HEAD").display().to_string()]);
        assert_eq!(files[0].name, "HEAD");
    }
}
