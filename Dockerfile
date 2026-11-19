# The Spool base image: the `spool` binary and the DuckDB library it loads.
#
# A project's own Dockerfile, which `spool build` writes, starts from this
# image, compiles the project into a manifest and runs `spool serve`.
#
#   docker build -t ghcr.io/alexpatow/spool:0.2.0 .
#
# The `archive` stage combines the two release files with the man pages
# and completions they write.

# The compiler runs on the architecture of the machine that builds, whichever
# one the image is for, and cross-compiles when the two differ. An image for
# arm64 built on x86-64 would otherwise compile under emulation, for hours.
FROM --platform=$BUILDPLATFORM rust:1.89-slim-bookworm AS build
ARG BUILDARCH
ARG TARGETARCH
WORKDIR /src
RUN case "$TARGETARCH" in \
        amd64) echo x86_64 ;; \
        arm64) echo aarch64 ;; \
        *) echo "Spool has no build for $TARGETARCH" >&2; exit 1 ;; \
    esac > /cpu \
    && rustup target add "$(cat /cpu)-unknown-linux-gnu" \
    && if [ "$BUILDARCH" != "$TARGETARCH" ]; then \
        apt-get update \
        && apt-get install -y --no-install-recommends \
            "gcc-$(tr _ - < /cpu)-linux-gnu" "libc6-dev-$TARGETARCH-cross" \
        && rm -rf /var/lib/apt/lists/*; \
    fi
# Debian names its own C compiler this way too, so these hold either way.
ENV CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
COPY Cargo.toml Cargo.lock ./
COPY crates crates
# The build downloads the DuckDB shared library that matches the `duckdb`
# crate. The binary looks for it beside itself and in ../lib, and with
# SPOOL_RELEASE_BUILD nowhere else.
# Keep just the release files in the build layer that CI caches.
RUN target="$(cat /cpu)-unknown-linux-gnu" \
    && SPOOL_RELEASE_BUILD=1 cargo build --release --locked -p spool-cli --target "$target" \
    && mkdir -p /out/bin /out/lib \
    && cp "target/$target/release/spool" /out/bin/spool \
    && find target -path '*/duckdb-download/*' -name 'libduckdb.so' -exec cp {} /out/lib/ \; \
    && test -f /out/lib/libduckdb.so \
    && rm -rf target "$CARGO_HOME/registry"

# The man pages and the shell completions, which the binary writes from the
# definition of its commands. It runs here, under emulation for the
# architecture the machine that builds is not.
FROM debian:bookworm-slim AS share
COPY --from=build /out/bin/spool /usr/local/bin/spool
COPY --from=build /out/lib/libduckdb.so /usr/local/lib/libduckdb.so
RUN mkdir -p /share/bash-completion/completions /share/zsh/site-functions \
        /share/fish/vendor_completions.d \
    && spool man /share/man/man1 \
    && spool completions bash > /share/bash-completion/completions/spool \
    && spool completions zsh > /share/zsh/site-functions/_spool \
    && spool completions fish > /share/fish/vendor_completions.d/spool.fish

# What a release's archive for Linux is made from: bin/spool,
# lib/libduckdb.so, and the man pages and completions under share/.
FROM scratch AS archive
COPY --from=build /out/ /
COPY --from=share /share /share

# Release CI replaces this stage with the files from the tested archives.
FROM scratch AS release-files
ARG TARGETARCH
COPY --from=build /out/ /${TARGETARCH}/

FROM debian:bookworm-slim
ARG TARGETARCH
LABEL org.opencontainers.image.title="Spool" \
      org.opencontainers.image.description="Extract, resolve and track entities." \
      org.opencontainers.image.source="https://github.com/alexpatow/spool" \
      org.opencontainers.image.licenses="MIT"
# `spool` carries the certificates it trusts. A project's Python code does
# not: it asks the system for them when it calls a model's API or fetches a
# model's weights over HTTPS.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=release-files /${TARGETARCH}/bin/spool /usr/local/bin/spool
COPY --from=release-files /${TARGETARCH}/lib/libduckdb.so /usr/local/lib/libduckdb.so
COPY LICENSE release/LICENSE-duckdb /usr/local/share/doc/spool/
# The server does not need to be root. The store lives on a volume at /data.
RUN useradd --system --uid 10001 --home-dir /data --shell /usr/sbin/nologin spool \
    && mkdir -p /data /spool \
    && chown spool:spool /data
ENTRYPOINT ["spool"]
CMD ["--help"]
