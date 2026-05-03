# AIDA Docker Quickstart — single-container build
# Serves REST API + React dashboard on port 8080.
#
# Storage (post EPIC-1-001):
#   The container runs `aida-server` against a directory passed as --data-dir.
#   For git-canonical projects, mount the host's `.aida-store/` (orphan-branch
#   worktree) at /data, e.g.:
#     docker run -v $(pwd)/.aida-store:/data -p 8080:8080 aida
#   The cache rebuilds automatically on first read inside the container.
#   For legacy SQLite-canonical projects, mount the directory containing
#   requirements.db at /data instead.

# =============================================================================
# Stage 1: Build React dashboard
# =============================================================================
FROM node:22-bookworm-slim AS frontend-builder

WORKDIR /build

# Copy shared types first (referenced via @shared alias)
COPY shared/ shared/

# Copy React app sources
COPY aida-web-react/package.json aida-web-react/package-lock.json aida-web-react/
RUN cd aida-web-react && npm ci

COPY aida-web-react/ aida-web-react/
RUN cd aida-web-react && npm run build

# =============================================================================
# Stage 2: Build Rust binaries
# =============================================================================
FROM rust:1.85-bookworm AS rust-builder

# Install protobuf compiler (needed by tonic-build)
RUN apt-get update && apt-get install -y protobuf-compiler && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Copy workspace manifest and lockfile
COPY Cargo.toml Cargo.lock ./

# Copy workspace crates (5 active crates after the 2026-05-02 desktop/web
# extraction — see /docs/archive/ for what was removed).
COPY aida-core/ aida-core/
COPY aida-cli/ aida-cli/
COPY aida-crate/ aida-crate/
COPY aida-server/ aida-server/
COPY aida-generate-types/ aida-generate-types/

# Proto files (needed by build.rs)
COPY proto/ proto/

# Build server and CLI in release mode
RUN cargo build --release -p aida-server -p aida-cli

# =============================================================================
# Stage 3: Runtime
# =============================================================================
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy binaries
COPY --from=rust-builder /build/target/release/aida-server /app/aida-server
COPY --from=rust-builder /build/target/release/aida /app/aida

# Copy React dashboard build output
COPY --from=frontend-builder /build/aida-web-react/dist /app/static

# Make CLI available on PATH
ENV PATH="/app:${PATH}"

# Data directory for SQLite databases
RUN mkdir -p /data
VOLUME /data

EXPOSE 8080

CMD ["/app/aida-server", "--host", "0.0.0.0", "--rest-port", "8080", "--data-dir", "/data", "--static-dir", "/app/static"]
