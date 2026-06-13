# syntax=docker/dockerfile:1

# --- Plan dependencies (cargo-chef caches the dependency build layer) ---
FROM lukemathwalker/cargo-chef:latest-rust-1 AS chef
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
# Build & cache dependencies — only re-runs when the recipe (deps) changes.
RUN cargo chef cook --release --recipe-path recipe.json
# Build the app. Templates (askama) and migrations (sqlx::migrate!) are embedded
# into the binary at compile time; only `assets/` is needed at runtime.
COPY . .
RUN cargo build --release --bin golf-booker

# --- Runtime ---
# trixie matches the cargo-chef builder's glibc (the binary needs >= 2.38).
FROM debian:trixie-slim AS runtime
WORKDIR /app

# ca-certificates: rustls verifies the club's TLS cert against the system store.
# curl: used by the container HEALTHCHECK.
RUN apt-get update -y \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /data

COPY --from=builder /app/target/release/golf-booker /usr/local/bin/golf-booker
# Static files served at /assets (CSS is pre-built and committed).
COPY assets ./assets

# SQLite DB lives on a mounted volume so it survives redeploys.
ENV DATABASE_URL="sqlite:/data/golf.db" \
    PORT=8080
EXPOSE 8080
VOLUME ["/data"]

HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD curl -fsS "http://localhost:${PORT}/health" || exit 1

ENTRYPOINT ["/usr/local/bin/golf-booker"]
