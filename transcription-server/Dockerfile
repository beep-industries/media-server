# SimulStreaming Transcription Server
#
# This Dockerfile sets up a SimulStreaming server for real-time
# speech-to-text transcription using Whisper.
#
# Build: docker build -t simulstreaming-server .
# Run (GPU): docker run --gpus all -p 43007:43007 simulstreaming-server
# Run (CPU): docker run -p 43007:43007 -e USE_CPU=1 simulstreaming-server

# Use CUDA 11.8 for broader GPU compatibility (supports older GPUs like GTX 1060)
# Or use ubuntu base for CPU-only mode
ARG BASE_IMAGE=nvidia/cuda:11.8.0-cudnn8-runtime-ubuntu22.04
FROM ${BASE_IMAGE}

ENV DEBIAN_FRONTEND=noninteractive
ENV PYTHONUNBUFFERED=1

# Install system dependencies
RUN apt-get update && apt-get install -y \
    python3.11 \
    python3-pip \
    python3.11-venv \
    git \
    ffmpeg \
    libsndfile1 \
    curl \
    netcat-openbsd \
    && rm -rf /var/lib/apt/lists/*

# Set Python 3.11 as default
RUN update-alternatives --install /usr/bin/python3 python3 /usr/bin/python3.11 1

WORKDIR /app

# Clone SimulStreaming
RUN git clone https://github.com/ufal/SimulStreaming.git .

# Install PyTorch with CUDA 11.8 support (works with older GPUs like GTX 1060)
# This version supports CUDA capability 3.7+ (including GTX 1060's 6.1)
RUN pip3 install --no-cache-dir torch torchaudio --index-url https://download.pytorch.org/whl/cu118

# Install other Python dependencies (excluding torch which we installed above)
RUN pip3 install --no-cache-dir librosa tiktoken "triton>=2.0.0;platform_machine=='x86_64' and sys_platform=='linux'"

# Pre-download Whisper large-v3 model (this takes a while but speeds up startup)
# Use 'medium' or 'small' for less VRAM usage on 6GB cards
RUN python3 -c "from simul_whisper.whisper import load_model; load_model('medium')"

# Download a warmup file for faster first inference
RUN curl -L -o warmup.wav https://github.com/ggerganov/whisper.cpp/raw/master/samples/jfk.wav

# Expose the TCP port
EXPOSE 43007

# Health check
HEALTHCHECK --interval=30s --timeout=10s --retries=3 \
    CMD nc -z localhost 43007 || exit 1

# Default command - transcription mode with VAD
# Using 'medium' model which fits in 6GB VRAM (large-v3 needs ~10GB)
CMD ["python3", "simulstreaming_whisper_server.py", \
     "--host", "0.0.0.0", \
     "--port", "43007", \
     "--language", "auto", \
     "--task", "transcribe", \
     "--vac", \
     "--min-chunk-size", "0.5", \
     "--warmup-file", "warmup.wav"]

