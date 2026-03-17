#!/bin/bash
# Node API — Hetzner CAX21 ARM Setup Script
# Ubuntu 24.04 (aarch64)
# Run as root on a fresh server: bash setup.sh

set -e
echo "=== Node API — Hetzner Setup ==="

# ── System dependencies ─────────────────────────────────────────────────────
echo "[1/8] Installing system packages..."
apt-get update -qq
apt-get install -y \
    python3.12 python3.12-venv python3.12-dev \
    python3-pip \
    gdal-bin libgdal-dev \
    libgeos-dev libproj-dev libsqlite3-dev \
    postgresql postgresql-contrib \
    git curl wget unzip \
    build-essential cmake \
    sqlite3 \
    nginx

# ── tippecanoe (compile from source — no ARM apt package) ──────────────────
echo "[2/8] Installing tippecanoe from source..."
if ! command -v tippecanoe &> /dev/null; then
    cd /tmp
    git clone https://github.com/felt/tippecanoe.git
    cd tippecanoe
    make -j$(nproc)
    make install
    cd /
    rm -rf /tmp/tippecanoe
    echo "tippecanoe $(tippecanoe --version 2>&1) installed"
else
    echo "tippecanoe already installed: $(tippecanoe --version 2>&1)"
fi

# ── Cloudflare Tunnel ──────────────────────────────────────────────────────
echo "[3/8] Installing cloudflared..."
if ! command -v cloudflared &> /dev/null; then
    curl -L https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-arm64 \
        -o /usr/local/bin/cloudflared
    chmod +x /usr/local/bin/cloudflared
    echo "cloudflared $(cloudflared --version) installed"
else
    echo "cloudflared already installed"
fi

# ── App user ────────────────────────────────────────────────────────────────
echo "[4/8] Creating nodeapi user..."
if ! id -u nodeapi &>/dev/null; then
    useradd -m -s /bin/bash nodeapi
fi

# ── Clone repo ──────────────────────────────────────────────────────────────
echo "[5/8] Cloning repo..."
if [ ! -d /opt/node-api ]; then
    git clone https://github.com/eianray/node-api /opt/node-api
    chown -R nodeapi:nodeapi /opt/node-api
else
    echo "/opt/node-api already exists — skipping clone"
fi

# ── Python virtualenv + dependencies ────────────────────────────────────────
echo "[6/8] Setting up Python environment..."
cd /opt/node-api
sudo -u nodeapi python3.12 -m venv venv
sudo -u nodeapi venv/bin/pip install --upgrade pip -q
sudo -u nodeapi venv/bin/pip install -r requirements.txt -q

# Some GDAL versions need explicit version pinning — auto-detect from system
GDAL_VERSION=$(gdal-config --version 2>/dev/null || echo "")
if [ -n "$GDAL_VERSION" ]; then
    echo "Installing GDAL Python bindings for $GDAL_VERSION..."
    sudo -u nodeapi venv/bin/pip install "GDAL==$GDAL_VERSION" -q || true
fi

# ── PostgreSQL setup ────────────────────────────────────────────────────────
echo "[7/8] Setting up PostgreSQL..."
systemctl enable postgresql
systemctl start postgresql

# Create DB user and database if they don't exist
sudo -u postgres psql -tc "SELECT 1 FROM pg_user WHERE usename = 'meridian'" | grep -q 1 || \
    sudo -u postgres psql -c "CREATE USER meridian WITH PASSWORD 'meridian';"
sudo -u postgres psql -tc "SELECT 1 FROM pg_database WHERE datname = 'meridian'" | grep -q 1 || \
    sudo -u postgres psql -c "CREATE DATABASE meridian OWNER meridian;"

echo "PostgreSQL ready. DB: meridian, User: meridian"
echo "⚠️  Change the DB password in .env for production!"

# ── .env file ───────────────────────────────────────────────────────────────
echo "[8/8] Checking .env..."
if [ ! -f /opt/node-api/.env ]; then
    cp /opt/node-api/.env.example /opt/node-api/.env 2>/dev/null || \
    cat > /opt/node-api/.env << 'ENVFILE'
DATABASE_URL=postgresql://meridian:meridian@localhost:5432/meridian
SOLANA_WALLET_ADDRESS=D8m8C9amSawdqSgEdXWkMZ3M86qsVQuDJswG1wZYkezP
SOLANA_RPC_URL=https://api.mainnet-beta.solana.com
INTERNAL_API_KEY=REPLACE_WITH_SECURE_KEY
MAX_UPLOAD_MB=50
ENVFILE
    chown nodeapi:nodeapi /opt/node-api/.env
    chmod 600 /opt/node-api/.env
    echo "⚠️  /opt/node-api/.env created — edit it before starting the service!"
else
    echo ".env already exists"
fi

echo ""
echo "=== Setup complete! ==="
echo ""
echo "Next steps:"
echo "  1. Edit /opt/node-api/.env — set INTERNAL_API_KEY and verify all values"
echo "  2. Install systemd service: cp /opt/node-api/deploy/nodeapi.service /etc/systemd/system/"
echo "  3. Enable service: systemctl enable nodeapi && systemctl start nodeapi"
echo "  4. Set up Cloudflare Tunnel: see deploy/cloudflare-tunnel.md"
echo "  5. Test: curl http://localhost:8100/v1/health"
