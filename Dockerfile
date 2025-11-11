## Multi-stage build for media-server (sfu-server binary)

# ---------- Builder stage ----------
FROM rust:1.82 as builder

WORKDIR /build

# Cache dependencies first
COPY Cargo.toml Cargo.lock ./
COPY build.rs ./
RUN mkdir -p src && echo "fn main(){}" > src/lib.rs
RUN cargo build --release || true

# Copy actual source
COPY src ./src
COPY proto ./proto

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
EXPOSE 50051/tcp
EXPOSE 3478-3482/udp

# Run with safe defaults that bind to all interfaces
CMD ["/app/sfu-server", "--grpc_addr", "0.0.0.0:50051", "--host", "0.0.0.0", "--media_port_min", "3478", "--media_port_max", "3482"]
