# syntax=docker/dockerfile:1

FROM rust:1.86-bookworm AS builder
WORKDIR /app

# Dependencies first for caching
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY migrations ./migrations

RUN cargo build --release -p lorelei-harbor --bin lorelei-harbor

# Migrator image (has sqlx-cli; not meant to be minimal)
FROM rust:1.86-bookworm AS migrator
WORKDIR /app

RUN cargo install sqlx-cli --no-default-features --features postgres

COPY migrations ./migrations
ENV SQLX_OFFLINE=true

ENTRYPOINT ["sqlx", "migrate", "run"]

# Final runtime image (no Rust toolchain)
FROM debian:bookworm-slim AS runtime
WORKDIR /app

RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/lorelei-harbor /usr/local/bin/lorelei-harbor

ENV RUST_LOG=info
EXPOSE 8080

ENTRYPOINT ["lorelei-harbor"]

