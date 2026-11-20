//! The commands that work on a project directory: `info`, `build` and
//! `apply`.

use std::path::{Path, PathBuf};

use anstream::println;
use clap::Args;
use spool::{Engine, Manifest, Options, Project, Runtime};

use super::{manifest_path, project_dir};
use crate::Failure;
use crate::output::{Output, OutputArgs, aligned, fields, hint};

/// List what a project declares, and check its SQL
///
/// Lists what the project directory declares: its sources, with the columns
/// each has and the features each column becomes; its entity types, with
/// their feature columns, their rules and their user-code strategies; and its
/// extractors. Every SQL file is checked on the way.
///
/// What the records hold in those columns is on the server: `spool show
/// attributes` says how many records have each and the values most have.
#[derive(Args)]
pub struct InfoArgs {
    /// The project directory [default: the current directory]
    dir: Option<PathBuf>,
    #[command(flatten)]
    output: OutputArgs,
}

/// Check a project and compile it into a manifest
///
/// Checks the project and compiles it into .spool/manifest.json, which the
/// server reads along with the strategy and extractor files it names. Writes
/// a Dockerfile if the project has none.
///
/// Set SPOOL_UV and SPOOL_BUN if `uv` and `bun` aren't on your PATH.
#[derive(Args)]
pub struct BuildArgs {
    /// The project directory [default: the current directory]
    dir: Option<PathBuf>,
    /// The image the written Dockerfile starts from
    #[arg(long, value_name = "IMAGE")]
    base_image: Option<String>,
    /// Start each strategy and extractor once, to fetch what it needs
    ///
    /// Starts each strategy and extractor once with no work, so `uv` and
    /// `bun` install its dependencies, any model it loads at startup is
    /// downloaded, and a file that can't start fails now rather than later.
    #[arg(long)]
    install: bool,
    #[command(flatten)]
    output: OutputArgs,
}

/// Apply a manifest to a store
///
/// Applies a manifest to a store: declares new entity types and rebuilds the
/// feature table of each type whose declaration changed. Identities aren't
/// touched. Stop the server on the store first.
#[derive(Args)]
pub struct ApplyArgs {
    /// The project directory [default: the current directory]
    dir: Option<PathBuf>,
    /// The store's database file; created if it does not exist
    #[arg(long, env = "SPOOL_STORE", hide_env_values = true, value_name = "PATH", value_parser = super::any_path())]
    store: Option<PathBuf>,
    /// The manifest to apply [default: <DIR>/.spool/manifest.json]
    #[arg(long, value_name = "PATH")]
    manifest: Option<PathBuf>,
    #[command(flatten)]
    output: OutputArgs,
}

/// The default base image of a written Dockerfile.
fn base_image() -> String {
    format!("ghcr.io/alexpatow/spool:{}", env!("CARGO_PKG_VERSION"))
}

fn format_dockerfile(base_image: &str) -> String {
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
# Passwords are for getting started. To sign people in with Google, run the
# container with SPOOL_GOOGLE_CLIENT_ID and SPOOL_GOOGLE_CLIENT_SECRET set to
# an OAuth client, and SPOOL_PUBLIC_URL to the address people open, such as
# https://spool.example.com. `spool serve --help` says more.
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
        // A model's weights go where its library keeps what it fetched,
        // which is under the home directory unless it is told otherwise,
        // and the home of the user `spool` is the volume.
        environment.push_str(
            "ENV UV_CACHE_DIR=/spool/cache/uv UV_PYTHON_INSTALL_DIR=/spool/cache/python \\\n    \
             XDG_CACHE_HOME=/spool/cache\n",
        );
    }
    if typescript {
        runtimes.push_str(&format!(
            "COPY --from={BUN_IMAGE} /usr/local/bin/bun /usr/local/bin/bun\n"
        ));
        environment.push_str("ENV BUN_INSTALL_CACHE_DIR=/spool/cache/bun\n");
    }
    let offline = if python {
        "\n# Everything was fetched above: a run does not wait for the network, and a\n\
         # model that came from Hugging Face is read from the image.\n\
         ENV UV_OFFLINE=1 HF_HUB_OFFLINE=1\n"
    } else {
        ""
    };
    format!(
        "\
# Written by `spool build`. It belongs to this project: edit it as you need.
#
# The image holds the spool binary, this project's manifest, and the strategy
# and extractor files the manifest names with what runs them. The store is
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
# Passwords are for getting started. To sign people in with Google, run the
# container with SPOOL_GOOGLE_CLIENT_ID and SPOOL_GOOGLE_CLIENT_SECRET set to
# an OAuth client, and SPOOL_PUBLIC_URL to the address people open, such as
# https://spool.example.com. `spool serve --help` says more.
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
# Starts each strategy and extractor once with nothing to do, which fetches
# what it depends on into the image, and a model it loads as it starts.
RUN spool build --install
{offline}EXPOSE 7765
CMD [\"serve\", \"--listen\", \"0.0.0.0:7765\"]
"
    )
}

