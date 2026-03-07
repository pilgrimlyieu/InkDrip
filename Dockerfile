# ─── Build Stage ────────────────────────────────────────────────
FROM rust:1.93-alpine AS builder

RUN apk add --no-cache musl-dev pkgconfig openssl-dev openssl-libs-static

WORKDIR /build

# Copy manifests first for layer caching
COPY Cargo.toml Cargo.lock ./
COPY inkdrip-core/Cargo.toml inkdrip-core/Cargo.toml
COPY inkdrip-store-sqlite/Cargo.toml inkdrip-store-sqlite/Cargo.toml
COPY inkdrip-server/Cargo.toml inkdrip-server/Cargo.toml
COPY inkdrip-cli/Cargo.toml inkdrip-cli/Cargo.toml

# Create stub source files for dependency caching
RUN mkdir -p inkdrip-core/src && echo "pub fn stub() {}" > inkdrip-core/src/lib.rs && \
    mkdir -p inkdrip-store-sqlite/src && echo "pub fn stub() {}" > inkdrip-store-sqlite/src/lib.rs && \
    mkdir -p inkdrip-server/src && echo "fn main() {}" > inkdrip-server/src/main.rs && \
    mkdir -p inkdrip-cli/src && echo "fn main() {}" > inkdrip-cli/src/main.rs

# Build dependencies (cached layer)
RUN cargo build --release 2>/dev/null || true

# Copy actual source
COPY inkdrip-core/src inkdrip-core/src
COPY inkdrip-store-sqlite/src inkdrip-store-sqlite/src
COPY inkdrip-server/src inkdrip-server/src
COPY inkdrip-cli/src inkdrip-cli/src

# Build release binaries
RUN cargo build --release --bin inkdrip-server --bin inkdrip-cli

# ─── Runtime Stage ──────────────────────────────────────────────
FROM alpine:3.21

RUN apk add --no-cache ca-certificates tzdata

COPY --from=builder /build/target/release/inkdrip-server /usr/local/bin/inkdrip-server
COPY --from=builder /build/target/release/inkdrip-cli /usr/local/bin/inkdrip

# Default data directory
VOLUME /data
WORKDIR /data

EXPOSE 8080

ENV INKDRIP__SERVER__HOST=0.0.0.0 \
    INKDRIP__SERVER__PORT=8080 \
    INKDRIP__STORAGE__DATA_DIR=/data \
    INKDRIP_CONFIG=/data/config.toml

CMD ["inkdrip-server"]
