# Production image for the cloud server (crates/server).
# Build from the REPO ROOT:  docker build -f deploy/Dockerfile -t stake-dev-server .
#
# Three stages: node builds the two SvelteKit apps (ui/ is embedded into the
# lgs crate at compile time; web/ is served at runtime via SERVER_WEB_DIR),
# rust builds the release binary (migrations are embedded by sqlx::migrate!),
# and a slim runtime carries just the binary + the dashboard build.

FROM node:20-alpine AS frontend
# Pinned major: corepack would fetch the latest pnpm (11+), which requires
# Node 22 built-ins and crashes on node:20.
RUN npm i -g pnpm@10
WORKDIR /repo
COPY pnpm-workspace.yaml package.json pnpm-lock.yaml ./
COPY ui/package.json ui/
COPY web/package.json web/
RUN pnpm install --frozen-lockfile --filter ui --filter web
COPY ui/ ui/
COPY web/ web/
RUN pnpm --filter ui build && pnpm --filter web build

FROM rust:1-bookworm AS backend
WORKDIR /repo
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
# lgs embeds ui/build via include_dir in release builds.
COPY --from=frontend /repo/ui/build ui/build
RUN cargo build --release -p server

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=backend /repo/target/release/server /usr/local/bin/server
COPY --from=frontend /repo/web/build /app/web
# Pre-owned by the runtime user so the blobdata volume (fs storage backend)
# initializes writable — named volumes copy ownership from the image path.
RUN mkdir -p /app/data && chown -R nobody /app/data
ENV SERVER_WEB_DIR=/app/web \
    SERVER_BIND_ADDR=0.0.0.0:8080
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --retries=3 \
    CMD curl -fsS http://127.0.0.1:8080/healthz || exit 1
USER nobody
CMD ["server"]
