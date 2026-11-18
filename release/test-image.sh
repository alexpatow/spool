#!/bin/sh
# Pushes the image release/linux.sh built to a registry on this machine, the
# way a release pushes it to ghcr.io, and runs what comes back from there.
#
#   release/test-image.sh [<directory>]
#
# <directory> is `dist` unless given, and holds spool-image.tar. For x86-64
# and for arm64 it checks that the image pulled is for that architecture,
# that `spool --version` names this version, and runs release/smoke.sh in a
# container of it. Then it builds a new project's image from the Dockerfile
# `spool build` writes, which starts from this one, and loads and matches
# rows on the server that image runs.
#
# The registry is a container that listens on port 5005 of this machine, or
# on SPOOL_TEST_REGISTRY_PORT. It and the rest are removed afterwards.

set -eu
here=$(cd "$(dirname "$0")" && pwd)
dir=${1:-dist}
version=$("$here/version.sh")
port=${SPOOL_TEST_REGISTRY_PORT:-5005}
image=127.0.0.1:$port/spool:$version

fail() {
    echo "release/test-image.sh: $*" >&2
    exit 1
}
[ -f "$dir/spool-image.tar" ] || fail "$dir holds no spool-image.tar: release/linux.sh makes it"

project=$(mktemp -d)
server=
registry=$(docker run --detach --publish "127.0.0.1:$port:5000" registry:3)
stop() {
    # The server's store is a volume without a name, which goes with it.
    [ -z "$server" ] || docker rm --force --volumes "$server" > /dev/null
    docker rm --force --volumes "$registry" > /dev/null
    docker image rm --force spool-test-project "$image" > /dev/null 2>&1 || true
    rm -rf "$project"
}
trap stop EXIT
tries=0
until curl -fsS "http://127.0.0.1:$port/v2/" > /dev/null 2>&1; do
    tries=$((tries + 1))
    [ "$tries" -lt 50 ] || fail "the registry did not start"
    sleep 0.2
done

skopeo copy --all --dest-tls-verify=false "oci-archive:$dir/spool-image.tar" "docker://$image"

for platform in linux/amd64 linux/arm64; do
    echo "== $image for $platform"
    docker pull --quiet --platform "$platform" "$image" > /dev/null
    case "$platform" in
        linux/amd64) cpu=x86_64 ;;
        linux/arm64) cpu=aarch64 ;;
    esac
    found=$(docker run --rm --platform "$platform" --entrypoint uname "$image" -m)
    [ "$found" = "$cpu" ] || fail "the image for $platform runs on $found"
    said=$(docker run --rm --platform "$platform" "$image" --version)
    [ "$said" = "spool $version" ] || fail "the image for $platform says \`$said\`, not \`spool $version\`"
    labelled=$(skopeo inspect --tls-verify=false --override-os linux --override-arch "${platform#linux/}" \
        --format '{{ index .Labels "org.opencontainers.image.version" }}' "docker://$image")
    [ "$labelled" = "$version" ] || fail "the image for $platform is labelled as version \`$labelled\`"
    docker run --rm --platform "$platform" --entrypoint sh --volume "$here:/release:ro" "$image" /release/smoke.sh
    docker image rm "$image" > /dev/null
done

# What the image is for: the Dockerfile `spool build` writes starts from it.
echo "== a project's image, from the Dockerfile \`spool build\` writes"
docker run --rm --user "$(id -u):$(id -g)" --volume "$project:/project" --workdir /project --entrypoint sh "$image" \
    -c "spool init companies > /dev/null && spool build companies --base-image $image"
docker build --quiet --tag spool-test-project "$project/companies" > /dev/null
server=$(docker run --detach spool-test-project)
token=$(docker exec "$server" spool token add test 2> /dev/null)
spool() {
    docker exec --interactive --env SPOOL_TOKEN="$token" "$server" spool "$@"
}
tries=0
until spool show > /dev/null 2>&1; do
    tries=$((tries + 1))
    [ "$tries" -lt 50 ] || fail "the project's server did not answer: $(docker logs "$server" 2>&1)"
    sleep 0.2
done
[ "$(docker exec "$server" id -un)" = spool ] || fail "the project's server does not run as the user \`spool\`"
spool load - --source crm < "$project/companies/samples/crm-1.csv"
spool match
count=$(spool show records | grep -c '^crm:' || true)
[ "$count" -eq 6 ] || fail "the project's server holds $count records of \`crm\`, and the file held 6"
echo "release/test-image.sh: passed"
