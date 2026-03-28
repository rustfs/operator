# Base image for final stage (override with: docker build --build-arg BASE_IMAGE=...)
ARG BASE_IMAGE=debian:bookworm-slim

# Use rust:bookworm so the binary is linked against glibc 2.36, matching final image.
ARG RUST_BUILD_IMAGE=rust:bookworm

# cargo-chef version (pin for reproducible builds; override if needed)
ARG CARGO_CHEF_VERSION=0.1.77

# When Docker build cannot reach crates.io (DNS/network), try:
#   docker build --network=host -t rustfs/operator:dev .
# For China mirrors, mount or COPY a .cargo/config.toml (see docs) before cargo install.

# Shared Cargo settings for slow / flaky networks (applies to all Rust stages)
FROM ${RUST_BUILD_IMAGE} AS rust-base
RUN mkdir -p /usr/local/cargo && \
    printf '%s\n' \
      '[http]' \
      'timeout = 300' \
      'multiplexing = false' \
      '' \
      '[net]' \
      'retry = 10' \
      > /usr/local/cargo/config.toml
ENV CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse

# Install cargo-chef once; planner + cacher only COPY the binary (avoids two slow installs)
FROM rust-base AS cargo-chef-installer
ARG CARGO_CHEF_VERSION
RUN cargo install cargo-chef --version "${CARGO_CHEF_VERSION}"

# Stage 1: Generate recipe for dependency caching
FROM rust-base AS planner
COPY --from=cargo-chef-installer /usr/local/cargo/bin/cargo-chef /usr/local/cargo/bin/cargo-chef
WORKDIR /app
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Stage 2: Build dependencies only (cached unless Cargo.lock changes)
FROM rust-base AS cacher
COPY --from=cargo-chef-installer /usr/local/cargo/bin/cargo-chef /usr/local/cargo/bin/cargo-chef
WORKDIR /app
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

# Stage 3: Build the binary
FROM rust-base AS builder
WORKDIR /app
COPY . .
COPY --from=cacher /app/target target
COPY --from=cacher /usr/local/cargo /usr/local/cargo
RUN cargo build --release

# Final image
FROM ${BASE_IMAGE}

WORKDIR /app
COPY --from=builder /app/target/release/operator .
CMD ["./operator", "-h"]
