# Stage 1: Generate recipe file for dependencies
FROM rust AS planner

WORKDIR /app
RUN cargo install cargo-chef
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Stage 2: Build dependencies
FROM rust AS cacher

WORKDIR /app
RUN cargo install cargo-chef
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

# Stage 3: Build
FROM rust AS builder

WORKDIR /app
COPY . .
COPY --from=cacher /app/target target
COPY --from=cacher /usr/local/cargo /usr/local/cargo

RUN cargo build --release

# Stage 4: Final image
FROM gcr.io/distroless/cc-debian13:latest

WORKDIR /app
COPY --from=builder /app/target/release/operator .
CMD ["./operator", "-h"]
