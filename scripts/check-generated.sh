#!/usr/bin/env bash
# Check what a commit would contain, including partially staged files.
#
#   scripts/check-generated.sh                 checks what the staged changes affect
#   scripts/check-generated.sh --all           checks every generated file
#   scripts/check-generated.sh --install-hook  runs the first before each commit
set -eu

root=$(git rev-parse --show-toplevel)
cd "$root"

if [[ ${1:-} == --install-hook && $# == 1 ]]; then
    if ! git -c hook.spool-hook-probe.event=pre-commit -c hook.spool-hook-probe.command=true \
        hook list pre-commit >/dev/null 2>&1; then
        echo 'Installing a configured hook requires Git 2.54 or newer.' >&2
        exit 1
    fi
    # A repository can also have worktrees on branches predating this script.
    git config --local --replace-all hook.spool-check-generated.command \
        'if test -f ./scripts/check-generated.sh; then exec bash ./scripts/check-generated.sh; fi'
    git config --local --replace-all hook.spool-check-generated.event pre-commit
    git config --local --replace-all hook.spool-check-generated.enabled true
    echo 'Installed the generated-file pre-commit check for this repository and its worktrees.'
    exit 0
fi
all=0
if [[ ${1:-} == --all && $# == 1 ]]; then
    all=1
elif (($#)); then
    echo 'Usage: check-generated.sh [--all | --install-hook]' >&2
    exit 2
fi

changed() {
    if ((all)); then
        return 0
    fi
    local status=0
    # Treat a rename as both a deletion and an addition for path matching.
    git diff --cached --quiet --no-ext-diff --no-renames -- "$@" || status=$?
    if ((status > 1)); then
        exit "$status"
    fi
    ((status == 1))
}

rust_config=(Cargo.toml Cargo.lock .cargo rust-toolchain rust-toolchain.toml ':(glob)crates/*/Cargo.toml')
schema_path=schema/spool-api.v1.schema.json
studio=0 schema=0 types=0 help=0 strategy=0
if changed packages/studio/src packages/studio/public packages/studio/index.html \
    packages/studio/vite.config.ts packages/studio/tsconfig.json packages/studio/package.json \
    packages/studio/bun.lock packages/studio/bunfig.toml \
    packages/studio/scripts/build.ts crates/spool-cli/src/studio \
    packages/client/src packages/client/package.json packages/client/tsconfig.json \
    site/src/styles/shelf site/src/components/brand.tsx site/src/app/icon.svg; then
    studio=1
fi
if changed "${rust_config[@]}" "$schema_path" crates/spool-cli/src/api.rs crates/spool-cli/src/api \
    crates/spool-cli/src/main.rs crates/spool-cli/src/commands/mod.rs crates/spool-cli/tests/client.rs \
    ':(glob)crates/spool/src/**/*.rs' ':(glob)crates/spool-core/src/**/*.rs'; then
    schema=1
fi
if changed "$schema_path" packages/client/scripts/generate-types.ts packages/client/src/types.ts \
    packages/client/package.json packages/client/bunfig.toml; then
    types=1
fi
if changed "${rust_config[@]}" crates/spool-cli/src/main.rs crates/spool-cli/src/commands \
    crates/spool-cli/src/output.rs crates/spool-cli/src/reference.rs \
    crates/spool-cli/tests/snapshots/help site/content/docs/cli-reference.mdx \
    crates/spool/src/engine.rs crates/spool/src/lib.rs; then
    help=1
fi
if changed packages/strategy/src packages/strategy/scripts/build.ts packages/strategy/package.json \
    packages/strategy/bun.lock packages/strategy/bunfig.toml packages/strategy/tsconfig.json \
    crates/spool/src/usercode/spool.js; then
    strategy=1
fi

if ((!studio && !schema && !types && !help && !strategy)); then
    echo 'No staged changes affect generated files.'
    exit 0
fi

snapshot=$(mktemp -d "${TMPDIR:-/tmp}/spool-generated.XXXXXXXX")
trap 'rm -rf -- "$snapshot"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
git checkout-index --all --ignore-skip-worktree-bits --prefix="$snapshot/"

# Keep the Rust dependency cache across checks. Resolve relative overrides
# before entering the snapshot, where the workspace sources are temporary.
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$root/target}
case "$CARGO_TARGET_DIR" in
    /*) ;;
    *) CARGO_TARGET_DIR=$root/$CARGO_TARGET_DIR ;;
esac
cd "$snapshot"

failed=0
check() {
    local name=$1 regenerate=$2
    shift 2
    printf 'Checking %s\n' "$name"
    if ! "$@"; then
        printf '\n%s check failed. Run from the repository root.\n  %s\n' "$name" "$regenerate" >&2
        failed=1
    fi
}

check_build() (
    cd "$1" && bun install --frozen-lockfile && bun run check-build
)

if ((schema)); then
    check 'HTTP JSON Schema' \
        'schema=$(cargo run --locked --quiet -p spool-cli -- schema) && mkdir -p schema &&
  printf "%s\n" "$schema" > schema/spool-api.v1.schema.json && (cd packages/client && bun run generate)' \
        cargo test --locked -p spool-cli --test client -- the_committed_schema_is_the_one_the_server_speaks --exact
fi
if ((types)); then
    check 'TypeScript client types' '(cd packages/client && bun run generate)' \
        bun run --cwd packages/client check-types
fi
if ((studio)); then
    check 'Studio bundle and shared assets' '(cd packages/studio && bun install --frozen-lockfile && bun run build)' \
        check_build packages/studio
fi
if ((help)); then
    help_test=(cargo test --locked -p spool-cli --bin spool -- --exact
        tests::the_help_of_every_command_is_its_snapshot tests::the_command_reference_is_what_the_commands_say)
    check 'CLI help snapshots and command reference' "SNAPSHOTS=overwrite ${help_test[*]}" \
        env SNAPSHOTS=verify "${help_test[@]}"
fi
if ((strategy)); then
    check 'TypeScript strategy helper' '(cd packages/strategy && bun install --frozen-lockfile && bun run build)' \
        check_build packages/strategy
fi

if ((failed)); then
    printf '\nStage the regenerated files and run the check again.\n' >&2
fi
exit "$failed"
