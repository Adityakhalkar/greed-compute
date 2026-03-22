#!/bin/bash
# greed-compute VPS deployment script
# Run on VPS: bash setup-vps.sh
set -e

echo "=== greed-compute VPS Setup ==="

# Install Rust if not present
if ! command -v cargo &> /dev/null; then
    echo "Installing Rust..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    source "$HOME/.cargo/env"
fi
echo "Rust: $(rustc --version)"

# Install Python3 + pip if not present
if ! command -v python3 &> /dev/null; then
    echo "Installing Python3..."
    apt-get update && apt-get install -y python3 python3-pip python3-venv
fi
echo "Python: $(python3 --version)"

# Clone or update repo
INSTALL_DIR="/opt/greed-compute"
if [ -d "$INSTALL_DIR" ]; then
    echo "Updating existing installation..."
    cd "$INSTALL_DIR"
    git pull origin main
else
    echo "Cloning greed-compute..."
    git clone https://github.com/Adityakhalkar/greed-compute.git "$INSTALL_DIR"
    cd "$INSTALL_DIR"
fi

# Create venv and install ML libraries
echo "Setting up Python environment..."
python3 -m venv .venv
.venv/bin/pip install --quiet numpy pandas scikit-learn matplotlib scipy
.venv/bin/python3 -c "import numpy, pandas, sklearn, matplotlib, scipy; print('ML libraries OK')"

# Build Rust binary
echo "Building greed-compute (release)..."
source "$HOME/.cargo/env"
cargo build --release

# Create workspace directory
mkdir -p /tmp/greed-compute

# Install systemd service
echo "Installing systemd service..."
cp deploy/greed-compute.service /etc/systemd/system/
systemctl daemon-reload
systemctl enable greed-compute
systemctl restart greed-compute

echo ""
echo "=== Setup Complete ==="
echo "Service status:"
systemctl status greed-compute --no-pager -l
echo ""
echo "Test: curl http://localhost:8080/v1/health"
