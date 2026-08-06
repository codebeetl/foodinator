# syntax=docker/dockerfile:1
FROM rust:1-slim AS builder
WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY .sqlx ./.sqlx
COPY src ./src
COPY migrations ./migrations
COPY templates ./templates

ENV SQLX_OFFLINE=true
RUN cargo build --release

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/ha-foodinator /app/ha-foodinator
COPY --from=builder /app/migrations /app/migrations
COPY static /app/static

EXPOSE 8080
ENTRYPOINT ["/app/ha-foodinator"]
