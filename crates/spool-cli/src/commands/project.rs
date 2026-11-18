//! The commands that work on a project directory: `init`, `info`, `build`
//! and `apply`.

use std::path::Path;

use spool::{Engine, Manifest, Options, Project, Runtime};

use super::manifest_path;
use crate::Failure;
use crate::args::Spec;

pub const INIT_USAGE: &str = "\
Usage: spool init [<dir>] [options]

Scaffolds a project: a source of rows and a source of documents, one entity
type with its features, a transform, three rules and an extractor, to be
replaced with your own. Under samples/ it writes two files of rows, which the
rules match as they stand, and a document, in which the extractor finds
three records.

  <dir>             the project directory (default: the current directory)

Options:
  --entity <name>   the entity type to scaffold (default: company)
  --source <name>   the source of rows to scaffold (default: crm)

The sample rows and the sample document are about companies, whatever the two
are called. The source of documents is `notes`.
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
columns: [id, name, country]
";

/// The source the sample document belongs to, and with it the records
/// the sample extractor finds.
const DOCUMENTS: &str = "notes";

const DOCUMENT_SOURCE: &str = "\
# A source of documents: here, notes of calls. An extractor finds records in
# them, and `columns` is everything such a record may say.
columns: [name]
";

fn entity_yaml(source: &str) -> String {
    format!(
        "\
# `features` is what records are compared by, and `label` is the feature a
# record is called by. `mapping` says, for each source, which column each
# feature comes from, and the transform it goes through if it needs one. A
# record found in a note has no country.
label: name
features: [name, name_key, country]

mapping:
  {source}:
    name: name
    name_key: {{ from: name, transform: name_key }}
    country: country
  {DOCUMENTS}:
    name: name
    name_key: {{ from: name, transform: name_key }}
"
    )
}

fn extractor_yaml(entity: &str) -> String {
    format!(
        "\
# An extractor reads documents and says where they name something. This one
# is a pattern, which Spool runs itself: every place the pattern matches
# becomes a record of `entity_type`, and the text there is its `attribute`.
#
# The pattern takes a run of capitalised words that ends in a legal form,
# such as Ltd, to be a name. It misses a name written without one, and takes
# a capitalised word before a name to be part of it. An extractor that asks
# a language model instead is a Python or TypeScript file in this directory:
# examples/extractors/model.ts in Spool's repository is one to copy.
entity_type: {entity}
attribute: name
pattern: '(?:[A-Z][A-Za-z]+ )+(?:Ltd|Inc|LLC|GmbH|plc)\\b'
"
    )
}

const TRANSFORM: &str = "\
-- A transform is `lambda`, the names of its arguments, and a DuckDB
-- expression over them.
lambda name: lower(trim(name))
";

const BLOCK_RULE: &str = "\
-- A block rule says which pairs are worth comparing: here, two names that
-- begin with the same four characters. `l` and `r` are two rows of the
-- feature table.
left(l.name_key, 4) = left(r.name_key, 4)
";

const MATCH_RULE: &str = "\
-- A match rule says a pair is the same thing: here, two names that are
-- close. The similarity is 1 for equal names and 0 for names with nothing
-- in common.
jaro_winkler_similarity(l.name_key, r.name_key) >= 0.92
";

const NO_MATCH_RULE: &str = "\
-- A no_match rule says a pair is not the same thing, and wins over any
-- match rule. This one holds only when both rows have a country.
l.country <> r.country
";

/// The sample rows. The first file has three spellings of one company, which
/// the match rule joins, and one name in two countries, which the no_match
/// rule keeps apart. The second has a misspelling of each of two companies
/// of the first, and one company that is new.
const FIRST_SAMPLE: &str = "\
id,name,country
c-1,Acorn Tools,GB
c-2,ACORN TOOLS,GB
c-3,Acorn Tools Ltd,GB
c-4,Larkfield Logistics,GB
c-5,Halden Group,GB
c-6,Halden Group,US
";

const SECOND_SAMPLE: &str = "\
id,name,country
c-7,Larkfeild Logistics,GB
c-8,Kestrel Analytics,US
c-9,Acorn Tool,GB
";

/// The sample document. It names two companies of the sample rows, each
/// with a legal form that some of its rows leave out, and one company that
/// no row has.
const SAMPLE_DOCUMENT: &str = "\
Call with Dana Reyes on 12 November. She has moved to Acorn Tools Ltd and runs
purchasing there. They now ship through Larkfield Logistics Ltd, and she asked
whether we work with Brightwater Foods Ltd, their new supplier.
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

