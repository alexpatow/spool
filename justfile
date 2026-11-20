# The commands this repository is worked on with. `just` lists them.
#
# CI runs the same checks, spelled out in .github/workflows/ci.yml, where
# each job installs only what it needs.

packages := "packages/client packages/strategy packages/stress packages/studio"

# Lists the commands.
default:
    @just --list --unsorted

# Everything CI checks that needs no test run.
check: fmt-check lint typecheck sizes generated

# Formats the Rust code.
fmt:
    cargo fmt --all

# Fails if the Rust code is not formatted.
fmt-check:
    cargo fmt --all -- --check

# Clippy, with warnings as errors, as CI runs it.
lint:
    cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

# Type-checks every TypeScript package.
typecheck:
    for package in {{ packages }}; do (cd "$package" && bun run typecheck) || exit 1; done

# Fails if a source file is over the line budget and not on the list.
sizes:
    bash scripts/check-file-size.sh

# Fails if a generated file is not what its source generates.
generated:
    bash scripts/check-generated.sh --all

# The Rust tests. The tests that run user code, the stress tests and the
# studio's are ignored unless asked for: `just test-all`.
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
regen-types: regen-schema
    cd packages/client && bun run generate

# The studio the binary serves, from packages/studio.
regen-studio:
    cd packages/studio && bun install --frozen-lockfile && bun run build

# The helper a TypeScript strategy imports, from packages/strategy.
regen-strategy:
    cd packages/strategy && bun install --frozen-lockfile && bun run build

# The help snapshots and the command reference of the documentation.
regen-help:
    SNAPSHOTS=overwrite cargo test --locked -p spool-cli --bin spool -- --exact tests::the_help_of_every_command_is_its_snapshot tests::the_command_reference_is_what_the_commands_say
