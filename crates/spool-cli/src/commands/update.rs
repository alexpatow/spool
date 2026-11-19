//! The files a coding agent reads in a project, and `spool update`, which
//! brings them to this binary's.
//!
//! Each skill is `.agents/skills/<skill>/SKILL.md`, the folder Codex and
//! Cursor read, with a link to its folder from `.claude/skills/`, the one
//! Claude Code reads. A link and not a copy, so that Cursor, which reads
//! both, lists each skill once. `AGENTS.md` says what an agent should know
//! of any Spool project and lists the skills, and `CLAUDE.md` imports it for
//! a Claude Code that does not read `AGENTS.md` by itself.
//!
//! What Spool writes carries a hash of itself, so that `spool update` can
//! tell a file nobody edited, which it may replace, from one somebody did,
//! which it keeps.

use std::path::{Path, PathBuf};

use anstream::println;
use clap::Args;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::project::{failed, write};
use crate::Failure;
use crate::output::{Output, OutputArgs, hint};
use crate::style::{self, paint};

/// Update the skills and AGENTS.md that tell coding agents how to use Spool
///
/// Writes the files for coding agents that `spool init` writes, as this
/// version of spool has them: a skill for each part of the work in
/// .agents/skills/, which Codex and Cursor read, a link to each from
/// .claude/skills/, which Claude Code reads, Spool's part of AGENTS.md, and a
/// CLAUDE.md that imports AGENTS.md where there is none.
///
/// A file somebody edited since Spool wrote it is kept, and the command says
/// so; --force replaces it. Spool's part of AGENTS.md is between its two
/// marker lines, and the rest of the file is never changed. A skill of
/// Spool's that this version no longer has is removed, unless it was edited.
///
/// It does not update spool itself: the install script does.
#[derive(Args)]
pub struct UpdateArgs {
    /// The project directory [default: the current directory]
    dir: Option<PathBuf>,
    /// Replace the files somebody edited as well
    #[arg(long)]
    force: bool,
    #[command(flatten)]
    output: OutputArgs,
}

/// The skills, each by its name, which is its folder's.
pub const SKILLS: [(&str, &str); 5] = [
    (
        "spool-project",
        include_str!("../agents/skills/spool-project/SKILL.md"),
    ),
    (
        "spool-rules",
        include_str!("../agents/skills/spool-rules/SKILL.md"),
    ),
    (
        "spool-extractor",
        include_str!("../agents/skills/spool-extractor/SKILL.md"),
    ),
    (
        "spool-data",
        include_str!("../agents/skills/spool-data/SKILL.md"),
    ),
    (
        "spool-api",
        include_str!("../agents/skills/spool-api/SKILL.md"),
    ),
];

/// What Spool says in AGENTS.md, between the two marker lines.
pub const AGENTS: &str = include_str!("../agents/AGENTS.md");

const AGENTS_FILE: &str = "AGENTS.md";
const CLAUDE_FILE: &str = "CLAUDE.md";
const CLAUDE_IMPORT: &str = "@AGENTS.md";
const SKILLS_DIR: &str = ".agents/skills";
const LINKS_DIR: &str = ".claude/skills";
/// The skills that are Spool's have names that begin with this. A skill of
/// another name in the same folder is somebody else's.
const PREFIX: &str = "spool-";

/// Where the hash goes, in a skill's frontmatter and on the first marker line.
const STAMP: &str = "spool-sha256: \"";
const BEGIN: &str = "<!-- spool:begin. `spool update` rewrites what is between this line and \
                     spool:end while nobody has edited it. ";
const END: &str = "<!-- spool:end -->";

/// What became of one file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Added,
    Updated,
    Unchanged,
    /// Left as it is, though it is not this version's: [`Change::note`] says why.
    Kept,
    Removed,
}