/// A name that is a file name as it stands: what the project reader takes
/// for a source.
fn file_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// A path as a word of a shell command: as it is if it is plain, and
/// between single quotes if not.
fn shell_word(path: &Path) -> String {
    let path = path.display().to_string();
    let plain = !path.is_empty()
        && !path.starts_with('-')
        && path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c));
    if plain {
        path
    } else {
        format!("'{}'", path.replace('\'', "'\\''"))
    }
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
    let entity = arguments.value("--entity").unwrap_or("company");
    spool::core::EntityType::new(entity.to_owned())
        .map_err(|error| Failure::Usage(error.to_string()))?;
    let source = arguments.value("--source").unwrap_or("crm");
    if !file_name(source) {
        return Err(Failure::Usage(format!(
            "invalid source name {source:?}: use letters, digits, `_` and `-`"
        )));
    }
    if source == DOCUMENTS {
        return Err(Failure::Usage(format!(
            "`{DOCUMENTS}` is the source of the sample document: give the source of rows another name"
        )));
    }

    let entities = dir.join("entities");
    if entities.exists() {
        return Err(failed(format!(
            "{} already exists; this is a project already",
            entities.display()
        )));
    }
    let entity_dir = format!("entities/{entity}");
    let source_file = format!("sources/{source}.yaml");
    let document_source_file = format!("sources/{DOCUMENTS}.yaml");
    let first_sample = format!("samples/{source}-1.csv");
    let second_sample = format!("samples/{source}-2.csv");
    // Named after its source, as the files of rows are, so that a command
    // that is given the file knows which source it belongs to.
    let sample_document = format!("samples/{DOCUMENTS}-1.txt");
    let entity_file = format!("{entity_dir}/entity.yaml");
    let entity_yaml = entity_yaml(source);
    // Named after what it finds, as a person would say it.
    let extractor_file = match entity {
        "company" => "extractors/companies.yaml".to_owned(),
        other => format!("extractors/{other}.yaml"),
    };
    let extractor_yaml = extractor_yaml(entity);
    let rule = |file: &str| format!("{entity_dir}/rules/{file}");
    let mut files = vec![
        (source_file.clone(), SOURCE),
        (document_source_file.clone(), DOCUMENT_SOURCE),
        ("transforms/name_key.sql".to_owned(), TRANSFORM),
        (entity_file.clone(), entity_yaml.as_str()),
        (rule("block/name_prefix.sql"), BLOCK_RULE),
        (rule("match/similar_name.sql"), MATCH_RULE),
        (rule("no_match/different_country.sql"), NO_MATCH_RULE),
        (extractor_file.clone(), extractor_yaml.as_str()),
        (first_sample.clone(), FIRST_SAMPLE),
        (second_sample.clone(), SECOND_SAMPLE),
        (sample_document.clone(), SAMPLE_DOCUMENT),
    ];
    // Nothing is written over, and nothing is written if anything would be.
    for (file, _) in &files {
        let file = dir.join(file);
        if file.exists() {
            return Err(failed(format!("{} already exists", file.display())));
        }
    }
    let ignores = !dir.join(".gitignore").exists();
    if ignores {
        files.push((".gitignore".to_owned(), ".spool/\n"));
    }
    for (file, contents) in &files {
        write(&dir.join(file), contents)?;
    }

    // What was written has to be a project that builds.
    Manifest::load(&dir)?;

    let mut written = vec![
        (
            source_file,
            if source == "crm" {
                "the columns of a CRM export".to_owned()
            } else {
                format!("the columns of a row from `{source}`")
            },
        ),
        (
            document_source_file,
            "what a record found in a note may say".to_owned(),
        ),
        (
            entity_file,
            if entity == "company" {
                "what companies are compared by".to_owned()
            } else {
                format!("what records of type `{entity}` are compared by")
            },
        ),
        (
            format!("{entity_dir}/rules/"),
            "three rules, one line of SQL each".to_owned(),
        ),
        (
            "transforms/name_key.sql".to_owned(),
            "the lower-case name the rules compare".to_owned(),
        ),
        (
            extractor_file,
            if entity == "company" {
                "a pattern that finds company names in text".to_owned()
            } else {
                format!("a pattern that finds records of type `{entity}` in text")
            },
        ),
        (first_sample.clone(), "six rows to load first".to_owned()),
        (second_sample, "three more for later".to_owned()),
        (
            sample_document.clone(),
            if entity == "company" {
                "a call note that names three companies".to_owned()
            } else {
                "a call note that names three of them".to_owned()
            },
        ),
    ];
    if ignores {
        written.push((
            ".gitignore".to_owned(),
            "keeps the server's data, .spool/, out of Git".to_owned(),
        ));
    }
    // The commands that come next, which run in the project.
    let commands = [
        ("spool dev".to_owned(), "runs the server on this machine"),
        (
            format!("spool load {first_sample}"),
            "loads the first rows, from a second terminal",
        ),
        (
            format!("spool extract {sample_document}"),
            if entity == "company" {
                "finds the companies in the note"
            } else {
                "finds the records in the note"
            },
        ),
    ];
    // What each line is for begins in one column: six characters after the
    // longest file, and at least two after the longest command.
    let longest = |lines: &mut dyn Iterator<Item = &str>| {
        lines.map(|line| line.chars().count()).max().unwrap_or(0)
    };
    let width = (longest(&mut written.iter().map(|(file, _)| file.as_str())) + 6)
        .max(longest(&mut commands.iter().map(|(command, _)| command.as_str())) + 2);
    println!("created   {}\n", dir.display());
    for (file, holds) in &written {
        println!("  {file:<width$}{holds}");
    }
    println!();
    if !is_current_dir(&dir) {
        println!("  cd {}", shell_word(&dir));
    }
    for (command, does) in &commands {
        println!("  {command:<width$}{does}");
    }
    Ok(())
}

fn is_current_dir(dir: &Path) -> bool {
    match (std::fs::canonicalize(dir), std::env::current_dir()) {
        (Ok(dir), Ok(current)) => {
            std::fs::canonicalize(current).is_ok_and(|current| current == dir)
        }
        _ => false,
    }
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
    // What the image needs something installed for. A pattern is run by
    // the server itself.
    let runtimes: Vec<Runtime> = strategies
        .iter()
        .map(|strategy| strategy.runtime)
        .chain(
            manifest
                .extractors
                .iter()
                .map(|extractor| extractor.runtime),
        )
        .filter(|runtime| runtime.is_user_code())
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
        for extractor in manifest
            .extractors
            .iter()
            .filter(|extractor| extractor.runtime.is_user_code())
        {
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
