//! A self-update from a release served by a stand-in for curl.

#![cfg(any(target_os = "linux", all(target_os = "macos", target_arch = "aarch64")))]

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
use std::process::Command;

use semver::{BuildMetadata, Prerelease, Version};
use sha2::{Digest, Sha256};

const SPOOL: &str = env!("CARGO_BIN_EXE_spool");
const CURRENT: &str = env!("CARGO_PKG_VERSION");

fn executable(path: &Path, script: &str) {
    fs::write(path, script).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn an_unmanaged_binary_leaves_its_installation_alone() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(SPOOL)
        .arg("update")
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("needs an installation made with install.sh")
    );
}

#[test]
#[expect(clippy::too_many_lines)]
fn updates_the_installation_then_refreshes_skills_with_the_installed_binary() {
    let dir = tempfile::tempdir().unwrap();
    let release_root = dir.path().join("releases");
    let old = release_root.join(CURRENT).join("bin/spool");
    fs::create_dir_all(old.parent().unwrap()).unwrap();
    fs::copy(SPOOL, &old).unwrap();
    // Both paths are custom. They can be recovered from the command even
    // when the installer environment is gone.
    let bin = dir.path().join("commands");
    fs::create_dir(&bin).unwrap();
    let link = bin.join("spool");
    symlink(&old, &link).unwrap();

    let project = dir.path().join("project");
    fs::create_dir_all(project.join("entities")).unwrap();
    let rules = project.join(".agents/skills/spool-rules/SKILL.md");
    let mock = dir.path().join("mock");
    fs::create_dir(&mock).unwrap();
    let path = std::env::join_paths(
        [mock.clone(), bin.clone()]
            .into_iter()
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    executable(
        &mock.join("curl"),
        r#"#!/bin/sh
set -eu
output=
while [ "$#" -gt 1 ]; do
    if [ "$1" = --output ]; then output=$2; shift; fi
    shift
done
case "$1" in
    https://api.github.com/repos/alexpatow/spool/releases/latest)
        cat "$FIXTURE/release.json" ;;
    "https://github.com/alexpatow/spool/releases/download/v$FIXTURE_VERSION/"*)
        cp "$FIXTURE/${1##*/}" "$output" ;;
    *) echo "unexpected curl URL $1" >&2; exit 1 ;;
esac
"#,
    );
    let current_version = Version::parse(CURRENT).unwrap();
    let mut latest = current_version.clone();
    latest.patch += 1;
    latest.pre = Prerelease::EMPTY;
    latest.build = BuildMetadata::EMPTY;
    let version = latest.to_string();
    let target = if cfg!(target_os = "macos") {
        "aarch64-apple-darwin".to_owned()
    } else {
        format!("{}-unknown-linux-gnu", std::env::consts::ARCH)
    };
    let name = format!("spool-{version}-{target}");
    let unpacked = mock.join(&name);
    fs::create_dir_all(unpacked.join("bin")).unwrap();
    fs::create_dir(unpacked.join("lib")).unwrap();
    fs::create_dir_all(unpacked.join("share/man/man1")).unwrap();
    fs::write(unpacked.join("share/man/man1/spool.1"), "mock manual\n").unwrap();
    executable(
        &unpacked.join("bin/spool"),
        r#"#!/bin/sh
set -eu
if [ "$1" = --version ]; then printf 'spool %s\n' "$FIXTURE_VERSION"; exit 0; fi
printf '%s\n' "$0" > "$FIXTURE/refreshed-by"
exec "$FIXTURE_SPOOL" "$@"
"#,
    );
    let archive = format!("{name}.tar.gz");
    assert!(
        Command::new("tar")
            .current_dir(&mock)
            .args(["-czf", &archive, &name])
            .status()
            .unwrap()
            .success()
    );
    let set_latest = |tag: &str| {
        fs::write(
            mock.join("release.json"),
            format!(r#"{{"tag_name":"v{tag}"}}"#),
        )
        .unwrap();
    };
    let update = || {
        Command::new("spool")
            .args(["update", "--quiet"])
            .current_dir(&project)
            .env("PATH", &path)
            .env("HOME", dir.path())
            .env("FIXTURE", &mock)
            .env("FIXTURE_VERSION", &version)
            .env("FIXTURE_SPOOL", SPOOL)
            .env_remove("SPOOL_BIN_DIR")
            .env_remove("SPOOL_INSTALL_DIR")
            .output()
            .unwrap()
    };

    // A current installation refreshes skills without fetching an archive.
    set_latest(if current_version.pre.is_empty() {
        CURRENT
    } else {
        "0.0.0"
    });
    let current = update();
    assert!(
        current.status.success(),
        "{}",
        current.stderr.escape_ascii()
    );
    assert!(rules.is_file());
    assert_eq!(fs::read_link(&link).unwrap(), old);
    fs::remove_file(&rules).unwrap();

    // A failed verification leaves both the previous command and skills alone.
    set_latest(&version);
    let sums = mock.join("SHA256SUMS");
    fs::write(&sums, format!("wrong  {archive}\n")).unwrap();
    let failed = update();
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("is not the file the release lists"));
    assert_eq!(fs::read_link(&link).unwrap(), old);
    assert!(!rules.exists());

    let digest = hex::encode(Sha256::digest(fs::read(mock.join(&archive)).unwrap()));
    fs::write(&sums, format!("{digest}  {archive}\n")).unwrap();
    let applied = update();
    assert!(
        applied.status.success(),
        "{}",
        applied.stderr.escape_ascii()
    );
    let new = release_root.join(&version).join("bin/spool");
    assert_eq!(
        fs::canonicalize(&link).unwrap(),
        fs::canonicalize(&new).unwrap()
    );
    assert!(old.is_file());
    assert!(rules.is_file());
    let refreshed_by = fs::read_to_string(mock.join("refreshed-by")).unwrap();
    assert_eq!(
        fs::canonicalize(refreshed_by.trim_end()).unwrap(),
        fs::canonicalize(new).unwrap()
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("share/man/man1/spool.1")).unwrap(),
        "mock manual\n"
    );
}
