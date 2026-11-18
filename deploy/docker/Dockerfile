# Hermes server image (PLAN.md section 18.3). Build from the repository root:
#   docker build -f deploy/docker/Dockerfile -t hermes-server .
# The `builder` stage alone produces the static musl binary at /out/hermes-server.

FROM rust:1.100-alpine3.24 AS builder
RUN apk add --no-cache musl-dev gcc
WORKDIR /src
COPY . .
# Queries are checked against .sqlx/ (no database needed to build).
ENV SQLX_OFFLINE=true
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    target="$(uname -m)-unknown-linux-musl" \
    && cargo build --release --locked -p hermes-server --target "$target" \
    && mkdir /out \
    && cp "target/$target/release/hermes-server" /out/hermes-server

# Prepares /data owned by the runtime user (distroless has no shell to do it there).
FROM alpine:3.24 AS data
RUN mkdir /data && chown 10001:10001 /data

# Same runtime base as Dockerfile.release.
FROM gcr.io/distroless/static-debian13
COPY --from=builder /out/hermes-server /hermes-server
COPY --from=data --chown=10001:10001 /data /data
USER 10001:10001
WORKDIR /data
# Config comes from HERMES__… variables; a /data/hermes.toml is used too if present.
ENV HERMES_CONFIG=/data/hermes.toml \
    HERMES__STORAGE__DATA_DIR=/data
EXPOSE 7700/tcp 7700/udp
VOLUME /data
HEALTHCHECK --interval=30s --timeout=10s --start-period=20s --retries=3 \
    CMD ["/hermes-server", "healthcheck"]
ENTRYPOINT ["/hermes-server", "run"]
