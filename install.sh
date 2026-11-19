#!/bin/sh
# Installs Spool, https://github.com/alexpatow/spool
#
#   curl -fsSL https://github.com/alexpatow/spool/releases/latest/download/install.sh | sh
#
# It downloads the release for this machine, checks it against the release's
# checksums, unpacks it under ~/.local/share/spool/<version> and links
# ~/.local/bin/spool, and links the man pages and the bash, zsh and fish
# completions into ~/.local/share, where `man`, bash and fish look. It needs
# no root and changes nothing else: if ~/.local/bin is not on your search
# path, it says what to add and adds nothing itself.
#
# It reads these from the environment, and none has to be set:
#
#   SPOOL_VERSION       the release to install (default: the one this script
#                       came with)
#   SPOOL_INSTALL_DIR   where releases are unpacked (default: ~/.local/share/spool)
#   SPOOL_BIN_DIR       where `spool` is linked (default: ~/.local/bin). The
#                       man pages and completions are linked under the
#                       share directory beside it
#   SPOOL_DOWNLOAD_URL  where the release's files are, for a copy of them
#                       kept elsewhere (default: the release on GitHub)
#
# To remove Spool:
#
#   rm -rf ~/.local/share/spool ~/.local/bin/spool ~/.local/share/man/man1/spool*.1 \
#     ~/.local/share/bash-completion/completions/spool ~/.local/share/zsh/site-functions/_spool \
#     ~/.local/share/fish/vendor_completions.d/spool.fish

set -eu

# The release this script belongs to. The release workflow refuses a tag
# that this line and Cargo.toml do not both name.
VERSION=0.2.0

# The oldest GNU C library the Linux builds start with.
GLIBC=2.34

fail() {
    printf 'install.sh: %s\n' "$@" >&2
    exit 1
}

need() {
    command -v "$1" > /dev/null 2>&1 || fail "\`$1\` is needed and was not found"
}

# Prints the SHA-256 of a file, with whichever tool this machine has.
sha256() {
    if command -v sha256sum > /dev/null 2>&1; then
        sha256sum "$1" | cut -d ' ' -f 1
    elif command -v shasum > /dev/null 2>&1; then
        shasum -a 256 "$1" | cut -d ' ' -f 1
    else
        openssl dgst -sha256 -r "$1" | cut -d ' ' -f 1
    fi
}

fetch() {
    case "$1" in
        # A redirect may not leave HTTPS.
        https://*) curl --fail --silent --show-error --location --retry 2 --proto '=https' --tlsv1.2 --output "$2" "$1" ;;
        *) curl --fail --silent --show-error --location --retry 2 --output "$2" "$1" ;;
    esac
}

# The name of this machine's build, which is also the name of its Rust target.
target() {
    os=$(uname -s)
    cpu=$(uname -m)
    case "$os" in
        Linux)
            case "$cpu" in
                x86_64 | amd64) echo x86_64-unknown-linux-gnu ;;
                aarch64 | arm64) echo aarch64-unknown-linux-gnu ;;
                *) fail "there is no build of Spool for Linux on $cpu, only for x86-64 and arm64" ;;
            esac
            ;;
        Darwin)
            case "$cpu" in
                arm64) echo aarch64-apple-darwin ;;
                x86_64)
                    # A shell that Rosetta runs on Apple silicon says x86_64.
                    if [ "$(sysctl -n sysctl.proc_translated 2> /dev/null || true)" = 1 ]; then
                        echo aarch64-apple-darwin
                    else
                        fail "there is no build of Spool for an Intel Mac, only for Apple silicon"
                    fi
                    ;;
                *) fail "there is no build of Spool for macOS on $cpu, only for Apple silicon" ;;
            esac
            ;;
        *) fail "there is no build of Spool for $os, only for Linux and macOS" ;;
    esac
}

# The Linux builds are linked against the GNU C library. Said here, because
# what the system says when the binary does not start names neither.
check_libc() {
    if (ldd --version 2>&1 || true) | grep -qi musl; then
        fail "this system's C library is musl, as on Alpine, and Spool is built for the GNU C library." \
            "The container image runs anywhere Docker does: ghcr.io/alexpatow/spool:$version"
    fi
    found=$(getconf GNU_LIBC_VERSION 2> /dev/null | cut -d ' ' -f 2) || found=
    case "$found" in
        [0-9]*.[0-9]*) ;;
        # Not one this can read. Whether the binary starts is checked below.
        *) return 0 ;;
    esac
    if [ "${found%%.*}" -lt "${GLIBC%%.*}" ] ||
        { [ "${found%%.*}" -eq "${GLIBC%%.*}" ] && [ "${found#*.}" -lt "${GLIBC#*.}" ]; }; then
        fail "this system has version $found of the GNU C library, and Spool needs $GLIBC or later." \
            "The container image runs anywhere Docker does: ghcr.io/alexpatow/spool:$version"
    fi
}

