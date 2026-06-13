FROM rust:1.95-slim AS builder

RUN apt-get update && apt-get install -y protobuf-compiler pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY . .
RUN cargo build --release -p fil-hub

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/fil-hub /usr/local/bin/fil-hub

RUN mkdir -p /data
ENV DATABASE_URL=sqlite:/data/fil-hub.db?mode=rwc

EXPOSE 3100
VOLUME ["/data"]

CMD ["fil-hub"]
