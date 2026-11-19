#!/bin/sh
# Builds what a release holds for Linux, on x86-64 and on arm64, with Docker:
#
#   <directory>/spool-<version>-x86_64-unknown-linux-gnu.tar.gz
#   <directory>/spool-<version>-aarch64-unknown-linux-gnu.tar.gz
#   <directory>/spool-image.tar   the image for both, as an OCI archive
#
#   release/linux.sh [--image-only] [<directory>]
#
# <directory> is `dist` unless given. The archives and the image come from
# one build of the Dockerfile at the root, so both hold the same two files.
# With --image-only, the image takes those files from the existing archives.
#
# The Dockerfile runs commands for both architectures. Docker needs
# emulation for whichever differs from the host, for example:
#
#   docker run --privileged --rm tonistiigi/binfmt --install arm64

set -eu
cd "$(dirname "$0")/.."
image_only=false
if [ "${1:-}" = --image-only ]; then
    image_only=true
    shift
fi
out=${1:-dist}
version=$(release/version.sh)

# A builder of its own: Docker's default one builds for one platform at a
# time and writes no OCI archive.
builder=spool-release
docker buildx inspect "$builder" > /dev/null 2>&1 ||
    docker buildx create --name "$builder" --driver docker-container > /dev/null

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT

set --
if [ "$image_only" = true ]; then
    for arch in amd64 arm64; do
        case "$arch" in
            amd64) cpu=x86_64 ;;
            arm64) cpu=aarch64 ;;
        esac
        archive=spool-$version-$cpu-unknown-linux-gnu
        mkdir "$stage/$arch"
        tar -xzf "$out/$archive.tar.gz" -C "$stage/$arch" --strip-components=1 \
            "$archive/bin/spool" "$archive/lib/libduckdb.so"
    done
    set -- --build-context "release-files=$stage"
else
    docker buildx build --builder "$builder" --platform linux/amd64,linux/arm64 \
        --target archive --output "type=local,dest=$stage" .
    release/archive.sh x86_64-unknown-linux-gnu "$stage/linux_amd64/bin/spool" "$stage/linux_amd64/lib/libduckdb.so" "$out"
    release/archive.sh aarch64-unknown-linux-gnu "$stage/linux_arm64/bin/spool" "$stage/linux_arm64/lib/libduckdb.so" "$out"
fi

# The compiler stage is either cached from above or supplied by the archives.
docker buildx build --builder "$builder" --platform linux/amd64,linux/arm64 \
    --provenance=false \
    --label "org.opencontainers.image.version=$version" \
    --label "org.opencontainers.image.revision=$(git rev-parse HEAD)" \
    --output "type=oci,dest=$out/spool-image.tar" "$@" .
ls -l "$out"
