#!/usr/bin/env bash
# Fails when a source file is longer than the budget.
#
#   scripts/check-file-size.sh            checks every tracked source file
#   scripts/check-file-size.sh --update   lowers and removes entries of the list
#
# A file that was over the budget when this check was added is listed in
# scripts/file-size-allowlist.txt with the length it may not pass. The list
# only shrinks: `--update` lowers a number and removes an entry, and never
# raises one or adds one. A file that outgrows the budget is split by what it
# is about (AGENTS.md, rule 4).
#
# Written for the bash of macOS as well, which is 3.2.
set -eu

budget=800
root=$(git rev-parse --show-toplevel)
cd "$root"
list=scripts/file-size-allowlist.txt

update=0
if [[ ${1:-} == --update && $# == 1 ]]; then
    update=1
elif (($#)); then
    echo 'Usage: check-file-size.sh [--update]' >&2
    exit 2
fi

# Source that people write, each with its length: not data, and not what a
# build wrote.
sizes() {
    git ls-files -- '*.rs' '*.ts' '*.tsx' '*.py' '*.sh' ':!benchmarks/' ':!site/video/' |
        git check-attr --stdin linguist-generated |
        awk -F': linguist-generated: ' '$2 != "true" && $2 != "set" { print $1 }' |
        while IFS= read -r path; do
            [[ -f $path ]] || continue
            printf '%s %s\n' "$(($(wc -l < "$path")))" "$path"
        done
}

kept=$(mktemp)
trap 'rm -f -- "$kept"' EXIT
touch "$list"

status=0
sizes | awk -v budget="$budget" -v update="$update" -v list="$list" -v kept="$kept" '
    # The list first: the most lines each of its files may have.
    FILENAME == list {
        if ($0 ~ /^#/ || NF < 2) next
        most = $1
        sub(/^[0-9]+ /, "")
        allowed[$0] = most
        next
    }
    {
        lines = $1
        sub(/^[0-9]+ /, "")
        path = $0
        seen[path] = 1
        if (!(path in allowed)) {
            if (lines > budget) {
                printf "%s has %d lines, and the budget is %d: split it by what it is about\n", path, lines, budget > "/dev/stderr"
                failed = 1
            }
            next
        }
        most = allowed[path]
        if (lines <= budget) {
            if (update) printf "%s is within the budget now: removed from the list\n", path
            else {
                printf "%s has %d lines, within the budget: remove it from %s (--update does)\n", path, lines, list > "/dev/stderr"
                failed = 1
            }
        } else if (lines > most) {
            printf "%s has %d lines, and the list allows it %d: it may only get shorter\n", path, lines, most > "/dev/stderr"
            failed = 1
            print most, path > kept
        } else if (lines < most && update) {
            printf "%s: %d to %d\n", path, most, lines
            print lines, path > kept
        } else {
            print most, path > kept
        }
    }
    END {
        for (path in allowed) {
            if (path in seen) continue
            if (update) printf "%s is gone: removed from the list\n", path
            else {
                printf "%s is on %s and is not a source file any more: remove it (--update does)\n", path, list > "/dev/stderr"
                failed = 1
            }
        }
        exit failed
    }
' "$list" - || status=$?

if ((update)); then
    {
        echo "# Source files over the budget of scripts/check-file-size.sh, each with the"
        echo "# most lines it may have. This list only shrinks: see the script."
        sort -k2 "$kept"
    } > "$list"
fi
exit "$status"
