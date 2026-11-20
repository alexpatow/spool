#!/usr/bin/env bash
# SessionStart hook: prepare a fresh checkout (typically a new worktree).
#
# 1. In a linked worktree whose branch has no commits of its own, fast-forward
#    to the latest origin/main. The desktop app cuts worktrees from the cached
#    origin/main without fetching, so new sessions can start behind.
# 2. Install dependencies. The TypeScript packages and the site are one bun
#    workspace, installed from the root; a checkout that already has
#    node_modules is skipped, and `cargo fetch` is a no-op when the registry
#    cache is warm.
#
# Command output goes to stderr; only problems are printed to stdout, which
# Claude Code adds to the session's context.
#
# Everything runs inside main(), called on the last line, so bash has parsed
# the whole file before the fast-forward can rewrite it.
set -u

sync_main() {
    # Only linked worktrees; never move the main checkout's branch.
    [ "$(git rev-parse --git-dir)" != "$(git rev-parse --git-common-dir)" ] || return 0

    if ! git fetch --quiet origin main >&2; then
        notes+=("git fetch origin main failed; the worktree may be behind main")
        return 0
    fi
    # Skip when the branch already has its own commits or is up to date.
    git merge-base --is-ancestor HEAD origin/main || return 0
    [ "$(git rev-parse HEAD)" != "$(git rev-parse origin/main)" ] || return 0

    git merge --ff-only --quiet origin/main >&2 ||
        notes+=("could not fast-forward to origin/main (local changes in the way?)")
}

install_deps() {
    if command -v cargo >/dev/null 2>&1; then
        cargo fetch --locked >&2 || notes+=("cargo fetch --locked failed")
    else
        notes+=("cargo not found on PATH")
    fi

    if command -v bun >/dev/null 2>&1; then
        if [ -f package.json ] && [ ! -d node_modules ]; then
            bun install --frozen-lockfile >&2 ||
                notes+=("bun install --frozen-lockfile failed")
        fi
    else
        notes+=("bun not found on PATH")
    fi
}

main() {
    local input dir root
    input=$(cat)
    dir=$(printf '%s' "$input" | jq -r '.cwd // empty' 2>/dev/null)
    dir=${dir:-${CLAUDE_PROJECT_DIR:-$PWD}}
    root=$(git -C "$dir" rev-parse --show-toplevel 2>/dev/null) || exit 0
    cd "$root" || exit 0

    notes=()
    sync_main
    install_deps

    if [ ${#notes[@]} -gt 0 ]; then
        echo "Worktree setup in $root hit problems:"
        printf -- '- %s\n' "${notes[@]}"
    fi
    exit 0
}

main "$@"
