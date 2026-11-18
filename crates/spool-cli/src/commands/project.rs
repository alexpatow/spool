//! The commands that work on a project directory: `init`, `info`, `build`
//! and `apply`.

use std::path::Path;

use spool::{Engine, Manifest, Options, Project, Runtime};

use super::manifest_path;
use crate::Failure;
use crate::args::Spec;

pub const INIT_USAGE: &str = "\
Usage: spool init [<dir>] [options]

Scaffolds a project: one source, one entity type with its features, a
transform and two rules, to be replaced with your own.

  <dir>             the project directory (default: the current directory)

Options:
  --entity <name>   the entity type to scaffold (default: record)
  --source <name>   the source to scaffold (default: example)
";

pub const INFO_USAGE: &str = "\
Usage: spool info [<dir>] [options]

Lists what the project directory declares: its sources, its entity types,
their feature columns, their rules and their user-code strategies, and its extractors.
Every SQL file is checked on the way.

  <dir>     the project directory (default: the current directory)

Options:
  --json    print the listing as JSON
";

pub const BUILD_USAGE: &str = "\
Usage: spool build [<dir>] [options]

Checks the project and compiles it into .spool/manifest.json. The server
reads the manifest, and the strategy files it names. Writes a Dockerfile if
the project has none.

  <dir>                  the project directory (default: the current directory)

Options:
  --base-image <image>   the image the written Dockerfile starts from
  --install              start each user-code strategy once with nothing to
                         do, so that `uv` and `bun` fetch what it depends on
                         and a file that cannot start is found now

SPOOL_UV and SPOOL_BUN name `uv` and `bun` if they are not on the search path.
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

const SOURCE: &str = "\
# A source of rows. `key` is the column that tells one row from another, and
# `columns` is every column a row may have.
key: id
columns: [id, name]
";

fn entity_yaml(source: &str) -> String {
    format!(
        "\
# `features` is what records are compared by, and `label` is the feature a
# record is called by. `mapping` says, for each source, which column each
# feature comes from, and the transform it goes through if it needs one.
label: name
features: [name, name_key]

mapping:
  {source}:
    name: name
    name_key: {{ from: name, transform: name_key }}
"
    )
}

const TRANSFORM: &str = "\
-- A transform is `lambda`, the names of its arguments, and a DuckDB
-- expression over them.
lambda name: lower(trim(name))
";

const BLOCK_RULE: &str = "\
-- A block rule says which pairs are worth comparing. `l` and `r` are two
-- rows of the feature table.
l.name_key = r.name_key
";

const MATCH_RULE: &str = "\
-- A match rule says a pair is the same thing. A file under rules/no_match/
-- says a pair is not, and wins over any match rule.
l.name_key = r.name_key
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
#   docker run -p 7765:7765 -v spool-data:/data my-spool
#
# When it first starts, the server prints a link that makes the first account.
# People sign in over HTTPS: put the server behind something that speaks it,
# or add `--plain-http` to the command below to try it without. A caller that
# is not a person presents a token: an admin makes one on the studio's Tokens
# page, and so does `docker exec <container> spool token add <name>`.
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

/// The `uv` and `bun` a written Dockerfile takes: the versions user code was
/// tested with.
const UV_IMAGE: &str = "ghcr.io/astral-sh/uv:0.13.8";
const BUN_IMAGE: &str = "oven/bun:1.4.4";

