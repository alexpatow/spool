#!/usr/bin/env bash
# SessionStart hook: install dependencies in a fresh checkout (typically a new
# worktree). Idempotent: packages that already have node_modules are skipped,
# and `cargo fetch` is a no-op when the registry cache is warm.
#
# Install output goes to stderr; only failures are printed to stdout, which
# Claude Code adds to the session's context.
set -u

input=$(cat)
dir=$(printf '%s' "$input" | jq -r '.cwd // empty' 2>/dev/null)
dir=${dir:-${CLAUDE_PROJECT_DIR:-$PWD}}
root=$(git -C "$dir" rev-parse --show-toplevel 2>/dev/null) || exit 0
cd "$root" || exit 0

failures=()

if command -v cargo >/dev/null 2>&1; then
    cargo fetch --locked >&2 || failures+=("cargo fetch --locked")
else
    failures+=("cargo not found on PATH")
fi

if command -v bun >/dev/null 2>&1; then
    for package in packages/client packages/strategy packages/stress packages/studio site; do
        [ -f "$package/package.json" ] || continue
        [ -d "$package/node_modules" ] && continue
        (cd "$package" && bun install --frozen-lockfile >&2) ||
            failures+=("bun install --frozen-lockfile in $package")
    done
else
    failures+=("bun not found on PATH")
fi

if [ ${#failures[@]} -gt 0 ]; then
    echo "Worktree setup in $root did not finish. Failed steps:"
    printf -- '- %s\n' "${failures[@]}"
fi
exit 0
