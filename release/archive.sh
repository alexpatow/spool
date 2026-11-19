#!/bin/sh
# Makes one archive of a release, and the line of SHA256SUMS that is its
# checksum:
#
#   <directory>/spool-<version>-<target>.tar.gz
#   <directory>/spool-<version>-<target>.tar.gz.sha256
#
#   release/archive.sh <target> <spool> <libduckdb> [<directory>]
#
# <target> is the Rust target the two files were built for, and <directory>
# is `dist` unless given. The archive holds bin/spool, lib/libduckdb.so or
# lib/libduckdb.dylib, the licences of both, and under share/ the man pages
# and the completions for bash, zsh and fish. Those are taken from a share/
# beside the binary's directory where the build left one, as the Linux build
# does, and are otherwise written by the binary, which then has to run here.
#
# It refuses a binary that would not run where it is unpacked: one built for
# another machine, one that looks for the library anywhere but beside itself
# and in ../lib, and on Linux one that needs a newer C library than
# install.sh asks for.

set -eu

if [ $# -lt 3 ]; then
    sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//' >&2
    exit 2
fi
target=$1
binary=$2
library=$3
out=${4:-dist}

here=$(cd "$(dirname "$0")" && pwd)
version=$("$here/version.sh")
name=spool-$version-$target

fail() {
    echo "release/archive.sh: $*" >&2
    exit 1
}

# Compares two versions of the form 2.34: is the first newer than the second?
newer() {
    [ "${1%%.*}" -gt "${2%%.*}" ] || { [ "${1%%.*}" -eq "${2%%.*}" ] && [ "${1#*.}" -gt "${2#*.}" ]; }
}

[ -f "$binary" ] || fail "$binary is not a file"
[ -f "$library" ] || fail "$library is not a file"

case "$target" in
    x86_64-unknown-linux-gnu) machine="Advanced Micro Devices X86-64" ;;
    aarch64-unknown-linux-gnu) machine="AArch64" ;;
    aarch64-apple-darwin) machine= ;;
    *) fail "a release has no archive for $target" ;;
esac

case "$target" in
    *-linux-gnu)
        [ "$(basename "$library")" = libduckdb.so ] || fail "the library for $target is libduckdb.so, not $library"
        for file in "$binary" "$library"; do
            readelf -h "$file" | grep -q "Machine: *$machine\$" || fail "$file was not built for $target"
        done
        search=$(readelf -d "$binary" | sed -n 's/.*(RUNPATH).*\[\(.*\)\]$/\1/p; s/.*(RPATH).*\[\(.*\)\]$/\1/p')
        # shellcheck disable=SC2016
        [ "$search" = '$ORIGIN:$ORIGIN/../lib' ] ||
            fail "$binary looks for its library in \`$search\`: build it with SPOOL_RELEASE_BUILD=1"
        asked=$(sed -n 's/^GLIBC=//p' "$here/../install.sh")
        for file in "$binary" "$library"; do
            needs=$(readelf -W --dyn-syms "$file" | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/GLIBC_//' | sort -t . -k 1,1n -k 2,2n | tail -n 1)
            if newer "$needs" "$asked"; then
                fail "$file needs version $needs of the GNU C library and install.sh asks for $asked: change GLIBC in install.sh"
            fi
        done
        ;;
    *-apple-darwin)
        [ "$(basename "$library")" = libduckdb.dylib ] || fail "the library for $target is libduckdb.dylib, not $library"
        lipo "$binary" -verify_arch arm64 || fail "$binary was not built for $target"
        lipo "$library" -verify_arch arm64 || fail "$library was not built for $target"
        search=$(otool -l "$binary" | awk '$1 == "path" { print $2 }' | paste -s -d : -)
        [ "$search" = '@executable_path:@executable_path/../lib' ] ||
            fail "$binary looks for its library in \`$search\`: build it with SPOOL_RELEASE_BUILD=1"
        ;;
esac

mkdir -p "$out"
out=$(cd "$out" && pwd)
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT

mkdir -p "$stage/$name/bin" "$stage/$name/lib"
cp "$binary" "$stage/$name/bin/spool"
cp "$library" "$stage/$name/lib/"
cp "$here/../LICENSE" "$stage/$name/LICENSE"
cp "$here/LICENSE-duckdb" "$stage/$name/LICENSE-duckdb"
share=$stage/$name/share
if [ -d "$(dirname "$binary")/../share/man/man1" ]; then
    cp -R "$(dirname "$binary")/../share" "$share"
else
    mkdir -p "$share/bash-completion/completions" "$share/zsh/site-functions" "$share/fish/vendor_completions.d"
    "$stage/$name/bin/spool" man "$share/man/man1" ||
        fail "$binary does not run here, so it cannot write its man pages: build them beside it, in ../share"
    "$stage/$name/bin/spool" completions bash > "$share/bash-completion/completions/spool"
    "$stage/$name/bin/spool" completions zsh > "$share/zsh/site-functions/_spool"
    "$stage/$name/bin/spool" completions fish > "$share/fish/vendor_completions.d/spool.fish"
fi
for file in man/man1/spool.1 bash-completion/completions/spool zsh/site-functions/_spool fish/vendor_completions.d/spool.fish; do
    [ -s "$share/$file" ] || fail "share/$file is missing or empty"
done
chmod 755 "$stage/$name" "$stage/$name/bin" "$stage/$name/lib" "$stage/$name/bin/spool" "$stage/$name/lib/"*
chmod 644 "$stage/$name/LICENSE" "$stage/$name/LICENSE-duckdb"
find "$share" -type d -exec chmod 755 {} +
find "$share" -type f -exec chmod 644 {} +

# The files belong to nobody in particular: whoever unpacks them as root
# gets files that are root's, and not those of a user of the machine that
# built them. The plain `ustar` form holds nothing but the files, which is
# what keeps a Mac's attributes out, and COPYFILE_DISABLE its `._` files.
if tar --version 2> /dev/null | grep -q GNU; then
    owner="--owner=0 --group=0 --numeric-owner"
else
    owner="--uid 0 --gid 0 --numeric-owner"
fi
# shellcheck disable=SC2086
COPYFILE_DISABLE=1 tar --format ustar $owner -czf "$out/$name.tar.gz" -C "$stage" "$name"

cd "$out"
if command -v sha256sum > /dev/null 2>&1; then
    sha256sum "$name.tar.gz" > "$name.tar.gz.sha256"
else
    shasum -a 256 "$name.tar.gz" > "$name.tar.gz.sha256"
fi
cat "$name.tar.gz.sha256"
