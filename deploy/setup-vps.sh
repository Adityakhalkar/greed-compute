#!/bin/bash
# greed-compute VPS deployment script
# Fresh DigitalOcean Ubuntu 22.04 droplet setup
# Run as root: bash setup-vps.sh
set -e

echo "=== greed-compute VPS Setup ==="
echo "Running on: $(hostname) as $(whoami)"

# ── 1. System packages ────────────────────────────────────────────────────────
echo ""
echo "[1/8] Installing system packages..."
apt-get update -qq
apt-get install -y \
    git curl wget build-essential pkg-config libssl-dev \
    python3 python3-pip python3-venv python3-dev \
    nginx sqlite3 \
    ufw certbot python3-certbot-nginx \
    htop unzip

# ── 2. Firewall ───────────────────────────────────────────────────────────────
echo ""
echo "[2/8] Configuring firewall..."
ufw --force reset
ufw default deny incoming
ufw default allow outgoing
ufw allow ssh
ufw allow 'Nginx Full'
ufw --force enable
echo "Firewall status:"
ufw status

# ── 3. Rust ───────────────────────────────────────────────────────────────────
echo ""
echo "[3/8] Setting up Rust..."
if ! command -v cargo &> /dev/null; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
fi
source "$HOME/.cargo/env"
echo "Rust: $(rustc --version)"

# ── 4. Clone / update repo ────────────────────────────────────────────────────
echo ""
echo "[4/8] Deploying greed-compute..."
INSTALL_DIR="/opt/greed-compute"
if [ -d "$INSTALL_DIR/.git" ]; then
    echo "Updating existing installation..."
    cd "$INSTALL_DIR"
    git pull origin main
else
    echo "Cloning greed-compute..."
    git clone https://github.com/Adityakhalkar/greed-compute.git "$INSTALL_DIR"
    cd "$INSTALL_DIR"
fi

# ── 5. Python virtualenv + ML libraries ──────────────────────────────────────
echo ""
echo "[5/8] Setting up Python environment..."
cd "$INSTALL_DIR"
python3 -m venv .venv
.venv/bin/pip install --quiet --upgrade pip
.venv/bin/pip install --quiet numpy pandas scikit-learn matplotlib scipy dill
.venv/bin/python3 -c "import numpy, pandas, sklearn, matplotlib, scipy; print('ML libraries OK')"

# ── 6. Build Rust binary ──────────────────────────────────────────────────────
echo ""
echo "[6/8] Building greed-compute (release build, this takes a few minutes)..."
source "$HOME/.cargo/env"
cargo build --release 2>&1

# ── 7. Workspace dir + systemd service ───────────────────────────────────────
echo ""
echo "[7/8] Installing systemd service..."
mkdir -p /tmp/greed-compute
cp "$INSTALL_DIR/deploy/greed-compute.service" /etc/systemd/system/
systemctl daemon-reload
systemctl enable greed-compute
systemctl restart greed-compute
sleep 2
systemctl status greed-compute --no-pager -l

# ── 8. nginx ──────────────────────────────────────────────────────────────────
echo ""
echo "[8/8] Configuring nginx..."
cp "$INSTALL_DIR/deploy/nginx-greed-compute.conf" /etc/nginx/sites-available/greed-compute
ln -sf /etc/nginx/sites-available/greed-compute /etc/nginx/sites-enabled/greed-compute
rm -f /etc/nginx/sites-enabled/default
nginx -t && systemctl restart nginx

echo ""
echo "=== Setup Complete ==="
echo ""
echo "Service health check:"
curl -s http://localhost:8080/v1/health | python3 -m json.tool || echo "(service starting up)"
echo ""
echo "Next steps:"
echo "  1. Point your DNS A record to this VPS IP"
echo "  2. Run: certbot --nginx -d compute.deep-ml.com --non-interactive --agree-tos -m your@email.com"
echo "  3. Run: bash $INSTALL_DIR/deploy/seed-key.sh"
echo "  4. If migrating from old VPS, run: bash $INSTALL_DIR/deploy/migrate-db.sh"
