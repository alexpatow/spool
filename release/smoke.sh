#!/bin/sh
# Runs the start of Getting started with the `spool` that is first on the
# search path: a new project, a server, the sample rows loaded and matched.
# It fails unless the nine rows come back as the README says they do.
#
#   release/smoke.sh
#
# The server listens on port 7791 of this machine, or on SPOOL_SMOKE_PORT.

set -eu

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

port=${SPOOL_SMOKE_PORT:-7791}
SPOOL_URL=http://127.0.0.1:$port
export SPOOL_URL
unset SPOOL_TOKEN

cd "$work"
spool --version
spool init companies > /dev/null
cd companies
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
records=$(spool show records)
echo "$records"

identity() {
    echo "$records" | awk -v key="crm:$1" '$1 == key { print $NF }'
}
count=$(echo "$records" | grep -c '^crm:' || true)
[ "$count" -eq 9 ] || fail "there are $count records of \`crm\`, and the sample files hold 9"
[ -n "$(identity c-1)" ] || fail "crm:c-1 has no identity"
[ "$(identity c-1)" = "$(identity c-2)" ] || fail "crm:c-1 and crm:c-2 are one company and have two identities"
[ "$(identity c-5)" != "$(identity c-6)" ] || fail "crm:c-5 and crm:c-6 are two companies and share an identity"
echo "release/smoke.sh: nine records, matched as the README says"