# Links the man pages and the completions a release holds under <from> into
# <to>, where `man`, bash and fish find them. A file there that is not a
# link is someone's own and is left alone. A release that holds none, as
# 0.1.0 does not, links nothing.
share_files() {
    [ -d "$1" ] || return 0
    for file in "$1"/man/man1/*.1 "$1/bash-completion/completions/spool" \
        "$1/zsh/site-functions/_spool" "$1/fish/vendor_completions.d/spool.fish"; do
        [ -f "$file" ] || continue
        place=$2${file#"$1"}
        if [ -e "$place" ] && [ ! -L "$place" ]; then
            echo "$place is a file and not a link to a release, so it is left alone"
            continue
        fi
        mkdir -p "$(dirname "$place")"
        ln -sf "$file" "$place"
    done
}

main() {
    [ -n "${HOME:-}" ] || fail "HOME is not set, so there is nowhere to install to"

    version=${SPOOL_VERSION:-$VERSION}
    version=${version#v}
    case "$version" in
        "" | *[!0-9A-Za-z.+-]* | [!0-9]*) fail "\`$version\` is not a version of Spool, which looks like $VERSION" ;;
    esac
    root=${SPOOL_INSTALL_DIR:-$HOME/.local/share/spool}
    bin=${SPOOL_BIN_DIR:-$HOME/.local/bin}
    # The link names the release by its whole path.
    case "$root" in
        /*) ;;
        *) root=$PWD/$root ;;
    esac
    url=${SPOOL_DOWNLOAD_URL:-https://github.com/alexpatow/spool/releases/download/v$version}
    url=${url%/}

    need curl
    need tar
    need uname
    command -v sha256sum > /dev/null 2>&1 || command -v shasum > /dev/null 2>&1 ||
        command -v openssl > /dev/null 2>&1 ||
        fail "one of \`sha256sum\`, \`shasum\` and \`openssl\` is needed to check the download, and none was found"

    target=$(target)
    case "$target" in
        *-linux-*) check_libc ;;
    esac
    name=spool-$version-$target
    link=$bin/spool
    if [ -e "$link" ] && [ ! -L "$link" ]; then
        fail "$link is a file and not a link to a release, so it is left alone." \
            "Move it away and run this again, or set SPOOL_BIN_DIR to another directory."
    fi

    # Everything is made in a directory beside where it will stay, and moved
    # there once it has been checked. A run that fails leaves no file behind.
    mkdir -p "$root"
    work=$(mktemp -d "$root/.install.XXXXXX")
    trap 'rm -rf "$work"; rmdir "$root" 2> /dev/null || true' EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM

    echo "downloading $name.tar.gz"
    fetch "$url/$name.tar.gz" "$work/$name.tar.gz" ||
        fail "could not download $url/$name.tar.gz" \
            "The releases are listed at https://github.com/alexpatow/spool/releases"
    fetch "$url/SHA256SUMS" "$work/SHA256SUMS" ||
        fail "could not download $url/SHA256SUMS, so the download is not checked and not installed"

    expected=$(awk -v file="$name.tar.gz" '$2 == file || $2 == "*" file { print $1; exit }' "$work/SHA256SUMS")
    [ -n "$expected" ] || fail "the release's SHA256SUMS does not list $name.tar.gz, so it is not installed"
    actual=$(sha256 "$work/$name.tar.gz")
    if [ "$actual" != "$expected" ]; then
        fail "$name.tar.gz is not the file the release lists, so it is not installed." \
            "  SHA-256 of the download   $actual" \
            "  SHA-256 in SHA256SUMS     $expected"
    fi

    tar -xzf "$work/$name.tar.gz" -C "$work"
    if [ ! -f "$work/$name/bin/spool" ] || [ ! -d "$work/$name/lib" ]; then
        fail "$name.tar.gz does not hold bin/spool and lib, so it is not installed"
    fi

    # The binary loads the DuckDB library from ../lib, so this also says
    # whether the two start together on this machine.
    said=$("$work/$name/bin/spool" --version 2>&1 < /dev/null) ||
        fail "the release does not start on this machine, so it is not installed. It said:" "  $said"
    [ "$said" = "spool $version" ] ||
        fail "the release says it is \`$said\`, not \`spool $version\`, so it is not installed"

    rm -rf "${root:?}/$version"
    mv "$work/$name" "$root/$version"
    mkdir -p "$bin"
    ln -sf "$root/$version/bin/spool" "$link"
    share_files "$root/$version/share" "$(dirname "$bin")/share"

    echo
    echo "installed  spool $version"
    echo "release    $root/$version"
    echo "command    $link"
    if [ -L "$(dirname "$bin")/share/man/man1/spool.1" ]; then
        echo "manual     man spool, and completions for bash, zsh and fish"
    fi
    echo
    case ":$PATH:" in
        *":$bin:"*)
            first=$(command -v spool 2> /dev/null || true)
            if [ -n "$first" ] && [ "$first" != "$link" ]; then
                echo "Another \`spool\` comes first on your search path: $first"
                echo
            fi
            ;;
        *)
            echo "$bin is not on your search path. This adds it for this shell:"
            echo
            echo "  export PATH=\"$bin:\$PATH\""
            echo
            echo "Put the same line in your shell's profile to keep it."
            echo
            ;;
    esac
    case "${SHELL:-}" in
        */zsh)
            if [ -L "$(dirname "$bin")/share/zsh/site-functions/_spool" ]; then
                echo "zsh completes \`spool\` once $(dirname "$bin")/share/zsh/site-functions is on its fpath:"
                echo
                echo "  fpath=($(dirname "$bin")/share/zsh/site-functions \$fpath)"
                echo
                echo "Put that line in ~/.zshrc, before \`compinit\`."
                echo
            fi
            ;;
    esac
    echo "Next, a project to try it on: spool init companies --example companies"
    echo "Or an empty one for your own data: spool init <name>"
}

# Nothing runs until the whole script has arrived.
main "$@"
