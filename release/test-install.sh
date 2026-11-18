#!/bin/sh
# Installs Spool from a directory of archives the way a user installs it
# from a release, and checks what that leaves.
#
#   release/test-install.sh [<directory>]
#   release/test-install.sh <directory> <image> [<platform>]
#
# <directory> is `dist` unless given, and holds archives made by
# release/archive.sh. They are served, with install.sh, at an address on
# this machine, as a release's files are on GitHub, and install.sh is fetched
# from there and piped to `sh`. That happens on this machine, or with an
# <image> in a container of it, which has to have `curl`; <platform> is one
# for `docker run --platform`, as in linux/arm64.
#
# It checks that the release is unpacked and linked where the README says,
# that a second run replaces it, that an archive which is not the one
# SHA256SUMS lists is refused and leaves nothing, that a file in the place of
# the link is left alone, and then runs release/smoke.sh with what it
# installed.

set -eu
here=$(cd "$(dirname "$0")" && pwd)

fail() {
    echo "release/test-install.sh: $*" >&2
    exit 1
}

# The checks themselves, which run where SPOOL_TEST_URL names the server.
if [ -n "${SPOOL_TEST_URL:-}" ]; then
    url=$SPOOL_TEST_URL
    unset SPOOL_VERSION SPOOL_INSTALL_DIR SPOOL_BIN_DIR SPOOL_DOWNLOAD_URL
    homes=$(mktemp -d)
    trap 'rm -rf "$homes"' EXIT
    cd "$homes"

    install() {
        curl -fsSL "$url/install.sh" | HOME=$1 SPOOL_DOWNLOAD_URL=$2 sh
    }
    # Fails unless a directory holds exactly the names that follow it.
    holds() {
        # shellcheck disable=SC2012
        found=$(ls -A "$1" | sort | tr '\n' ' ')
        where=$1
        shift
        [ "$found" = "$(printf '%s\n' "$@" | sort | tr '\n' ' ')" ] || fail "$where holds \`$found\` and should hold \`$*\`"
    }
    version=$(curl -fsSL "$url/install.sh" | sed -n 's/^VERSION=//p')

    echo "== a first install"
    mkdir first
    install "$homes/first" "$url"
    release=$homes/first/.local/share/spool/$version
    link=$homes/first/.local/bin/spool
    holds "$homes/first" .local
    holds "$homes/first/.local" bin share
    holds "$homes/first/.local/bin" spool
    holds "$homes/first/.local/share" spool
    holds "$homes/first/.local/share/spool" "$version"
    holds "$release" bin lib LICENSE LICENSE-duckdb
    holds "$release/bin" spool
    [ -f "$release/lib/libduckdb.so" ] || [ -f "$release/lib/libduckdb.dylib" ] || fail "$release/lib holds no DuckDB library"
    [ -L "$link" ] || fail "$link is not a link"
    [ "$("$link" --version)" = "spool $version" ] || fail "$link --version does not say \`spool $version\`"

    echo "== a second install over the first"
    echo "not the release" > "$release/stale"
    install "$homes/first" "$url"
    holds "$homes/first/.local/share/spool" "$version"
    holds "$release" bin lib LICENSE LICENSE-duckdb
    [ "$("$link" --version)" = "spool $version" ] || fail "$link does not run after a second install"

    echo "== an archive that is not the one SHA256SUMS lists"
    mkdir tampered
    if install "$homes/tampered" "$url/tampered" 2> tampered.log; then
        fail "an archive with another checksum was installed"
    fi
    sed 's/^/  /' tampered.log
    grep -q "is not the file the release lists" tampered.log || fail "the refusal does not say that the checksum differs"
    left=$(find "$homes/tampered" -mindepth 1 ! -type d)
    [ -z "$left" ] || fail "a refused install left files: $left"

    echo "== a file where the link goes"
    mkdir -p occupied/.local/bin
    echo "someone's own" > occupied/.local/bin/spool
    if install "$homes/occupied" "$url" 2> occupied.log; then
        fail "a file at ~/.local/bin/spool was replaced"
    fi
    sed 's/^/  /' occupied.log
    [ "$(cat occupied/.local/bin/spool)" = "someone's own" ] || fail "a file at ~/.local/bin/spool was changed"
    holds "$homes/occupied/.local" bin

    echo "== the start of Getting started, with what was installed"
    PATH=$homes/first/.local/bin:$PATH HOME=$homes/first sh "$here/smoke.sh"
    echo "release/test-install.sh: passed"
    exit 0
fi

dir=${1:-dist}
image=${2:-}
platform=${3:-}
ls "$dir"/spool-*.tar.gz > /dev/null 2>&1 || fail "$dir holds no archives: release/archive.sh makes them"

served=$(mktemp -d)
server=
stop() {
    [ -z "$server" ] || kill "$server" 2> /dev/null || true
    rm -rf "$served"
}
trap stop EXIT

# What a release holds, and beside it one whose archives are not the ones
# its SHA256SUMS lists.
mkdir "$served/tampered"
cp "$here/../install.sh" "$dir"/spool-*.tar.gz "$served/"
cat "$dir"/spool-*.tar.gz.sha256 > "$served/SHA256SUMS"
cp "$served/SHA256SUMS" "$served/tampered/"
for archive in "$served"/spool-*.tar.gz; do
    cp "$archive" "$served/tampered/"
    printf 'x' >> "$served/tampered/$(basename "$archive")"
done

python3 -u -m http.server 0 --bind 127.0.0.1 --directory "$served" > "$served/.server.log" 2>&1 &
server=$!
port=
tries=0
while [ -z "$port" ]; do
    tries=$((tries + 1))
    [ "$tries" -lt 50 ] || fail "the file server did not start: $(cat "$served/.server.log")"
    sleep 0.2
    port=$(sed -n 's/.* port \([0-9][0-9]*\).*/\1/p' "$served/.server.log" | head -n 1)
done
url=http://127.0.0.1:$port

if [ -z "$image" ]; then
    echo "== installing from $dir on this machine"
    env -i PATH="$PATH" SPOOL_TEST_URL="$url" sh "$here/test-install.sh"
else
    echo "== installing from $dir in a container of $image${platform:+, $platform}"
    # The container shares this machine's network, to reach the file server.
    # shellcheck disable=SC2086
    docker run --rm --network host ${platform:+--platform $platform} \
        --env SPOOL_TEST_URL="$url" --volume "$here:/release:ro" \
        "$image" sh /release/test-install.sh
fi
