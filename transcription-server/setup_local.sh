#!/bin/bash
# Setup script for SimulStreaming on older NVIDIA GPUs (GTX 1060, etc.)
#
# The GTX 1060 has CUDA capability 6.1, which requires PyTorch with CUDA 11.x
#
# Usage: ./setup_local.sh

set -e

echo "=== SimulStreaming Setup for GTX 1060 (CUDA 6.1) ==="

# Check if we're in the right directory or clone
if [ ! -f "simulstreaming_whisper_server.py" ]; then
    echo "Cloning SimulStreaming..."
    git clone https://github.com/ufal/SimulStreaming.git
    cd SimulStreaming
fi

# Create virtual environment
echo "Creating virtual environment..."
python3 -m venv venv
source venv/bin/activate

# Install PyTorch with CUDA 11.8 (supports GTX 1060)
echo "Installing PyTorch with CUDA 11.8 support..."
pip install torch torchaudio --index-url https://download.pytorch.org/whl/cu118

# Install other dependencies
echo "Installing other dependencies..."
pip install librosa tiktoken

# Optional: Install triton for Linux x86_64
if [[ "$(uname -s)" == "Linux" && "$(uname -m)" == "x86_64" ]]; then
    pip install "triton>=2.0.0"
fi

# Test CUDA availability
echo ""
echo "=== Testing CUDA ==="
python3 -c "
import torch
print(f'PyTorch version: {torch.__version__}')
print(f'CUDA available: {torch.cuda.is_available()}')
if torch.cuda.is_available():
    print(f'CUDA version: {torch.version.cuda}')
    print(f'GPU: {torch.cuda.get_device_name(0)}')
    print(f'GPU memory: {torch.cuda.get_device_properties(0).total_memory / 1024**3:.1f} GB')
"

# Download warmup file
echo ""
echo "Downloading warmup file..."
curl -L -o warmup.wav https://github.com/ggerganov/whisper.cpp/raw/master/samples/jfk.wav 2>/dev/null

echo ""
echo "=== Setup Complete ==="
echo ""
echo "To run the server:"
echo "  source venv/bin/activate"
echo "  python3 simulstreaming_whisper_server.py \\"
echo "      --host 0.0.0.0 \\"
echo "      --port 43007 \\"
echo "      --language auto \\"
echo "      --task transcribe \\"
echo "      --vac \\"
echo "      --min-chunk-size 0.5 \\"
echo "      --warmup-file warmup.wav"
echo ""
echo "NOTE: GTX 1060 has 6GB VRAM. Use these model options:"
echo "  - 'small'  (~2GB VRAM) - fastest, good quality"
echo "  - 'medium' (~5GB VRAM) - balanced (recommended)"
echo "  - 'large-v3' (~10GB VRAM) - best quality, WON'T FIT on 6GB"
echo ""
echo "To specify a model, the server will auto-download on first run."

