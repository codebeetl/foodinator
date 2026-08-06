# syntax=docker/dockerfile:1
FROM rust:1-slim-bookworm AS builder
WORKDIR /app

# Dependency layer: only invalidated when Cargo.toml/Cargo.lock change, so an
# ordinary source-only change reuses this instead of recompiling every crate.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src \
    && echo "fn main() {}" > src/main.rs \
    && echo "" > src/lib.rs
ENV SQLX_OFFLINE=true
RUN cargo build --release \
    && rm -rf src \
       target/release/deps/foodinator-* \
       target/release/deps/libfoodinator-* \
       target/release/foodinator

# Source layer: only this recompiles on a normal code change.
COPY .sqlx ./.sqlx
COPY src ./src
COPY migrations ./migrations
COPY templates ./templates
RUN cargo build --release

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/foodinator /app/foodinator
COPY --from=builder /app/migrations /app/migrations
COPY static /app/static

EXPOSE 8080
ENTRYPOINT ["/app/foodinator"]
