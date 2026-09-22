# syntax=docker/dockerfile:1.7
FROM rust:1.95-slim-bookworm AS builder

RUN apt-get update && apt-get install -y protobuf-compiler pkg-config libssl-dev cmake && rm -rf /var/lib/apt/lists/*

WORKDIR /app
ARG FIL_BUILD_REVISION=development
ENV FIL_BUILD_REVISION=$FIL_BUILD_REVISION
ARG CARGO_BUILD_JOBS=2
ENV CARGO_BUILD_JOBS=$CARGO_BUILD_JOBS
COPY . .
RUN --mount=type=cache,id=fil-cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=fil-cargo-git,target=/usr/local/cargo/git \
    --mount=type=cache,id=fil-target,target=/app/target \
    cargo test --release --locked -p fil-hub && \
    cargo build --release --locked -p fil-hub && \
    cp /app/target/release/fil-hub /tmp/fil-hub

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates libssl3 curl && rm -rf /var/lib/apt/lists/*

COPY --from=builder /tmp/fil-hub /usr/local/bin/fil-hub

RUN mkdir -p /data
ENV DATABASE_URL=sqlite:/data/fil-hub.db?mode=rwc

EXPOSE 3100
EXPOSE 16433/udp
VOLUME ["/data"]

HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD curl --fail --silent http://127.0.0.1:3100/health || exit 1

CMD ["fil-hub"]
