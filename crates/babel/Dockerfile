# Build stage with cargo-chef for dependency caching
FROM rust:1.88.0 AS base

RUN cargo install cargo-chef --version ^0.1

WORKDIR /app

# Planner stage - analyze dependencies
FROM base AS planner
# Copy the entire workspace
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo chef prepare --recipe-path recipe.json --bin babel

# Builder stage - build dependencies then app
FROM base AS builder
COPY --from=planner /app/recipe.json recipe.json

# Build dependencies - this is the cached Docker layer
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    cargo chef cook --release --recipe-path recipe.json

# Copy source code and build application
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    cargo build --release --bin babel

# Runtime stage
FROM debian:bookworm-slim

# Install CA certificates for HTTPS requests
RUN apt-get update && \
    apt-get install -y ca-certificates && \
    rm -rf /var/lib/apt/lists/*

# Copy the binary from builder
COPY --from=builder /app/target/release/babel /usr/local/bin/babel

# Set the entrypoint
ENTRYPOINT ["/usr/local/bin/babel"]
