# Base image for final stage (override with: docker build --build-arg BASE_IMAGE=...)
ARG BASE_IMAGE=debian:bookworm-slim

# Use rust:bookworm so the binary is linked against glibc 2.36, matching final image.
ARG RUST_BUILD_IMAGE=rust:bookworm

# When Docker build cannot reach crates.io (DNS/network), use host network:
#   docker build --network=host -t rustfs/operator:dev .

# Stage 1: Generate recipe for dependency caching
FROM ${RUST_BUILD_IMAGE} AS planner
WORKDIR /app
RUN cargo install cargo-chef
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Stage 2: Build dependencies only (cached unless Cargo.lock changes)
FROM ${RUST_BUILD_IMAGE} AS cacher
WORKDIR /app
RUN cargo install cargo-chef
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

# Stage 3: Build the binary
FROM ${RUST_BUILD_IMAGE} AS builder
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
