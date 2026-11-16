# The Spool base image: the `spool` binary and the DuckDB library it loads.
#
# A project's own Dockerfile, which `spool build` writes, starts from this
# image, compiles the project into a manifest and runs `spool serve`.
#
#   docker build -t ghcr.io/alexpatow/spool:0.1.0 .

FROM rust:1.89-slim-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
# The build downloads the DuckDB shared library that matches the `duckdb`
# crate. The binary looks for it in ../lib relative to itself.
RUN cargo build --release --locked -p spool-cli \
    && mkdir -p /out/bin /out/lib \
    && cp target/release/spool /out/bin/spool \
    && find target/duckdb-download -name 'libduckdb.so' -exec cp {} /out/lib/ \; \
    && test -f /out/lib/libduckdb.so

FROM debian:bookworm-slim
COPY --from=build /out/bin/spool /usr/local/bin/spool
COPY --from=build /out/lib/libduckdb.so /usr/local/lib/libduckdb.so
# The server does not need to be root. The store lives on a volume at /data.
RUN useradd --system --uid 10001 --home-dir /data --shell /usr/sbin/nologin spool \
    && mkdir -p /data /spool \
    && chown spool:spool /data
ENTRYPOINT ["spool"]
CMD ["--help"]
