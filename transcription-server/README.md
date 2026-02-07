# SimulStreaming Transcription Server

This directory contains the Docker configuration for running a SimulStreaming
transcription server that integrates with the SFU media server.

## Quick Start

### 1. Build the Docker image

```bash
cd transcription-server
docker build -t simulstreaming-server .
```

### 2. Run with Docker

```bash
# Run with GPU support
docker run --gpus all -p 43007:43007 simulstreaming-server

# Or with docker-compose
docker-compose up -d
```

### 3. Test the server

```bash
# Check if server is running
nc -z localhost 43007 && echo "Server is running"

# Test with microphone (Linux)
arecord -f S16_LE -c1 -r 16000 -t raw -D default | nc localhost 43007
```

## Running the SFU with Transcription

Start the SFU server with transcription enabled:

```bash
cargo run --bin sfu-server -- \
    --enable-transcription \
    --transcription-servers "localhost:43007"
```

For multiple transcription servers (load balancing):

```bash
cargo run --bin sfu-server -- \
    --enable-transcription \
    --transcription-servers "server1:43007,server2:43008"
```

## Configuration Options

The SimulStreaming server supports many options. Common ones:

| Option | Description | Default |
|--------|-------------|---------|
| `--host` | Bind address | `0.0.0.0` |
| `--port` | TCP port | `43007` |
| `--language` | Source language (`auto` for detection) | `auto` |
| `--task` | `transcribe` or `translate` | `transcribe` |
| `--vac` | Enable Voice Activity Controller | Off |
| `--min-chunk-size` | Minimum audio chunk (seconds) | `1.0` |
| `--warmup-file` | Audio file for model warmup | None |

## GPU Requirements

- **Whisper large-v3**: ~10GB VRAM (recommended for best quality)
- **Whisper medium**: ~5GB VRAM
- **Whisper small**: ~2GB VRAM

## Scaling

Each SimulStreaming instance handles one audio stream at a time.
For multiple concurrent streams, run multiple instances:

```yaml
# docker-compose.yml
services:
  simulstreaming-1:
    ports: ["43007:43007"]
    # ...
  simulstreaming-2:
    ports: ["43008:43007"]
    # ...
```

Then configure the SFU:

```bash
--transcription-servers "localhost:43007,localhost:43008"
```

