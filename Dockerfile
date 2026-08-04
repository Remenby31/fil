# syntax=docker/dockerfile:1.7
FROM rust:1.95-slim AS builder

RUN apt-get update && apt-get install -y protobuf-compiler pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY . .
RUN --mount=type=cache,id=fil-cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=fil-cargo-git,target=/usr/local/cargo/git \
    --mount=type=cache,id=fil-target,target=/app/target \
    cargo build --release -p fil-hub && \
    cp /app/target/release/fil-hub /tmp/fil-hub

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*

COPY --from=builder /tmp/fil-hub /usr/local/bin/fil-hub

RUN mkdir -p /data
ENV DATABASE_URL=sqlite:/data/fil-hub.db?mode=rwc

EXPOSE 3100
EXPOSE 16433/udp
VOLUME ["/data"]

CMD ["fil-hub"]
