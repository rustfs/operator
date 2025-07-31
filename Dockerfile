# 使用官方 Rust 镜像
FROM rust:1.88-alpine AS builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM alpine:latest
WORKDIR /app
COPY --from=builder /app/target/release/operator .
CMD ["./operator", "-h"]