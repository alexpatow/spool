#!/bin/sh
# Prints the version a release made from this commit has, which is the one in
# Cargo.toml, and fails unless install.sh names the same one.
#
#   release/version.sh [<tag>]
#
# With a tag, it also fails unless the tag is `v` and that version.

set -eu
cd "$(dirname "$0")/.."

version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
installs=$(sed -n 's/^VERSION=//p' install.sh)

if [ -z "$version" ]; then
    echo "release/version.sh: Cargo.toml names no version" >&2
    exit 1
fi
if [ "$installs" != "$version" ]; then
    echo "release/version.sh: Cargo.toml says $version and install.sh installs \`$installs\`: change VERSION in install.sh" >&2
    exit 1
fi
if [ -n "${1:-}" ] && [ "$1" != "v$version" ]; then
    echo "release/version.sh: the tag is $1 and this commit is version $version, whose tag is v$version" >&2
    exit 1
fi
echo "$version"