impl Status {
    fn word(self) -> &'static str {
        match self {
            Status::Added => "added",
            Status::Updated => "updated",
            Status::Unchanged => "unchanged",
            Status::Kept => "kept",
            Status::Removed => "removed",
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Change {
    pub path: String,
    pub status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

fn change(path: impl Into<String>, status: Status) -> Change {
    Change {
        path: path.into(),
        status,
        note: None,
    }
}

fn kept(path: impl Into<String>, note: impl Into<String>) -> Change {
    Change {
        path: path.into(),
        status: Status::Kept,
        note: Some(note.into()),
    }
}

const EDITED: &str = "edited since Spool wrote it: --force replaces it";

pub fn update(arguments: UpdateArgs) -> Result<(), Failure> {
    let out = Output::report(&arguments.output, "`spool update`")?;
    let dir = super::project_dir(&arguments.dir);
    if !dir.join("entities").is_dir() {
        let which = match arguments.dir.is_some() {
            true => dir.display().to_string(),
            false => "this folder".to_owned(),
        };
        return Err(failed(format!(
            "{which} is not a Spool project: it has no entities/ folder"
        ))
        .hint("`spool init` makes a project, with these files in it"));
    }
    let changes = write_agent_files(&dir, arguments.force)?;
    if !out.is_table() {
        out.json(&serde_json::json!({ "project": dir, "files": changes }));
        return Ok(());
    }
    println!("project  {}\n", dir.display());
    let width = changes.iter().map(|c| c.path.len()).max().unwrap_or(0) + 2;
    for change in &changes {
        let word = change.status.word();
        let painted = match change.status {
            Status::Kept => paint(style::WAITING, word),
            Status::Unchanged => word.to_owned(),
            _ => paint(style::GOOD, word),
        };
        let pad = " ".repeat(11 - word.len());
        match &change.note {
            Some(note) => println!("  {painted}{pad}{:<width$}{note}", change.path),
            None => println!("  {painted}{pad}{}", change.path),
        }
    }
    if changes.iter().all(|c| c.status == Status::Unchanged) {
        hint("\nThe files for agents are this version's already.");
    }
    Ok(())
}

/// Writes the files for agents into the project at `dir`, or brings them to
/// this version's, and says what became of each. `force` replaces what
/// somebody edited.
pub fn write_agent_files(dir: &Path, force: bool) -> Result<Vec<Change>, Failure> {
    let mut changes = Vec::new();
    changes.push(agents_md(dir, force)?);
    changes.push(claude_md(dir)?);
    for (name, source) in SKILLS {
        let path = format!("{SKILLS_DIR}/{name}/SKILL.md");
        changes.push(owned_file(dir, &path, &stamp_skill(source), force)?);
        changes.push(link(dir, name, force)?);
    }
    changes.extend(remove_old_skills(dir)?);
    Ok(changes)
}

fn agents_md(dir: &Path, force: bool) -> Result<Change, Failure> {
    let path = dir.join(AGENTS_FILE);
    let section = agents_section();
    let Some(held) = read(&path)? else {
        write(&path, &format!("{section}\n"))?;
        return Ok(change(AGENTS_FILE, Status::Added));
    };
    let Some((start, end)) = find_section(&held) else {
        let mut appended = held.clone();
        if !appended.is_empty() && !appended.ends_with('\n') {
            appended.push('\n');
        }
        if !appended.is_empty() {
            appended.push('\n');
        }
        appended.push_str(&section);
        appended.push('\n');
        write(&path, &appended)?;
        return Ok(Change {
            path: AGENTS_FILE.to_owned(),
            status: Status::Updated,
            note: Some("Spool's part added after what was there".to_owned()),
        });
    };
    let current = &held[start..end];
    if current == section {
        return Ok(change(AGENTS_FILE, Status::Unchanged));
    }
    if !force && !unedited(current) {
        return Ok(kept(
            AGENTS_FILE,
            "Spool's part was edited since Spool wrote it: --force replaces that part",
        ));
    }
    write(
        &path,
        &format!("{}{section}{}", &held[..start], &held[end..]),
    )?;
    Ok(change(AGENTS_FILE, Status::Updated))
}

fn claude_md(dir: &Path) -> Result<Change, Failure> {
    let path = dir.join(CLAUDE_FILE);
    match read(&path)? {
        None => {
            write(&path, &format!("{CLAUDE_IMPORT}\n"))?;
            Ok(change(CLAUDE_FILE, Status::Added))
        }
        Some(held) if held.lines().any(|line| line.trim() == CLAUDE_IMPORT) => {
            Ok(change(CLAUDE_FILE, Status::Unchanged))
        }
        Some(_) => Ok(kept(
            CLAUDE_FILE,
            "it does not import AGENTS.md: add a line `@AGENTS.md` for Claude Code to read it",
        )),
    }
}

/// A file that is Spool's as a whole: written if it is not there, replaced
/// if nobody edited it, and kept if somebody did.
fn owned_file(dir: &Path, path: &str, contents: &str, force: bool) -> Result<Change, Failure> {
    let full = dir.join(path);
    let status = match read(&full)? {
        None => Status::Added,
        Some(held) if held == contents => return Ok(change(path, Status::Unchanged)),
        Some(held) if force || unedited(&held) => Status::Updated,
        Some(_) => return Ok(kept(path, EDITED)),
    };
    write(&full, contents)?;
    Ok(change(path, status))
}

/// The link from `.claude/skills/<name>` to the skill's folder.
fn link(dir: &Path, name: &str, force: bool) -> Result<Change, Failure> {
    let path = format!("{LINKS_DIR}/{name}");
    let full = dir.join(&path);
    let target = PathBuf::from(format!("../../{SKILLS_DIR}/{name}"));
    let status = match std::fs::symlink_metadata(&full) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Status::Added,
        Err(error) => return Err(failed(format!("{}: {error}", full.display()))),
        Ok(found) if found.file_type().is_symlink() => {
            let points = std::fs::read_link(&full)
                .map_err(|error| failed(format!("{}: {error}", full.display())))?;
            if points == target {
                return Ok(change(path, Status::Unchanged));
            }
            if !force {
                return Ok(kept(
                    path,
                    format!(
                        "a link to {}, not to {SKILLS_DIR}/{name}: --force replaces it",
                        points.display()
                    ),
                ));
            }
            remove(&full, false)?;
            Status::Updated
        }
        Ok(_) => {
            return Ok(kept(
                path,
                format!("not a link to {SKILLS_DIR}/{name}: remove it, and run this again"),
            ));
        }
    };
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| failed(format!("{}: {error}", parent.display())))?;
    }
    symlink(&target, &full)?;
    Ok(change(path, status))
}

