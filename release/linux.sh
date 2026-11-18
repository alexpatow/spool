#!/bin/sh
# Builds what a release holds for Linux, on x86-64 and on arm64, with Docker:
#
#   <directory>/spool-<version>-x86_64-unknown-linux-gnu.tar.gz
#   <directory>/spool-<version>-aarch64-unknown-linux-gnu.tar.gz
#   <directory>/spool-image.tar   the image for both, as an OCI archive
#
#   release/linux.sh [<directory>]
#
# <directory> is `dist` unless given. The archives and the image come from
# one build of the Dockerfile at the root, so both hold the same two files.
#
# The image's last step runs under emulation for the architecture this
# machine is not, which Docker has to be told how to do once:
#
#   docker run --privileged --rm tonistiigi/binfmt --install arm64

set -eu
cd "$(dirname "$0")/.."
out=${1:-dist}
version=$(release/version.sh)

# A builder of its own: Docker's default one builds for one platform at a
# time and writes no OCI archive.
builder=spool-release
docker buildx inspect "$builder" > /dev/null 2>&1 ||
    docker buildx create --name "$builder" --driver docker-container > /dev/null

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT

docker buildx build --builder "$builder" --platform linux/amd64,linux/arm64 \
    --target archive --output "type=local,dest=$stage" .
release/archive.sh x86_64-unknown-linux-gnu "$stage/linux_amd64/bin/spool" "$stage/linux_amd64/lib/libduckdb.so" "$out"
release/archive.sh aarch64-unknown-linux-gnu "$stage/linux_arm64/bin/spool" "$stage/linux_arm64/lib/libduckdb.so" "$out"

# The same build again, which is already done, and the stage after it.
docker buildx build --builder "$builder" --platform linux/amd64,linux/arm64 \
    --provenance=false \
    --label "org.opencontainers.image.version=$version" \
    --label "org.opencontainers.image.revision=$(git rev-parse HEAD)" \
    --output "type=oci,dest=$out/spool-image.tar" .
ls -l "$out"
