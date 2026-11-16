//! The commands that work on a project directory: `init`, `info`, `build`
//! and `apply`.

use std::path::Path;

use spool::{Engine, Manifest, Options, Project};

use super::manifest_path;
use crate::Failure;
use crate::args::Spec;

pub const INIT_USAGE: &str = "\
Usage: spool init [<dir>] [options]

Scaffolds a project: one entity type with a features query and two rules, to
be replaced with your own.

  <dir>             the project directory (default: the current directory)

Options:
  --entity <name>   the entity type to scaffold (default: record)
";

pub const INFO_USAGE: &str = "\
Usage: spool info [<dir>] [options]

Lists what the project directory declares: its entity types, their feature
columns and their rules. Every SQL file is checked on the way.

  <dir>     the project directory (default: the current directory)

Options:
  --json    print the listing as JSON
";

pub const BUILD_USAGE: &str = "\
Usage: spool build [<dir>] [options]

Checks the project and compiles it into .spool/manifest.json, which is all
the server reads. Writes a Dockerfile if the project has none.

  <dir>                  the project directory (default: the current directory)

Options:
  --base-image <image>   the image the written Dockerfile starts from
";

pub const APPLY_USAGE: &str = "\
Usage: spool apply [<dir>] --store <path> [options]

Applies a manifest to a store: declares new entity types, and computes the
feature table again for each type whose declaration changed. Identities are
not touched. The server must not be running on the store.

  <dir>               the project directory (default: the current directory)

Options:
  --store <path>      the store's database file; created if it does not exist
                      (default: $SPOOL_STORE)
  --manifest <path>   the manifest to apply (default: <dir>/.spool/manifest.json)
";

const FEATURES: &str = "\
-- The features of this entity type: one row per observation, and whatever
-- columns the rules need. `observation` holds this type's observations, and
-- `payload` holds each one's attributes exactly as they were ingested.
SELECT
  observation_id,
  lower(trim(payload->>'name')) AS name
FROM observation
";

const BLOCK_RULE: &str = "\
-- A block rule says which pairs are worth comparing. `l` and `r` are two
-- rows of the feature table.
l.name = r.name
";

const MATCH_RULE: &str = "\
-- A match rule says a pair is the same thing. A file under rules/no_match/
-- says a pair is not, and wins over any match rule.
l.name = r.name
";

/// The default base image of a written Dockerfile.
fn base_image() -> String {
    format!("ghcr.io/alexpatow/spool:{}", env!("CARGO_PKG_VERSION"))
}

fn dockerfile(base_image: &str) -> String {
    format!(
        "\
# Written by `spool build`. It belongs to this project: edit it as you need.
#
# The image holds the spool binary and this project's manifest. The store is
# not in the image: it is a file on the volume at /data.
#
#   docker build -t my-spool .
#   docker run -p 7765:7765 -v spool-data:/data -e SPOOL_TOKEN=... my-spool
#
# A changed project is applied to an existing store as a step of its own,
# with the server stopped:
#
#   docker run --rm -v spool-data:/data my-spool apply --manifest /spool/manifest.json

FROM {base_image} AS build
WORKDIR /project
COPY . .
RUN spool build

FROM {base_image}
COPY --from=build /project/.spool/manifest.json /spool/manifest.json
ENV SPOOL_STORE=/data/spool.duckdb
VOLUME /data
USER spool
EXPOSE 7765
CMD [\"serve\", \"--manifest\", \"/spool/manifest.json\", \"--listen\", \"0.0.0.0:7765\"]
"
    )
}

const DOCKERIGNORE: &str = "\
.git
.spool
";

fn failed(message: impl Into<String>) -> Failure {
    Failure::Failed(message.into())
}

fn write(path: &Path, contents: &str) -> Result<(), Failure> {
    if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)
            .map_err(|error| failed(format!("{}: {error}", dir.display())))?;
    }
    std::fs::write(path, contents).map_err(|error| failed(format!("{}: {error}", path.display())))
}

pub fn init(arguments: &[String]) -> Result<(), Failure> {
    let arguments = super::arguments(
        arguments,
        &Spec {
            values: &["--entity"],
            flags: &[],
            positional: 1,
        },
    )?;
    let dir = arguments.project_dir();
    let entity = arguments.value("--entity").unwrap_or("record");
    spool::core::EntityType::new(entity.to_owned())
        .map_err(|error| Failure::Usage(error.to_string()))?;

    let entities = dir.join("entities");
    if entities.exists() {
        return Err(failed(format!(
            "{} already exists; this is a project already",
            entities.display()
        )));
    }
    let entity_dir = entities.join(entity);
    write(&entity_dir.join("features.sql"), FEATURES)?;
    write(&entity_dir.join("rules/block/same_name.sql"), BLOCK_RULE)?;
    write(&entity_dir.join("rules/match/same_name.sql"), MATCH_RULE)?;
    let gitignore = dir.join(".gitignore");
    if !gitignore.exists() {
        write(&gitignore, ".spool/\n")?;
    }

    // What was written has to be a project that builds.
    Manifest::load(&dir)?;
    println!("created   {}", entity_dir.display());
    println!(
        "\nThe entity type `{entity}` matches observations whose `name` is the same.\n\
         Edit features.sql and the rules to say what makes two of yours the same thing.\n\n  \
         spool info    lists what the project declares\n  \
         spool dev     runs the server on localhost and applies your changes as you save"
    );
    Ok(())
}