#[cfg(unix)]
fn symlink(target: &Path, at: &Path) -> Result<(), Failure> {
    std::os::unix::fs::symlink(target, at)
        .map_err(|error| failed(format!("{}: {error}", at.display())))
}

#[cfg(not(unix))]
fn symlink(_: &Path, at: &Path) -> Result<(), Failure> {
    Err(failed(format!(
        "{}: links are made on Linux and macOS only",
        at.display()
    )))
}

/// Removes the skills of Spool's that this version does not have, and their
/// links, unless somebody edited them.
fn remove_old_skills(dir: &Path) -> Result<Vec<Change>, Failure> {
    let skills = dir.join(SKILLS_DIR);
    let Ok(entries) = std::fs::read_dir(&skills) else {
        return Ok(Vec::new());
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| Some(entry.ok()?.file_name().to_string_lossy().into_owned()))
        .filter(|name| name.starts_with(PREFIX))
        .filter(|name| SKILLS.iter().all(|(skill, _)| skill != name))
        .collect();
    names.sort();
    let mut changes = Vec::new();
    for name in names {
        let path = format!("{SKILLS_DIR}/{name}/SKILL.md");
        let folder = skills.join(&name);
        let only_the_skill = std::fs::read_dir(&folder)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .all(|entry| entry.file_name() == "SKILL.md")
            })
            .unwrap_or(false);
        match read(&folder.join("SKILL.md"))? {
            Some(held) if only_the_skill && unedited(&held) => {
                remove(&folder.join("SKILL.md"), false)?;
                remove(&folder, true)?;
                changes.push(change(path, Status::Removed));
                let link = dir.join(LINKS_DIR).join(&name);
                let ours = std::fs::read_link(&link)
                    .is_ok_and(|points| points == Path::new(&format!("../../{SKILLS_DIR}/{name}")));
                if ours {
                    remove(&link, false)?;
                    changes.push(change(format!("{LINKS_DIR}/{name}"), Status::Removed));
                }
            }
            Some(_) => changes.push(kept(
                path,
                "this version of Spool has no such skill, and it was edited: remove it if it is \
                 no longer wanted",
            )),
            None => {}
        }
    }
    Ok(changes)
}

