# WebRTC SFU Server

A basic Selective Forwarding Unit (SFU) server built with Rust using the webrtc-rs library with optional real-time transcription support.

## Prerequisites

- Rust 1.70+
- Protocol Buffers compiler (`protoc`)
- Python 3.8+ (for transcription backend)

## Running the Server

### Basic Server

```bash
cargo run --bin sfu-server
```

### With Transcription Support

```bash
cargo run --bin sfu-server -- --enable-transcription --transcription-servers 192.168.3.100:43008
```

### Docker

```bash
docker build -t sfu-server .
docker run -p 50051:50051 -p 3478:3478/udp sfu-server
```

### Docker Compose

```bash
docker-compose up --build
```

## Configuration

Command-line options:

| Option | Default | Description |
|--------|---------|-------------|
| `--host` | `127.0.0.1` | Media server bind address |
| `--grpc-addr` | `127.0.0.1:50051` | gRPC signaling server address |
| `--media-port-min` | `3478` | Minimum media UDP port |
| `--media-port-max` | `3478` | Maximum media UDP port |
| `--force-local-loop` / `-f` | `false` | Force use of 127.0.0.1 for media |
| `--level` / `-l` | `info` | Log level (error, warn, info, debug, trace) |
| `--debug` / `-d` | `false` | Enable debug logging with timestamps |
| `--enable-transcription` | `false` | Enable transcription support |
| `--transcription-servers` | `localhost:43007` | Transcription server address(es), comma-separated |

## Transcription Backend

The server supports real-time audio transcription using a separate Whisper-based backend powered by [SimulStreaming](https://github.com/ufal/SimulStreaming).

### Running the Transcription Server

```bash
python simulstreaming_whisper_server.py \
  --host 0.0.0.0 \
  --port 43008 \
  --language auto \
  --task transcribe \
  --vac \
  --min-chunk-size 0.5 \
  --warmup-file warmup.wav \
  --model_path turbo
```

### Transcription Options

| Option | Description |
|--------|-------------|
| `--host` | Server bind address |
| `--port` | Server port |
| `--language` | Target language (use `auto` for detection) |
| `--task` | Task type (`transcribe` or `translate`) |
| `--vac` | Enable Voice Activity Detection |
| `--min-chunk-size` | Minimum audio chunk size in seconds |
| `--warmup-file` | Audio file for model warmup |
| `--model_path` | Whisper model path/name (e.g., `turbo`, `large-v3`) |

For more configuration options and details, see the [SimulStreaming documentation](https://github.com/ufal/SimulStreaming).

### Connecting the SFU to Transcription Backend

Configure the transcription server address(es) when starting the SFU:

```bash
cargo run --bin sfu-server -- \
  --enable-transcription \
  --transcription-servers 192.168.3.100:43008
```

For multiple transcription servers, separate addresses with commas:

```bash
cargo run --bin sfu-server -- \
  --enable-transcription \
  --transcription-servers 192.168.3.100:43008,192.168.3.101:43008
```

## Project Structure

```
├── src/
│   ├── lib.rs           # Library entry point
│   ├── bin/
│   │   └── sfu.rs       # Server binary
│   ├── signal/          # gRPC signaling
│   ├── transcription/   # Transcription integration
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
- `k8s/service.yaml` - gRPC (TCP:50051) and Media (UDP:3478) services
- `k8s/stunner-gateway.yaml` - STUNner Gateway configuration
- `k8s/stunner-routes.yaml` - UDP and gRPC routes

## License

MIT