/// The Dockerfile of a project with user code. The image also holds the
/// project's files, what runs them, and what they depend on.
fn dockerfile_with_user_code(base_image: &str, python: bool, typescript: bool) -> String {
    let mut runtimes = String::new();
    let mut environment = String::new();
    if python {
        runtimes.push_str(&format!("COPY --from={UV_IMAGE} /uv /usr/local/bin/uv\n"));
        environment.push_str(
            "ENV UV_CACHE_DIR=/spool/cache/uv UV_PYTHON_INSTALL_DIR=/spool/cache/python\n",
        );
    }
    if typescript {
        runtimes.push_str(&format!(
            "COPY --from={BUN_IMAGE} /usr/local/bin/bun /usr/local/bin/bun\n"
        ));
        environment.push_str("ENV BUN_INSTALL_CACHE_DIR=/spool/cache/bun\n");
    }
    let offline = if python {
        "\n# Everything was fetched above: a run does not wait for the network.\nENV UV_OFFLINE=1\n"
    } else {
        ""
    };
    format!(
        "\
# Written by `spool build`. It belongs to this project: edit it as you need.
#
# The image holds the spool binary, this project's manifest, and the strategy
# files the manifest names with what runs them. The store is not in the
# image: it is a file on the volume at /data.
#
#   docker build -t my-spool .
#   docker run -p 7765:7765 -v spool-data:/data my-spool
#
# When it first starts, the server prints a link that makes the first account.
# People sign in over HTTPS: put the server behind something that speaks it,
# or add `--plain-http` to the command below to try it without. A caller that
# is not a person presents a token: an admin makes one on the studio's Tokens
# page, and so does `docker exec <container> spool token add <name>`.
#
# A changed project is applied to an existing store as a step of its own,
# with the server stopped:
#
#   docker run --rm -v spool-data:/data my-spool apply

FROM {base_image} AS build
WORKDIR /project
COPY . .
RUN spool build

FROM {base_image}
{runtimes}RUN mkdir -p /spool/cache && chown spool:spool /spool/cache
COPY --from=build --chown=spool:spool /project /spool/project
WORKDIR /spool/project
ENV SPOOL_STORE=/data/spool.duckdb
{environment}VOLUME /data
USER spool
# Starts each strategy once with nothing to do, which fetches what it depends
# on into the image.
RUN spool build --install
{offline}EXPOSE 7765
CMD [\"serve\", \"--listen\", \"0.0.0.0:7765\"]
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
            values: &["--entity", "--source"],
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
    let source = arguments.value("--source").unwrap_or("example");
    let source_file = dir.join("sources").join(format!("{source}.yaml"));
    let transform_file = dir.join("transforms/name_key.sql");
    for file in [&source_file, &transform_file] {
        if file.exists() {
            return Err(failed(format!("{} already exists", file.display())));
        }
    }
    let entity_dir = entities.join(entity);
    write(&source_file, SOURCE)?;
    write(&transform_file, TRANSFORM)?;
    write(&entity_dir.join("entity.yaml"), &entity_yaml(source))?;
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
        "\nThe entity type `{entity}` takes rows from the source `{source}` and matches\n\
         those whose `name` is the same but for case.\n\
         Edit sources/{source}.yaml, entity.yaml and the rules to say what your rows\n\
         hold and what makes two of them the same thing.\n\n  \
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
            warnings.push(if entity_type.strategies.is_empty() {
                format!(
                    "`{}` has no block rules, so no two records are ever compared",
                    entity_type.name
                )
            } else {
                format!(
                    "`{}` has no block rules, so its strategies are given no candidate pairs",
                    entity_type.name
                )
            });
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
            "sources": manifest.sources,
            "entity_types": manifest.entity_types.iter().map(|entity_type| serde_json::json!({
                "name": entity_type.name,
                "label": entity_type.declaration.label,
                "features": entity_type.declaration.columns,
                "sources": entity_type.sources,
                "rules": {
                    "block": names(&entity_type.rules.block),
                    "no_match": names(&entity_type.rules.no_match),
                    "match": names(&entity_type.rules.match_),
                },
                "strategies": entity_type.strategies,
                "extract": entity_type.extract,
            })).collect::<Vec<_>>(),
            "extractors": manifest.extractors,
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
        println!("\nno entity types: add a directory under entities/ with an entity.yaml");
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
        if let Some(label) = &entity_type.declaration.label {
            println!("  label     {label}");
        }
        println!("  sources   {}", entity_type.sources.join(", "));
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
        for strategy in &entity_type.strategies {
            println!(
                "  strategy  {} ({}, {})",
                strategy.name, strategy.runtime, strategy.file
            );
        }
        if entity_type.extract.is_some() {
            println!("  extract   extract.md says what an extractor looks for");
        }
    }
    if !manifest.extractors.is_empty() {
        println!();
    }
    for extractor in &manifest.extractors {
        println!(
            "extractor `{}` ({}, {})",
            extractor.name, extractor.runtime, extractor.file
        );
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
            flags: &["--install"],
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
    let strategies: Vec<&spool::Strategy> = manifest
        .entity_types
        .iter()
        .flat_map(|entity_type| &entity_type.strategies)
        .collect();
    let runtimes: Vec<Runtime> = strategies
        .iter()
        .map(|strategy| strategy.runtime)
        .chain(
            manifest
                .extractors
                .iter()
                .map(|extractor| extractor.runtime),
        )
        .collect();
    let uses = |runtime: Runtime| runtimes.contains(&runtime);

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
        let contents = if runtimes.is_empty() {
            dockerfile(&image)
        } else {
            dockerfile_with_user_code(&image, uses(Runtime::Python), uses(Runtime::Typescript))
        };
        write(&dockerfile_path, &contents)?;
        println!("wrote     {}", dockerfile_path.display());
        let dockerignore = dir.join(".dockerignore");
        if !dockerignore.exists() {
            write(&dockerignore, DOCKERIGNORE)?;
            println!("wrote     {}", dockerignore.display());
        }
    }

    if arguments.flag("--install") {
        let user_code = super::user_code(&arguments, &dir)?;
        for strategy in strategies {
            user_code.install(strategy)?;
            println!("starts    {}", strategy.file);
        }
        for extractor in &manifest.extractors {
            user_code.install(extractor)?;
            println!("starts    {}", extractor.file);
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