fn read(path: &Path) -> Result<Option<String>, Failure> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(failed(format!("{}: {error}", path.display()))),
    }
}

fn remove(path: &Path, folder: bool) -> Result<(), Failure> {
    let removed = match folder {
        true => std::fs::remove_dir(path),
        false => std::fs::remove_file(path),
    };
    removed.map_err(|error| failed(format!("{}: {error}", path.display())))
}

/// A skill as it is written: with the hash of itself in its frontmatter.
pub fn stamp_skill(source: &str) -> String {
    let with = |hash: &str| {
        let body = source
            .strip_prefix("---\n")
            .expect("a skill begins with its frontmatter");
        let (front, rest) = body
            .split_once("\n---\n")
            .expect("a skill's frontmatter ends");
        format!("---\n{front}\nmetadata:\n  {STAMP}{hash}\"\n---\n{rest}")
    };
    with(&digest(&with("")))
}

/// Spool's part of AGENTS.md, with its marker lines and the hash of itself.
pub fn agents_section() -> String {
    let with = |hash: &str| {
        format!(
            "{BEGIN}{STAMP}{hash}\" -->\n{}{END}",
            AGENTS.trim_end().to_owned() + "\n"
        )
    };
    with(&digest(&with("")))
}

/// Where Spool's part of AGENTS.md is in `text`: from the start of its first
/// marker line to the end of its last.
fn find_section(text: &str) -> Option<(usize, usize)> {
    let start = text.find("<!-- spool:begin")?;
    let end = start + text[start..].find(END)? + END.len();
    Some((start, end))
}

/// Whether `text` is as Spool wrote it: the hash it carries is its own.
fn unedited(text: &str) -> bool {
    let Some(at) = text.find(STAMP) else {
        return false;
    };
    let from = at + STAMP.len();
    let Some(length) = text[from..].find('"') else {
        return false;
    };
    let hash = &text[from..from + length];
    let blank = format!("{}{}", &text[..from], &text[from + length..]);
    !hash.is_empty() && digest(&blank) == hash
}

