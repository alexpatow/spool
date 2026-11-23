# The commands this repository is worked on with. `just` lists them.
#
# CI runs the same checks, spelled out in .github/workflows/ci.yml, where
# each job installs only what it needs.

packages := "packages/client packages/strategy packages/stress packages/studio packages/testing packages/ui"

# Lists the commands.
default:
    @just --list --unsorted

# Everything CI checks that needs no test run.
check: fmt-check lint unused typecheck sizes generated

# Formats the Rust code and the TypeScript.
fmt:
    cargo fmt --all
    bun run fmt

# Fails if the Rust code or the TypeScript is not formatted.
fmt-check: install
    cargo fmt --all -- --check
    bun run fmt:check

# Clippy and oxlint, with warnings as errors, as CI runs them.
lint: install
    cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
    bun run lint

# TypeScript exports, files and dependencies nothing uses.
unused: install
    bun run unused

# They are one bun workspace, with one lockfile at the root.
#
# Installs what the TypeScript packages and the site depend on.
install:
    bun install --frozen-lockfile

# Type-checks every TypeScript package.
typecheck: install
    for package in {{ packages }}; do (cd "$package" && bun run typecheck) || exit 1; done

# Fails if a source file is over the line budget and not on the list.
sizes:
    bash scripts/check-file-size.sh

# Fails if a generated file is not what its source generates.
generated:
    bash scripts/check-generated.sh --all

# The tests that run user code, the stress tests and the studio's are
# ignored unless asked for: `just test-all`.
#
# The Rust tests, without the ones that are ignored.
test *arguments:
    cargo test --locked --workspace {{ arguments }}

# Every test, as CI runs them all together. Needs `uv` and `bun`.
test-all *arguments:
    cargo test --locked --workspace {{ arguments }} -- --include-ignored

# Rewrites every generated file.
regen: regen-schema regen-types regen-studio regen-strategy regen-help

# The JSON Schema of the API, from the server's types.
regen-schema:
    schema=$(cargo run --locked --quiet -p spool-cli -- schema) && printf '%s\n' "$schema" > schema/spool-api.v1.schema.json

# The client's TypeScript types, from the schema.
regen-types: regen-schema install
    cd packages/client && bun run generate

# The studio the binary serves, from packages/studio.
regen-studio: install
    cd packages/studio && bun run build

# The helper a TypeScript strategy imports, from packages/strategy.
regen-strategy: install
    cd packages/strategy && bun run build

# The help snapshots and the command reference of the documentation.
regen-help:
    SNAPSHOTS=overwrite cargo test --locked -p spool-cli --bin spool -- --exact tests::the_help_of_every_command_is_its_snapshot tests::the_command_reference_is_what_the_commands_say
