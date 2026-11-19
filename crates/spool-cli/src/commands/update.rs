//! Update an installation made by the release installer.

use std::env;
use std::fs::{self, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anstream::println;
use clap::{Args, ColorChoice};
use semver::Version;
use serde::Deserialize;

use crate::Failure;
use crate::output::{hint, quiet};

const CURRENT: &str = env!("CARGO_PKG_VERSION");
const LATEST_RELEASE: &str = "https://api.github.com/repos/alexpatow/spool/releases/latest";
const INSTALLER: &str = include_str!("../../../../install.sh");

/// Update Spool itself and refresh the project's coding-agent skills
///
/// Checks for the latest stable GitHub release and updates an installation
/// made with install.sh. The installer verifies the archive and checks that
/// the new binary starts before switching the command to it.
///
/// The installed binary then refreshes the project's skills, keeping edits.
/// It does this even when Spool is already current. Outside a project, you
/// can give its directory or run this command to update just Spool.
#[derive(Args)]
pub struct UpdateArgs {
    /// The project to refresh [default: the current directory if it is a project]
    dir: Option<PathBuf>,
}

struct Installation {
    executable: PathBuf,
    link: PathBuf,
    root: PathBuf,
}

pub fn update(arguments: UpdateArgs, color: ColorChoice) -> Result<(), Failure> {
    let project = super::project_dir(&arguments.dir);
    let refresh = project.join("entities").is_dir();
    if arguments.dir.is_some() && !refresh {
        return Err(Failure::failed(format!(
            "{} has no entities/ folder. Give the path to a Spool project.",
            project.display()
        )));
    }

    let installed = Installation::find()?;
    let lock_path = installed.root.join(".update.lock");
    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|error| Failure::failed(format!("{}: {error}", lock_path.display())))?;
    lock.try_lock().map_err(|error| match error {
        TryLockError::WouldBlock => Failure::failed("another `spool update` is running"),
        TryLockError::Error(error) => Failure::failed(format!("{}: {error}", lock_path.display())),
    })?;
    // An earlier updater could have switched the link before we got the lock.
    let old_target = link_target(&installed.link, &installed.executable).ok_or_else(|| {
        Failure::failed("the installed command changed. Run `spool update` again.")
    })?;

    let latest = latest_version()?;
    let current = Version::parse(CURRENT).expect("Cargo's version is semver");
    if latest.cmp_precedence(&current).is_gt() {
        install(&installed, &old_target, &latest)?;
        println!("updated  spool {current} to {latest}");
    } else {
        println!("current  spool {current}");
    }

    if refresh {
        let mut command = Command::new(&installed.link);
        command
            .arg("--color")
            .arg(color.to_string())
            .arg("update-skills");
        if quiet() {
            command.arg("--quiet");
        }
        let failed = || {
            Failure::failed("Spool is current, but the project's skills could not be refreshed")
                .hint("run `spool update-skills` in the project to retry")
        };
        let status = command
            .arg("--")
            .arg(&project)
            .status()
            .map_err(|error| failed().and(error.to_string()))?;
        if !status.success() {
            return Err(failed());
        }
    } else {
        hint("Run `spool update-skills` in a project to refresh its skills.");
    }
    Ok(())
}

impl Installation {
    fn find() -> Result<Self, Failure> {
        let executable = env::current_exe()
            .and_then(fs::canonicalize)
            .map_err(|error| {
                Failure::failed(format!("could not locate the running spool: {error}"))
            })?;
        let unmanaged = || {
            Failure::failed("`spool update` needs an installation made with install.sh")
                .hint("update Spool using the method you installed it with, then run `spool update-skills`")
        };
        if !executable.ends_with(Path::new(CURRENT).join("bin/spool")) {
            return Err(unmanaged());
        }
        let root = executable
            .ancestors()
            .nth(3)
            .ok_or_else(unmanaged)?
            .to_owned();
        let link = find_link(&executable).ok_or_else(unmanaged)?;
        let link = std::path::absolute(&link)
            .map_err(|error| Failure::failed(format!("{}: {error}", link.display())))?;
        Ok(Self {
            executable,
            link,
            root,
        })
    }
}