/// The first 16 hexadecimal digits of the SHA-256 of `text`: enough to tell
/// an edit, which is all it is for.
fn digest(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stamped_file_is_unedited_and_an_edit_shows() {
        for (_, source) in SKILLS {
            let stamped = stamp_skill(source);
            assert!(unedited(&stamped));
            assert!(!unedited(&stamped.replace("spool", "Spool")));
            assert!(!unedited(source));
        }
        let section = agents_section();
        assert!(unedited(&section));
        assert!(!unedited(&section.replace("Spool", "spool")));
        assert_eq!(
            find_section(&format!("mine\n\n{section}\nmore")).map(|(s, e)| e - s),
            Some(section.len())
        );
    }

    fn written(dir: &Path, force: bool) -> Vec<Change> {
        write_agent_files(dir, force).unwrap_or_else(|failure| panic!("{}", failure.message()))
    }

    fn statuses(changes: &[Change]) -> Vec<(String, Status)> {
        changes
            .iter()
            .map(|change| (change.path.clone(), change.status))
            .collect()
    }

    fn status_of(changes: &[Change], path: &str) -> Status {
        changes
            .iter()
            .find(|change| change.path == path)
            .unwrap_or_else(|| panic!("nothing about {path}"))
            .status
    }

    #[test]
    fn a_project_gets_every_file_and_a_second_time_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let first = written(dir.path(), false);
        assert!(
            first.iter().all(|change| change.status == Status::Added),
            "{first:?}"
        );
        assert_eq!(first.len(), 2 + 2 * SKILLS.len());
        for (name, source) in SKILLS {
            let through_link = dir.path().join(LINKS_DIR).join(name).join("SKILL.md");
            assert_eq!(
                std::fs::read_to_string(through_link).unwrap(),
                stamp_skill(source)
            );
        }
        assert_eq!(
            std::fs::read_to_string(dir.path().join(CLAUDE_FILE)).unwrap(),
            "@AGENTS.md\n"
        );
        let second = written(dir.path(), false);
        assert!(
            second
                .iter()
                .all(|change| change.status == Status::Unchanged),
            "{second:?}"
        );
    }

    #[test]
    fn what_an_older_version_wrote_is_replaced_and_an_edit_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        written(dir.path(), false);
        let skill = |name: &str| dir.path().join(SKILLS_DIR).join(name).join("SKILL.md");

        // As an older version wrote it: another text, with its own hash.
        let older = stamp_skill(&SKILLS[1].1.replace("Write matching rules", "Rules"));
        std::fs::write(skill("spool-rules"), &older).unwrap();
        // Edited by somebody.
        let edited = format!(
            "{}\nOur rules use `country_code`.\n",
            stamp_skill(SKILLS[2].1)
        );
        std::fs::write(skill("spool-extractor"), &edited).unwrap();
        // A skill an older version had and this one does not, and an edited one.
        let gone = dir.path().join(SKILLS_DIR).join("spool-gone");
        std::fs::create_dir_all(&gone).unwrap();
        std::fs::write(
            gone.join("SKILL.md"),
            stamp_skill("---\nname: spool-gone\ndescription: Gone.\n---\n\nGone.\n"),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            Path::new("../../.agents/skills/spool-gone"),
            dir.path().join(LINKS_DIR).join("spool-gone"),
        )
        .unwrap();
        let mine = dir.path().join(SKILLS_DIR).join("spool-mine");
        std::fs::create_dir_all(&mine).unwrap();
        std::fs::write(mine.join("SKILL.md"), "---\nname: spool-mine\n---\nMine.\n").unwrap();
        // Somebody else's skill, by its name.
        let theirs = dir.path().join(SKILLS_DIR).join("deploy");
        std::fs::create_dir_all(&theirs).unwrap();
        std::fs::write(theirs.join("SKILL.md"), "theirs").unwrap();

        let changes = written(dir.path(), false);
        assert_eq!(
            status_of(&changes, ".agents/skills/spool-rules/SKILL.md"),
            Status::Updated
        );
        assert_eq!(
            std::fs::read_to_string(skill("spool-rules")).unwrap(),
            stamp_skill(SKILLS[1].1)
        );
        assert_eq!(
            status_of(&changes, ".agents/skills/spool-extractor/SKILL.md"),
            Status::Kept
        );
        assert_eq!(
            std::fs::read_to_string(skill("spool-extractor")).unwrap(),
            edited
        );
        assert_eq!(
            status_of(&changes, ".agents/skills/spool-gone/SKILL.md"),
            Status::Removed
        );
        assert_eq!(
            status_of(&changes, ".claude/skills/spool-gone"),
            Status::Removed
        );
        assert!(!gone.exists());
        assert!(std::fs::symlink_metadata(dir.path().join(LINKS_DIR).join("spool-gone")).is_err());
        assert_eq!(
            status_of(&changes, ".agents/skills/spool-mine/SKILL.md"),
            Status::Kept
        );
        assert!(mine.join("SKILL.md").is_file());
        assert!(changes.iter().all(|change| !change.path.contains("deploy")));
        assert_eq!(
            std::fs::read_to_string(theirs.join("SKILL.md")).unwrap(),
            "theirs"
        );

        let forced = written(dir.path(), true);
        assert_eq!(
            status_of(&forced, ".agents/skills/spool-extractor/SKILL.md"),
            Status::Updated
        );
        assert_eq!(
            std::fs::read_to_string(skill("spool-extractor")).unwrap(),
            stamp_skill(SKILLS[2].1)
        );
    }

    #[test]
    fn agents_md_keeps_what_is_around_spools_part() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(AGENTS_FILE);

        // A file of somebody's own gets Spool's part after what it says.
        std::fs::write(&file, "# Our project\n\nRun the tests with `make test`.").unwrap();
        std::fs::write(dir.path().join(CLAUDE_FILE), "Be brief.\n").unwrap();
        let changes = written(dir.path(), false);
        assert_eq!(status_of(&changes, AGENTS_FILE), Status::Updated);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            format!(
                "# Our project\n\nRun the tests with `make test`.\n\n{}\n",
                agents_section()
            )
        );
        assert_eq!(status_of(&changes, CLAUDE_FILE), Status::Kept);
        assert_eq!(
            std::fs::read_to_string(dir.path().join(CLAUDE_FILE)).unwrap(),
            "Be brief.\n"
        );

        // An older version's part is replaced, and what is around it stays.
        let with = |hash: &str| format!("{BEGIN}{STAMP}{hash}\" -->\nOlder words.\n{END}");
        let older = with(&digest(&with("")));
        std::fs::write(&file, format!("Before.\n\n{older}\n\nAfter.\n")).unwrap();
        let changes = written(dir.path(), false);
        assert_eq!(status_of(&changes, AGENTS_FILE), Status::Updated);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            format!("Before.\n\n{}\n\nAfter.\n", agents_section())
        );

        // An edit inside it is kept, and --force replaces only that part.
        let edited = format!(
            "Before.\n\n{}\n\nAfter.\n",
            agents_section().replace("## Working here", "## Working here\n\n- Use port 7800.")
        );
        std::fs::write(&file, &edited).unwrap();
        let changes = written(dir.path(), false);
        assert_eq!(status_of(&changes, AGENTS_FILE), Status::Kept);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), edited);
        written(dir.path(), true);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            format!("Before.\n\n{}\n\nAfter.\n", agents_section())
        );
    }

    #[test]
    fn a_link_that_is_not_spools_is_kept_unless_forced() {
        let dir = tempfile::tempdir().unwrap();
        let links = dir.path().join(LINKS_DIR);
        std::fs::create_dir_all(links.join("spool-data")).unwrap();
        std::fs::write(links.join("spool-data/SKILL.md"), "a copy").unwrap();
        std::os::unix::fs::symlink(Path::new("/somewhere/else"), links.join("spool-api")).unwrap();

        let changes = written(dir.path(), false);
        assert_eq!(
            status_of(&changes, ".claude/skills/spool-data"),
            Status::Kept
        );
        assert_eq!(
            status_of(&changes, ".claude/skills/spool-api"),
            Status::Kept
        );
        let forced = statuses(&written(dir.path(), true));
        assert!(forced.contains(&(".claude/skills/spool-api".to_owned(), Status::Updated)));
        assert!(forced.contains(&(".claude/skills/spool-data".to_owned(), Status::Kept)));
        assert_eq!(
            std::fs::read_link(links.join("spool-api")).unwrap(),
            Path::new("../../.agents/skills/spool-api")
        );
        assert_eq!(
            std::fs::read_to_string(links.join("spool-data/SKILL.md")).unwrap(),
            "a copy"
        );
    }

    /// Each skill is as the Agent Skills format says: a name of lowercase
    /// letters, digits and hyphens that is its folder's, a description of at
    /// most 1,024 characters, and no `metadata` of its own, which is where
    /// the hash goes.
    #[test]
    fn every_skill_has_the_frontmatter_agents_read() {
        for (name, source) in SKILLS {
            let front = source
                .strip_prefix("---\n")
                .and_then(|body| body.split_once("\n---\n"))
                .map(|(front, _)| front)
                .unwrap_or_else(|| panic!("{name}: no frontmatter"));
            let field = |key: &str| {
                front
                    .lines()
                    .find_map(|line| line.strip_prefix(&format!("{key}: ")))
                    .unwrap_or_else(|| panic!("{name}: no `{key}`"))
            };
            assert_eq!(field("name"), name);
            assert!(name.starts_with(PREFIX), "{name}");
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            );
            let description = field("description");
            assert!(
                !description.is_empty() && description.len() <= 1024,
                "{name}"
            );
            assert!(
                !description.contains(": "),
                "{name}: `: ` in a plain YAML value"
            );
            assert!(!front.contains("metadata:"), "{name}");
            assert!(
                AGENTS.contains(&format!("`{name}`")),
                "{name} is not listed in AGENTS.md"
            );
        }
    }
}
