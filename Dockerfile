FROM lukemathwalker/cargo-chef:0.1.62-rust-bullseye AS chef
WORKDIR /app

FROM chef as planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef as builder
RUN apt-get update && \
    apt-get install -y pkg-config libssl-dev && \
    apt-get clean && rm -rf /var/lib/apt/lists/*
COPY --from=planner /app/recipe.json recipe.json
# Build dependencies
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --bin cargo-http-registry

FROM debian:bullseye-slim as runner

WORKDIR /app

RUN apt-get update && \
    apt-get install -y git && \
    apt-get clean && rm -rf /var/lib/apt/lists/*

RUN git config --global user.name = "cargo-http-registry"
RUN git config --global user.email = "cargo-http-registry@example.com"

COPY --from=builder /app/target/release/cargo-http-registry /usr/local/bin
ENTRYPOINT ["cargo-http-registry"]