fn find_link(executable: &Path) -> Option<PathBuf> {
    // Starting through the installer's link also identifies custom bin
    // directories when SPOOL_BIN_DIR is unset after installation.
    if let Some(invoked) = env::args_os().next().map(PathBuf::from) {
        if invoked == Path::new("spool") {
            if let Some(path) = env::var_os("PATH")
                && let Some(link) = env::split_paths(&path)
                    .map(|bin| bin.join("spool"))
                    .find(|link| link_target(link, executable).is_some())
            {
                return Some(link);
            }
        } else if invoked.file_name().is_some_and(|name| name == "spool")
            && link_target(&invoked, executable).is_some()
        {
            return Some(invoked);
        }
    }

    let bin = env::var_os("SPOOL_BIN_DIR")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/bin")))?;
    let link = bin.join("spool");
    link_target(&link, executable).map(|_| link)
}

/// The installer links directly to the version's binary. A link to that
/// link is an alias that belongs to whoever added it.
fn link_target(link: &Path, executable: &Path) -> Option<PathBuf> {
    let raw = fs::read_link(link).ok()?;
    let target = link.parent()?.join(&raw);
    let file = fs::symlink_metadata(&target).ok()?;
    (file.is_file() && fs::canonicalize(&target).ok()? == executable).then_some(raw)
}

fn latest_version() -> Result<Version, Failure> {
    #[derive(Deserialize)]
    struct Release {
        tag_name: String,
    }

    let response = Command::new("curl")
        .args([
            "--disable",
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--proto",
            "=https",
            "--tlsv1.2",
            "--max-time",
            "30",
            "--max-filesize",
            "1048576",
            "--user-agent",
            concat!("spool/", env!("CARGO_PKG_VERSION")),
            "--header",
            "Accept: application/vnd.github+json",
            LATEST_RELEASE,
        ])
        .output()
        .map_err(|error| {
            Failure::failed(format!("could not run curl to check for updates: {error}"))
        })?;
    if !response.status.success() {
        return Err(
            Failure::failed("could not check for the latest Spool release")
                .and(String::from_utf8_lossy(&response.stderr).trim()),
        );
    }
    let release: Release = serde_json::from_slice(&response.stdout).map_err(|error| {
        Failure::failed(format!("could not read the latest Spool release: {error}"))
    })?;
    release
        .tag_name
        .strip_prefix('v')
        .and_then(|tag| Version::parse(tag).ok())
        .filter(|version| version.pre.is_empty())
        .ok_or_else(|| {
            Failure::failed(format!(
                "the latest Spool release has an unexpected tag {:?}",
                release.tag_name
            ))
        })
}

fn install(installed: &Installation, old_target: &Path, version: &Version) -> Result<(), Failure> {
    let mut command = Command::new("sh");
    command
        .arg("-s")
        .env("SPOOL_VERSION", version.to_string())
        .env("SPOOL_INSTALL_DIR", &installed.root)
        .env(
            "SPOOL_BIN_DIR",
            installed.link.parent().expect("a link has a directory"),
        )
        .env("SPOOL_UPDATE_FROM", old_target)
        .env_remove("SPOOL_DOWNLOAD_URL")
        .stdin(Stdio::piped());
    if quiet() {
        command.stdout(Stdio::null());
    }
    let failed = |error| Failure::failed(format!("could not run the Spool installer: {error}"));
    let mut child = command.spawn().map_err(failed)?;
    let wrote = child
        .stdin
        .take()
        .expect("the shell has piped input")
        .write_all(INSTALLER.as_bytes());
    let status = child.wait().map_err(failed)?;
    if !status.success() {
        return Err(Failure::failed(format!(
            "could not install Spool {version}"
        )));
    }
    wrote.map_err(failed)
}