pub fn info(arguments: &[String]) -> Result<(), Failure> {
    let arguments = super::arguments(
        arguments,
        &Spec {
            values: &[],
            flags: &["--json"],
            positional: 1,
        },
    )?;
    let dir = arguments.project_dir();
    let project = Project::read(&dir)?;
    let manifest = project.compile()?;

    let names = |rules: &[spool::Rule]| -> Vec<String> {
        rules.iter().map(|rule| rule.name.clone()).collect()
    };
    let mut warnings = Vec::new();
    for entity_type in &manifest.entity_types {
        if entity_type.rules.block.is_empty() {
            warnings.push(format!(
                "`{}` has no block rules, so no two observations are ever compared",
                entity_type.name
            ));
        } else if entity_type.rules.match_.is_empty() {
            warnings.push(format!(
                "`{}` has no match rules, so the built-in strategy never matches anything",
                entity_type.name
            ));
        }
    }
    let ignored: Vec<String> = project
        .ignored
        .iter()
        .map(|path| path.display().to_string())
        .collect();

    if arguments.flag("--json") {
        let listing = serde_json::json!({
            "manifest_hash": manifest.hash,
            "entity_types": manifest.entity_types.iter().map(|entity_type| serde_json::json!({
                "name": entity_type.name,
                "features": entity_type.declaration.columns,
                "rules": {
                    "block": names(&entity_type.rules.block),
                    "no_match": names(&entity_type.rules.no_match),
                    "match": names(&entity_type.rules.match_),
                },
            })).collect::<Vec<_>>(),
            "ignored": ignored,
            "warnings": warnings,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&listing).expect("a listing serialises to JSON")
        );
        return Ok(());
    }

    println!("project   {}", dir.display());
    println!("manifest  {}", manifest.hash);
    if manifest.entity_types.is_empty() {
        println!("\nno entity types: add a directory under entities/ with a features.sql");
    }
    for entity_type in &manifest.entity_types {
        println!("\nentity type `{}`", entity_type.name);
        let columns: Vec<String> = entity_type
            .declaration
            .columns
            .iter()
            .map(|column| format!("{} {}", column.name, column.data_type))
            .collect();
        println!("  features  {}", columns.join(", "));
        for (label, rules) in [
            ("block", &entity_type.rules.block),
            ("no_match", &entity_type.rules.no_match),
            ("match", &entity_type.rules.match_),
        ] {
            let listed = match names(rules) {
                names if names.is_empty() => "(none)".to_owned(),
                names => names.join(", "),
            };
            println!("  {label:<9} {listed}");
        }
    }
    if !ignored.is_empty() {
        println!("\nnot used by this version:");
        for path in &ignored {
            println!("  {path}");
        }
    }
    for warning in &warnings {
        println!("\nwarning: {warning}");
    }
    Ok(())
}

pub fn build(arguments: &[String]) -> Result<(), Failure> {
    let arguments = super::arguments(
        arguments,
        &Spec {
            values: &["--base-image"],
            flags: &[],
            positional: 1,
        },
    )?;
    let dir = arguments.project_dir();
    let manifest = Manifest::load(&dir)?;
    let path = manifest_path(&dir);
    manifest.write_file(&path)?;
    println!("manifest  {}", path.display());
    println!("hash      {}", manifest.hash);
    let names: Vec<&str> = manifest
        .entity_types
        .iter()
        .map(|entity_type| entity_type.name.as_str())
        .collect();
    println!("types     {}", names.join(", "));

    let dockerfile_path = dir.join("Dockerfile");
    if dockerfile_path.exists() {
        if arguments.value("--base-image").is_some() {
            return Err(failed(format!(
                "{} already exists and belongs to the project; change its base image there",
                dockerfile_path.display()
            )));
        }
    } else {
        let image = arguments
            .value("--base-image")
            .map_or_else(base_image, str::to_owned);
        write(&dockerfile_path, &dockerfile(&image))?;
        println!("wrote     {}", dockerfile_path.display());
        let dockerignore = dir.join(".dockerignore");
        if !dockerignore.exists() {
            write(&dockerignore, DOCKERIGNORE)?;
            println!("wrote     {}", dockerignore.display());
        }
    }
    Ok(())
}

pub fn apply(arguments: &[String]) -> Result<(), Failure> {
    let arguments = super::arguments(
        arguments,
        &Spec {
            values: &["--store", "--manifest"],
            flags: &[],
            positional: 1,
        },
    )?;
    let store = super::store_path(&arguments)?;
    let manifest_file = arguments
        .path("--manifest")
        .unwrap_or_else(|| manifest_path(&arguments.project_dir()));
    let manifest = Manifest::read_file(&manifest_file)?;

    let engine = Engine::open(&store, Options::default())?;
    let before = engine.applied_manifest_hash()?;
    let report = engine.apply(&manifest)?;
    println!("store     {}", store.display());
    println!("manifest  {}", manifest.hash);
    let list = |label: &str, types: &[spool::core::EntityType]| {
        if !types.is_empty() {
            let names: Vec<&str> = types.iter().map(|name| name.as_str()).collect();
            println!("{label:<9} {}", names.join(", "));
        }
    };
    list("declared", &report.declared);
    list("rebuilt", &report.rebuilt);
    list("unchanged", &report.unchanged);
    if before.as_deref() == Some(manifest.hash.as_str()) {
        println!("\nthe store was already at this manifest");
    }
    Ok(())
}
