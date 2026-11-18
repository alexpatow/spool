#!/bin/sh
# Runs the start of the README with the `spool` that is first on the search
# path: a new project, a server, the sample rows loaded and matched, and the
# sample note read. It fails unless the records come back as the README says
# they do.
#
#   release/smoke.sh
#
# The extractor `spool init --example companies` writes runs a small model,
# which a first run fetches. A release is not tested with it:
# by-legal-form.py takes its place, and finds the same three companies in
# the note.
#
# An extractor is a Python file, which the server runs with `uv`. Where `uv`
# is not on the search path and SPOOL_UV names none, as in the image, only
# the rows are loaded and matched, and the script says so. With
# SPOOL_SMOKE_NOTE=required it fails there instead.
#
# The server listens on port 7791 of this machine, or on SPOOL_SMOKE_PORT.

set -eu
here=$(cd "$(dirname "$0")" && pwd)

work=$(mktemp -d)
server=
stop() {
    if [ -n "$server" ]; then
        kill "$server" 2> /dev/null || true
        wait "$server" 2> /dev/null || true
    fi
    rm -rf "$work"
}
trap stop EXIT

fail() {
    echo "release/smoke.sh: $*" >&2
    if [ -f "$work/server.log" ]; then
        sed 's/^/  server: /' "$work/server.log" >&2
    fi
    exit 1
}

note=
if command -v "${SPOOL_UV:-uv}" > /dev/null 2>&1; then
    note=1
elif [ "${SPOOL_SMOKE_NOTE:-}" = required ]; then
    fail "\`uv\` is not on the search path, and SPOOL_SMOKE_NOTE=required asks for the note to be read"
fi

port=${SPOOL_SMOKE_PORT:-7791}
SPOOL_URL=http://127.0.0.1:$port
export SPOOL_URL
unset SPOOL_TOKEN

cd "$work"
spool --version
spool init companies --example companies > /dev/null
cd companies
[ -f extractors/companies.py ] || fail "\`spool init --example companies\` wrote no extractors/companies.py for the stand-in to replace"
cp "$here/by-legal-form.py" extractors/companies.py
spool dev --listen "127.0.0.1:$port" > "$work/server.log" 2>&1 &
server=$!

# Under emulation a server takes seconds to start.
tries=0
until spool show > /dev/null 2>&1; do
    tries=$((tries + 1))
    [ "$tries" -lt 120 ] || fail "the server did not answer within a minute"
    kill -0 "$server" 2> /dev/null || fail "the server stopped before it answered"
    sleep 0.5
done

spool load samples/crm-1.csv
spool match
spool load samples/crm-2.csv
spool match
if [ -n "$note" ]; then
    spool extract samples/notes-1.txt
    spool match
fi
records=$(spool show records)
echo "$records"

# The identity of the record whose source and key are $1.
identity() {
    echo "$records" | awk -v key="$1" '$1 == key { print $NF }'
}
count=$(echo "$records" | grep -c '^crm:' || true)
[ "$count" -eq 9 ] || fail "there are $count records of \`crm\`, and the sample files hold 9"
[ -n "$(identity crm:c-1)" ] || fail "crm:c-1 has no identity"
[ "$(identity crm:c-1)" = "$(identity crm:c-2)" ] || fail "crm:c-1 and crm:c-2 are one company and have two identities"
[ "$(identity crm:c-5)" != "$(identity crm:c-6)" ] || fail "crm:c-5 and crm:c-6 are two companies and share an identity"
if [ -z "$note" ]; then
    echo "release/smoke.sh: nine records, matched as the README says. \`uv\` is not on the search path, so the note was not read"
    exit 0
fi

count=$(echo "$records" | grep -c '^notes:' || true)
[ "$count" -eq 3 ] || fail "there are $count records of \`notes\`, and the sample note names 3 companies"
acorn=$(identity 'notes:notes-1.txt[54-69]')
brightwater=$(identity 'notes:notes-1.txt[179-200]')
[ -n "$acorn" ] || fail "the note's Acorn Tools Ltd has no identity"
[ "$acorn" = "$(identity crm:c-1)" ] || fail "the note's Acorn Tools Ltd is not in the identity of crm:c-1"
[ -n "$brightwater" ] || fail "the note's Brightwater Foods Ltd has no identity"
if echo "$records" | grep '^crm:' | grep -q "$brightwater"; then
    fail "the note's Brightwater Foods Ltd, which no row has, shares an identity with a row"
fi
echo "release/smoke.sh: twelve records, matched as the README says"
