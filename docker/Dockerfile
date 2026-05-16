# Builds and runs the Lorelei Harbor HTTP API.

FROM rust:1.88-bookworm AS build
WORKDIR /app

# Cache deps first
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

RUN cargo build --release -p lorelei-harbor

FROM debian:bookworm-slim AS runtime
WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates wget && rm -rf /var/lib/apt/lists/*

COPY --from=build /app/target/release/lorelei-harbor /usr/local/bin/lorelei-harbor
COPY lorelei.toml.example /app/lorelei.toml

EXPOSE 8080
CMD ["lorelei-harbor"]
