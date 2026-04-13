#!/usr/bin/env bash
set -euo pipefail

BINARY="${1:-target/release/hubmq}"
TARGET_HOST="${TARGET_HOST:-hubmq}"

echo "→ Deploying $BINARY to $TARGET_HOST"
scp "$BINARY" "$TARGET_HOST:/tmp/hubmq-new"
scp deploy/hubmq-fallback-p0.sh "$TARGET_HOST:/tmp/"
scp deploy/hubmq.service deploy/hubmq-fallback-p0.service deploy/hubmq-heartbeat.service deploy/hubmq-heartbeat.timer "$TARGET_HOST:/tmp/"
scp deploy/config.toml.example "$TARGET_HOST:/tmp/"

ssh "$TARGET_HOST" bash <<'REMOTE'
set -euo pipefail
# Create user + dirs if needed
sudo useradd -r -s /usr/sbin/nologin -M hubmq 2>/dev/null || true
sudo mkdir -p /etc/hubmq /var/lib/hubmq /var/log/hubmq
sudo chown hubmq:hubmq /var/lib/hubmq /var/log/hubmq

# Install binary
sudo install -m 755 /tmp/hubmq-new /usr/local/bin/hubmq
sudo install -m 755 /tmp/hubmq-fallback-p0.sh /usr/local/bin/hubmq-fallback-p0.sh

# Install systemd units
sudo install -m 644 /tmp/hubmq.service /etc/systemd/system/
sudo install -m 644 /tmp/hubmq-fallback-p0.service /etc/systemd/system/
sudo install -m 644 /tmp/hubmq-heartbeat.service /etc/systemd/system/
sudo install -m 644 /tmp/hubmq-heartbeat.timer /etc/systemd/system/

# Install config if missing
# NB: sudo test -f requis car /etc/hubmq est 0640 root:hubmq — motreffs ne peut pas stat le fichier
# sans sudo. Un simple `[ -f ]` retournait toujours false et écrasait le config de prod à chaque deploy.
sudo test -f /etc/hubmq/config.toml || sudo install -m 640 -o root -g hubmq /tmp/config.toml.example /etc/hubmq/config.toml

sudo systemctl daemon-reload
sudo systemctl enable hubmq.service hubmq-heartbeat.timer
sudo systemctl restart hubmq.service
sudo systemctl start hubmq-heartbeat.timer

sleep 3
sudo systemctl is-active hubmq.service && echo "HUBMQ_ACTIVE" || echo "HUBMQ_FAILED"
curl -sf http://localhost:8470/health && echo " HEALTH_OK"
REMOTE
