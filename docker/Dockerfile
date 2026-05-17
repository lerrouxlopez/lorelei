# Release image for The Reef.
# - Builds `lorelei-harbor` + `lore` (CLI) in a multi-stage build.
# - Runtime image contains no Rust toolchain.

FROM rust:1.88-bookworm AS build
WORKDIR /app

# Cache deps first
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY prompts ./prompts

RUN cargo build --release -p lorelei-harbor -p lorelei-cli

FROM debian:bookworm-slim AS runtime
WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates wget && rm -rf /var/lib/apt/lists/*

COPY --from=build /app/target/release/lorelei-harbor /usr/local/bin/lorelei-harbor
COPY --from=build /app/target/release/lore /usr/local/bin/lore

# Aliases for convenience.
RUN ln -sf /usr/local/bin/lore /usr/local/bin/lori

# Ship prompts + migrations for transparency/debugging (even though they are embedded at build time).
COPY prompts /app/prompts
COPY crates/lorelei-harbor/migrations /app/migrations
COPY lorelei.toml.example /app/lorelei.toml

EXPOSE 8080
CMD ["lorelei-harbor", "serve"]
