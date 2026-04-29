# =============================================================================
# gasket Dockerfile - Multi-platform (Rust)
# =============================================================================
# Build target: rust (default)
# Usage:
#   docker build -t gasket .
# =============================================================================

# -----------------------------------------------------------------------------
# Stage 1: Rust Builder
# -----------------------------------------------------------------------------
FROM rust:1.82-bookworm AS rust-builder

WORKDIR /build

# Install build dependencies (protoc is required by lance-encoding -> prost-build)
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        pkg-config \
        libssl-dev \
        protobuf-compiler && \
    rm -rf /var/lib/apt/lists/*

# Copy workspace root files for dependency caching
COPY gasket/Cargo.toml gasket/Cargo.lock ./

# Copy all workspace member Cargo.toml files
COPY gasket/types/Cargo.toml ./types/
COPY gasket/storage/Cargo.toml ./storage/
COPY gasket/embedding/Cargo.toml ./embedding/
COPY gasket/broker/Cargo.toml ./broker/
COPY gasket/engine/Cargo.toml ./engine/
COPY gasket/cli/Cargo.toml ./cli/
COPY gasket/providers/Cargo.toml ./providers/
COPY gasket/channels/Cargo.toml ./channels/
COPY gasket/sandbox/Cargo.toml ./sandbox/
COPY gasket/wiki/Cargo.toml ./wiki/

# Create dummy source files so cargo can build dependencies layer
RUN mkdir -p \
        types/src \
        storage/src \
        embedding/src \
        broker/src \
        engine/src \
        cli/src \
        providers/src \
        channels/src \
        sandbox/src \
        wiki/src && \
    echo "pub fn dummy() {}" > types/src/lib.rs && \
    echo "pub fn dummy() {}" > storage/src/lib.rs && \
    echo "pub fn dummy() {}" > embedding/src/lib.rs && \
    echo "pub fn dummy() {}" > broker/src/lib.rs && \
    echo "pub fn dummy() {}" > engine/src/lib.rs && \
    echo "fn main() {}" > cli/src/main.rs && \
    echo "pub fn dummy() {}" > providers/src/lib.rs && \
    echo "pub fn dummy() {}" > channels/src/lib.rs && \
    echo "pub fn dummy() {}" > sandbox/src/lib.rs && \
    echo "pub fn dummy() {}" > wiki/src/lib.rs && \
    cargo build --release --all-features && \
    rm -rf \
        types/src \
        storage/src \
        embedding/src \
        broker/src \
        engine/src \
        cli/src \
        providers/src \
        channels/src \
        sandbox/src \
        wiki/src

# Copy actual source code
COPY gasket/types/src ./types/src
COPY gasket/storage/src ./storage/src
COPY gasket/embedding/src ./embedding/src
COPY gasket/broker/src ./broker/src
COPY gasket/engine/src ./engine/src
COPY gasket/cli/src ./cli/src
COPY gasket/providers/src ./providers/src
COPY gasket/channels/src ./channels/src
COPY gasket/sandbox/src ./sandbox/src
COPY gasket/wiki/src ./wiki/src

# Touch source files to invalidate cargo cache and rebuild
RUN touch \
        types/src/lib.rs \
        storage/src/lib.rs \
        embedding/src/lib.rs \
        broker/src/lib.rs \
        engine/src/lib.rs \
        cli/src/main.rs \
        providers/src/lib.rs \
        channels/src/lib.rs \
        sandbox/src/lib.rs \
        wiki/src/lib.rs && \
    cargo build --release --all-features

# -----------------------------------------------------------------------------
# Stage 2: Rust Runtime
# -----------------------------------------------------------------------------
FROM debian:bookworm-slim AS rust

# Install runtime dependencies
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        ca-certificates \
        libssl3 && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy binary from builder
COPY --from=rust-builder /build/target/release/gasket /usr/local/bin/gasket

# Create config directory
RUN mkdir -p /root/.gasket

# Gateway default port
EXPOSE 18790

ENTRYPOINT ["gasket"]
CMD ["status"]
