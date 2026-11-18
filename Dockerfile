# The Spool base image: the `spool` binary and the DuckDB library it loads.
#
# A project's own Dockerfile, which `spool build` writes, starts from this
# image, compiles the project into a manifest and runs `spool serve`.
#
#   docker build -t ghcr.io/alexpatow/spool:0.1.0 .
#
# A release's archives for Linux hold the two files this builds, and
# `release/linux.sh` takes them from the `archive` stage.

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
RUN target="$(cat /cpu)-unknown-linux-gnu" \
    && SPOOL_RELEASE_BUILD=1 cargo build --release --locked -p spool-cli --target "$target" \
    && mkdir -p /out/bin /out/lib \
    && cp "target/$target/release/spool" /out/bin/spool \
    && find target -path '*/duckdb-download/*' -name 'libduckdb.so' -exec cp {} /out/lib/ \; \
    && test -f /out/lib/libduckdb.so

# What a release's archive for Linux is made from: bin/spool and lib/libduckdb.so.
FROM scratch AS archive
COPY --from=build /out/ /

FROM debian:bookworm-slim
LABEL org.opencontainers.image.title="Spool" \
      org.opencontainers.image.description="Extract, resolve and track entities." \
      org.opencontainers.image.source="https://github.com/alexpatow/spool" \
      org.opencontainers.image.licenses="MIT"
COPY --from=build /out/bin/spool /usr/local/bin/spool
COPY --from=build /out/lib/libduckdb.so /usr/local/lib/libduckdb.so
COPY LICENSE release/LICENSE-duckdb /usr/local/share/doc/spool/
# The server does not need to be root. The store lives on a volume at /data.
RUN useradd --system --uid 10001 --home-dir /data --shell /usr/sbin/nologin spool \
    && mkdir -p /data /spool \
    && chown spool:spool /data
ENTRYPOINT ["spool"]
CMD ["--help"]
