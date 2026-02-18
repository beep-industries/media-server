## Multi-stage build for media-server (sfu-server binary)

# ---------- Builder stage ----------
FROM rust:1.91.1 AS builder

RUN apt-get update && apt-get install -y --no-install-recommends protobuf-compiler cmake build-essential \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Cache dependencies first
COPY Cargo.toml Cargo.lock ./
COPY vendor ./vendor
COPY build.rs ./
COPY proto ./proto
COPY src ./src

# Build release binary
RUN cargo build --bin sfu-server --release

# ---------- Runtime stage ----------
FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && update-ca-certificates

WORKDIR /app

COPY --from=builder /build/target/release/sfu-server /app/sfu-server

# UDP media default range and gRPC port
EXPOSE 50052/tcp
EXPOSE 3478-3482/udp

# Run with safe defaults that bind to all interfaces
CMD ["/app/sfu-server", "--grpc-addr", "0.0.0.0:50052", "--host", "0.0.0.0", "--media-port-min", "3478", "--media-port-max", "3482"]
