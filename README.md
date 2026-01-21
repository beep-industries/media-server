# WebRTC SFU Server

A basic Selective Forwarding Unit (SFU) server built with Rust using the webrtc-rs library.

## Prerequisites

- Rust 1.70+
- Protocol Buffers compiler (`protoc`)

## Running the Server

```bash
cargo run -- -f
```

### Docker

```bash
docker build -t sfu-server .
docker run -p 8080:8080 -p 3478:3478/udp sfu-server
```

### Docker Compose

```bash
docker-compose up --build
```

## Configuration

Set environment variables or use defaults:

| Variable   | Default | Description         |
|------------|---------|---------------------|
| `SFU_HOST` | `0.0.0.0` | Server bind address |
| `SFU_PORT` | `8080` | gRPC signaling port |
| `MEDIA_PORT`   | `3478` | Media UDP port      |

## Project Structure

```
├── src/
│   ├── lib.rs           # Library entry point
│   ├── bin/
│   │   └── sfu.rs       # Server binary
│   ├── signal/          # gRPC signaling
│   └── util/            # Utilities and certificates
├── proto/
│   └── signaling.proto  # Protocol buffer definitions
├── build.rs             # Proto compilation
├── Dockerfile
└── Cargo.toml
```

## Kubernetes Deployment (with STUNner)

### Prerequisites

Install STUNner Gateway Operator:

```bash
helm repo add stunner https://l7mp.io/stunner
helm install stunner-gateway-operator stunner/stunner-gateway-operator
```

### Deploy

```bash
kubectl apply -k k8s/
```

### Manifests

- `k8s/namespace.yaml` - SFU namespace
- `k8s/deployment.yaml` - SFU server deployment
- `k8s/service.yaml` - gRPC (TCP:8080) and Media (UDP:3478) services
- `k8s/stunner-gateway.yaml` - STUNner Gateway configuration
- `k8s/stunner-routes.yaml` - UDP and gRPC routes

## License

MIT