/// `.env` holds the project's secrets, which the image is not to carry: a
/// container is given them where it runs.
const DOCKERIGNORE: &str = "\
.git
.spool
.env
";

pub(super) fn write_file(path: &Path, contents: &str) -> Result<(), Failure> {
    if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)
            .map_err(|error| Failure::failed(format!("{}: {error}", dir.display())))?;
    }
    std::fs::write(path, contents)
        .map_err(|error| Failure::failed(format!("{}: {error}", path.display())))
}

pub fn info(arguments: InfoArgs) -> Result<(), Failure> {
    let out = Output::report(&arguments.output, "`spool info`")?;
    let dir = project_dir(&arguments.dir);
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

    if !out.is_table() {
        let listing = serde_json::json!({
            "manifest_hash": manifest.hash,
            "sources": manifest.sources,
            "entity_types": manifest.entity_types.iter().map(|entity_type| serde_json::json!({
                "name": entity_type.name,
                "label": entity_type.declaration.label,
                "features": entity_type.declaration.columns,
                "sources": entity_type.sources,
                "mapping": entity_type.mapping,
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
        out.json(&listing);
        return Ok(());
    }

    println!("project   {}", dir.display());
    println!("manifest  {}", manifest.hash);
    for source in &manifest.sources {
        print_source(&manifest, source);
    }
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
        hint(format!("warning: {warning}"));
    }
    Ok(())
}

/// A source: its columns, and the features of each entity type that each
/// column becomes, as the types' mappings say.
fn print_source(manifest: &Manifest, source: &spool::Source) {
    let readers: Vec<&spool::EntityTypeManifest> = manifest
        .entity_types
        .iter()
        .filter(|entity_type| entity_type.sources.contains(&source.name))
        .collect();
    println!("\nsource `{}`", source.name);
    println!(
        "  read by   {}",
        match readers.is_empty() {
            true => "no entity type: no mapping in entities/*/entity.yaml names it".to_owned(),
            false => readers
                .iter()
                .map(|entity_type| entity_type.name.to_string())
                .collect::<Vec<_>>()
                .join(", "),
        }
    );
    let mut rows = vec![vec![
        "  column".to_owned(),
        match readers.len() {
            1 => format!("becomes, in `{}`", readers[0].name),
            _ => "becomes".to_owned(),
        },
    ]];
    for column in &source.columns {
        let mut becomes = Vec::new();
        if source.key.as_ref() == Some(column) {
            becomes.push("the key of each row".to_owned());
        }
        if source.evidence.as_ref() == Some(column) {
            becomes.push("the evidence of each record".to_owned());
        }
        for entity_type in &readers {
            let Some(mapping) = entity_type.mapping.get(&source.name) else {
                continue;
            };
            let features: Vec<String> = entity_type
                .declaration
                .columns
                .iter()
                .filter_map(|feature| {
                    let mapped = mapping.get(&feature.name)?;
                    mapped
                        .from
                        .contains(column)
                        .then(|| match &mapped.transform {
                            None => feature.name.clone(),
                            Some(transform) => format!(
                                "{} = {transform}({})",
                                feature.name,
                                mapped.from.join(", ")
                            ),
                        })
                })
                .collect();
            if features.is_empty() {
                continue;
            }
            becomes.push(match readers.len() {
                1 => features.join(", "),
                _ => format!("{}: {}", entity_type.name, features.join(", ")),
            });
        }
        if becomes.is_empty() {
            becomes.push("no feature".to_owned());
        }
        rows.push(vec![format!("  {column}"), becomes.join("; ")]);
    }
    aligned(&rows, 0);
}

pub fn build(arguments: BuildArgs) -> Result<(), Failure> {
    let out = Output::report(&arguments.output, "`spool build`")?;
    // A line for people as it happens; JSON at the end.
    let say = |label: &str, value: &dyn std::fmt::Display| {
        if out.is_table() {
            println!("{label:<9} {value}");
        }
    };
    let dir = project_dir(&arguments.dir);
    let manifest = Manifest::load(&dir)?;
    let path = manifest_path(&dir);
    manifest.write_file(&path)?;
    say("manifest", &path.display());
    say("hash", &manifest.hash);
    let names: Vec<&str> = manifest
        .entity_types
        .iter()
        .map(|entity_type| entity_type.name.as_str())
        .collect();
    say("types", &names.join(", "));
    let mut wrote = Vec::new();
    let mut started = Vec::new();
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
        if arguments.base_image.is_some() {
            return Err(Failure::failed(format!(
                "{} already exists and belongs to the project; change its base image there",
                dockerfile_path.display()
            )));
        }
    } else {
        let image = arguments.base_image.clone().unwrap_or_else(base_image);
        let contents = if runtimes.is_empty() {
            format_dockerfile(&image)
        } else {
            dockerfile_with_user_code(&image, uses(Runtime::Python), uses(Runtime::Typescript))
        };
        write_file(&dockerfile_path, &contents)?;
        say("wrote", &dockerfile_path.display());
        wrote.push(dockerfile_path);
        let dockerignore = dir.join(".dockerignore");
        if !dockerignore.exists() {
            write_file(&dockerignore, DOCKERIGNORE)?;
            say("wrote", &dockerignore.display());
            wrote.push(dockerignore);
        }
    }

    if arguments.install {
        let user_code = super::user_code(None, &dir);
        for strategy in strategies {
            user_code.install(strategy)?;
            say("starts", &strategy.file);
            started.push(strategy.file.clone());
        }
        for extractor in &manifest.extractors {
            user_code.install(extractor)?;
            say("starts", &extractor.file);
            started.push(extractor.file.clone());
        }
    }
    if !out.is_table() {
        out.json(&serde_json::json!({
            "manifest": path,
            "manifest_hash": manifest.hash,
            "entity_types": names,
            "wrote": wrote,
            "started": started,
        }));
    }
    Ok(())
}

pub fn apply(arguments: ApplyArgs) -> Result<(), Failure> {
    let out = Output::report(&arguments.output, "`spool apply`")?;
    let store = super::store_path(&arguments.store)?;
    let manifest_file = arguments
        .manifest
        .clone()
        .unwrap_or_else(|| manifest_path(&project_dir(&arguments.dir)));
    let manifest = Manifest::read_file(&manifest_file)?;

    let engine = Engine::open(&store, Options::default())?;
    let before = engine.applied_manifest_hash()?;
    let report = engine.apply(&manifest)?;
    let already = before.as_deref() == Some(manifest.hash.as_str());
    if !out.is_table() {
        out.json(&serde_json::json!({
            "store": store,
            "manifest_hash": manifest.hash,
            "declared": report.declared,
            "rebuilt": report.rebuilt,
            "unchanged": report.unchanged,
            "already_applied": already,
        }));
        return Ok(());
    }
    let mut rows = vec![
        ("store", store.display().to_string()),
        ("manifest", manifest.hash.clone()),
    ];
    for (label, types) in [
        ("declared", &report.declared),
        ("rebuilt", &report.rebuilt),
        ("unchanged", &report.unchanged),
    ] {
        if !types.is_empty() {
            let names: Vec<&str> = types.iter().map(|name| name.as_str()).collect();
            rows.push((label, names.join(", ")));
        }
    }
    fields(&rows);
    if already {
        println!("\nthe store was already at this manifest");
    }
    Ok(())
}
