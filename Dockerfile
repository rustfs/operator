# 使用官方 Rust 镜像
FROM rust:1.91-alpine AS builder
WORKDIR /app

# Install build dependencies for OpenSSL, git2, and other native libraries
RUN apk add --no-cache \
    musl-dev \
    openssl-dev \
    openssl-libs-static \
    pkgconfig \
    perl \
    make \
    git \
    zlib-dev \
    zlib-static

COPY src Cargo.toml Cargo.lock .

# Use vendored libgit2 to avoid linking issues with Alpine's libgit2
ENV LIBGIT2_NO_VENDOR=0
RUN cargo build --release

FROM alpine:latest
WORKDIR /app

# Install runtime dependencies
RUN apk add --no-cache \
    libgcc \
    openssl \
    ca-certificates

COPY --from=builder /app/target/release/operator .
CMD ["./operator", "-h"]
