# HubMQ Setup — From Zero to Production

Complete reproducible guide to set up HubMQ Phase Core infrastructure on LXC 415 (or any Debian 13 system).

## Prerequisites

- Proxmox host with LXC capability
- Debian 13 container (2 vCPU / 2GB RAM / 20GB disk minimum)
- SSH access from LXC 500 (or your deployment machine)
- SMTP Gmail account with App Password generated at https://myaccount.google.com/apppasswords
- Telegram bot token from @BotFather (`/newbot` → copy token)

## 1. LXC 415 Provisioning (Proxmox)

### 1.1 Create container on Proxmox

```bash
# On Proxmox host (pve)
pct create 415 \
  -hostname hubmq \
  -ostype debian \
  -osversion 13 \
  -cores 2 \
  -memory 2048 \
  -swap 0 \
  -disk sda:vm-415-disk-0,size=20G \
  -net0 name=eth0,bridge=vmbr0,firewall=1,hwaddr=xx:xx:xx:xx:xx:xx \
  -unprivileged 1
```

### 1.2 Boot and configure network

```bash
pct start 415
pct enter 415

# Inside container
apt update && apt install -y curl wget sudo ssh net-tools

# Set static IP (edit /etc/network/interfaces or netplan)
# Should be 192.168.10.15/24, gateway 192.168.10.1
```

### 1.3 Create user and SSH hardening

```bash
# Create motreffs user with sudo NOPASSWD
useradd -m -s /bin/bash motreffs
usermod -aG sudo motreffs

# Edit /etc/sudoers (visudo)
# Add: motreffs ALL=(ALL) NOPASSWD:ALL

# SSH hardening
# Edit /etc/ssh/sshd_config
# - PermitRootLogin no
# - PasswordAuthentication no
# - PubkeyAuthentication yes

# Copy your SSH public key
mkdir -p /home/motreffs/.ssh
cat > /home/motreffs/.ssh/authorized_keys <<EOF
ssh-rsa AAAA... # your public key here
EOF
chown -R motreffs:motreffs /home/motreffs/.ssh
chmod 700 /home/motreffs/.ssh
chmod 600 /home/motreffs/.ssh/authorized_keys

systemctl restart ssh

# SSH alias in ~/.ssh/config on LXC 500:
# Host hubmq
#   HostName 192.168.10.15
#   User motreffs
#   IdentityFile ~/.ssh/id_rsa
```

### 1.4 UFW firewall setup

```bash
# From container (as motreffs sudo)
sudo apt install -y ufw
sudo ufw default deny incoming
sudo ufw default allow outgoing

# Allow SSH from LAN
sudo ufw allow from 192.168.10.0/24 to any port 22 comment "SSH LAN"

# Allow NATS from LAN
sudo ufw allow from 192.168.10.0/24 to any port 4222 comment "NATS LAN"

# Allow monitoring endpoint from LAN
sudo ufw allow from 192.168.10.0/24 to any port 8222 comment "NATS monitoring"

# HTTP webhook endpoint (internal)
sudo ufw allow from 192.168.10.0/24 to any port 8470 comment "HubMQ health"

# Enable firewall
sudo ufw enable
sudo ufw status

# Verify
sudo ufw status numbered
```

### 1.5 Wazuh agent installation

```bash
# From LXC 500, use the deployment script
ssh hubmq "sudo bash" <<'EOF'
# Install script from ~/scripts/deploy-wazuh-agent.sh
# Or manual installation:
curl -s https://packages.wazuh.com/key/GPG-KEY-WAZUH | apt-key add -
echo "deb https://packages.wazuh.com/4.x/apt/ stable main" > /etc/apt/sources.list.d/wazuh.list
apt update
apt install -y wazuh-agent

# Configure Wazuh manager IP
# Edit /var/ossec/etc/ossec.conf
# Set <client><manager_ip>192.168.10.12</manager_ip></client>

systemctl start wazuh-agent
systemctl enable wazuh-agent
EOF

# On LXC 412 (Wazuh manager), approve agent
# Web UI: Administration > Agents > Find agent 010 (hubmq) > Approve
```

### 1.6 Create Wazuh FIM group (custom)

```bash
# SSH into hubmq
ssh hubmq

# Create hubmq group in Wazuh agent config
# Edit /var/ossec/etc/ossec.conf (inside container), add:

<localfile>
  <log_format>command</log_format>
  <command>find /etc/hubmq /var/lib/hubmq /usr/local/bin/hubmq* -type f</command>
  <frequency>3600</frequency>
</localfile>

<ossec_config>
  <localfile>
    <log_format>command</log_format>
    <command>ls -la /etc/hubmq /var/lib/hubmq /usr/local/bin/hubmq* 2>/dev/null | md5sum</command>
    <frequency>600</frequency>
    <only-future-events>no</only-future-events>
  </localfile>
</ossec_config>

# Restart agent
sudo systemctl restart wazuh-agent
```

## 2. NATS JetStream Setup

### 2.1 Install NATS server binary

```bash
ssh hubmq "sudo bash" <<'EOF'
# Download latest nats-server release
cd /tmp
wget -O nats-server.tar.gz https://github.com/nats-io/nats-server/releases/download/v2.10.24/nats-server-v2.10.24-linux-amd64.tar.gz
tar xzf nats-server.tar.gz
sudo install -m 755 nats-server-v2.10.24-linux-amd64/nats-server /usr/local/bin/

# Verify
nats-server -v

# Download nsc (NATS Secure Config tool)
cd /tmp
wget -O nsc.tar.gz https://github.com/nats-io/nsc/releases/download/v2.8.4/nsc-v2.8.4-linux-amd64.tar.gz
tar xzf nsc.tar.gz
sudo install -m 755 nsc-v2.8.4-linux-amd64/nsc /usr/local/bin/

# Verify
nsc --version
EOF
```

### 2.2 Create nats user and directories

```bash
ssh hubmq "sudo bash" <<'EOF'
# Create system user
useradd -r -s /usr/sbin/nologin -M nats || true

# Create directories with proper ownership
mkdir -p /var/lib/nats/jetstream
chown -R nats:nats /var/lib/nats
chmod 700 /var/lib/nats

mkdir -p /etc/nats/nkeys
chown root:nats /etc/nats/nkeys
chmod 700 /etc/nats/nkeys

mkdir -p /var/log/nats
chown nats:nats /var/log/nats
chmod 755 /var/log/nats
EOF
```

### 2.3 Generate NKeys

```bash
ssh hubmq "sudo bash" <<'EOF'
cd /etc/nats/nkeys

# Generate account keys
nsc add account -n hubmq || true

# Generate hubmq-service NKey (daemon, full access)
nsc add user -a hubmq hubmq-service -K
nsc pub user -a hubmq hubmq-service -K /tmp/hubmq-service-pub.key

# Extract seed and public key from ~/.nsc/stores/hubmq/accounts/hubmq/users/
# Adjust paths based on nsc version

# Manual NKey generation if nsc doesn't cooperate:
# Use offline NKey generation or import from NATS documentation

# For now, assume seeds are at:
# /etc/nats/nkeys/hubmq-service.seed
# /etc/nats/nkeys/publisher.seed
# (Will be copied via deploy script)

# For testing, generate with nkey library (see deploy/README-nkeys.md)

chmod 600 /etc/nats/nkeys/*.seed
chmod 644 /etc/nats/nkeys/*.pub
chown nats:nats /etc/nats/nkeys/hubmq-service.seed /etc/nats/nkeys/publisher.seed
EOF
```

### 2.4 Install NATS configuration

```bash
# Copy NATS config template from project
scp deploy/nats-server.conf.template hubmq:/tmp/nats-server.conf.template

ssh hubmq "sudo bash" <<'EOF'
# Substitute actual NKey public keys into template
HUBMQ_PUBKEY=$(grep "^UD" /etc/nats/nkeys/hubmq-service.pub | awk '{print $1}')
PUBLISHER_PUBKEY=$(grep "^UA" /etc/nats/nkeys/publisher.pub | awk '{print $1}')

sed -e "s|\${HUBMQ_NKEY}|$HUBMQ_PUBKEY|g" \
    -e "s|\${PUB_NKEY}|$PUBLISHER_PUBKEY|g" \
    /tmp/nats-server.conf.template > /etc/nats/nats-server.conf

# Verify syntax
sudo -u nats /usr/local/bin/nats-server -c /etc/nats/nats-server.conf -t

# Install logrotate
cat > /etc/logrotate.d/nats <<'LOGROTATE'
/var/log/nats-server.log {
  daily
  rotate 7
  compress
  delaycompress
  missingok
  notifempty
  postrotate
    systemctl reload nats.service > /dev/null 2>&1 || true
  endscript
}
LOGROTATE
EOF
```

### 2.5 Install and enable NATS systemd service

```bash
# Copy systemd unit
scp deploy/nats.service hubmq:/tmp/

ssh hubmq "sudo bash" <<'EOF'
install -m 644 /tmp/nats.service /etc/systemd/system/
systemctl daemon-reload
systemctl enable nats.service
systemctl start nats.service

# Verify
sleep 2
systemctl status nats.service
netstat -tuln | grep 4222
EOF
```

### 2.6 Create JetStream streams manually

```bash
ssh hubmq <<'EOF'
# Install nats CLI (standalone client)
cd /tmp
wget -O nats.tar.gz https://github.com/nats-io/natscli/releases/download/v0.1.5/nats-v0.1.5-linux-amd64.tar.gz
tar xzf nats.tar.gz
sudo install -m 755 nats-v0.1.5-linux-amd64/nats /usr/local/bin/

# Export seed for auth
export NATS_NKEY_SEED_PATH=/etc/nats/nkeys/hubmq-service.seed

# Create streams (via nats-server internal, or nats CLI commands)
nats stream add ALERTS \
  --subjects 'alert.*' \
  --storage file \
  --retention limits \
  --max-age 24h \
  --max-msgs 10000 \
  --max-bytes 1073741824 \
  --discard old \
  --dupe-window 300s

nats stream add MONITOR \
  --subjects 'monitor.*' \
  --storage file \
  --retention limits \
  --max-age 1h \
  --max-msgs 5000 \
  --max-bytes 104857600 \
  --discard old \
  --dupe-window 300s

nats stream add AGENTS \
  --subjects 'agent.*' \
  --storage file \
  --retention limits \
  --max-age 24h \
  --max-msgs 2000 \
  --max-bytes 524288000 \
  --discard old \
  --dupe-window 300s

nats stream add SYSTEM \
  --subjects 'system.*' \
  --storage file \
  --retention limits \
  --max-age 24h \
  --max-msgs 5000 \
  --max-bytes 524288000 \
  --discard old \
  --dupe-window 300s

nats stream add CRON \
  --subjects 'cron.*' \
  --storage file \
  --retention limits \
  --max-age 168h \
  --max-msgs 10000 \
  --max-bytes 1073741824 \
  --discard old \
  --dupe-window 300s

nats stream add USER_IN \
  --subjects 'user.incoming.*' \
  --storage file \
  --retention limits \
  --max-age 24h \
  --max-msgs 1000 \
  --max-bytes 104857600 \
  --discard old \
  --dupe-window 300s

# Verify
nats stream list
nats stream info ALERTS
EOF
```

## 3. Apprise Installation

```bash
ssh hubmq "sudo bash" <<'EOF'
# Install Python venv
apt install -y python3-venv python3-pip

# Create apprise venv
mkdir -p /opt/apprise
python3 -m venv /opt/apprise/venv
/opt/apprise/venv/bin/pip install apprise

# Create symlink for easy access
ln -sf /opt/apprise/venv/bin/apprise /usr/local/bin/apprise

# Verify
apprise --version
EOF
```

## 4. Deploy HubMQ Daemon

### 4.1 Build release binary on LXC 500

```bash
cd ~/projects/hubmq
cargo build --release -p hubmq

# Binary should be at: target/release/hubmq
```

### 4.2 Deploy using deploy script

```bash
cd ~/projects/hubmq
TARGET_HOST=hubmq bash deploy/deploy-hubmq.sh target/release/hubmq

# This script:
# 1. SCPs binary + scripts + systemd units
# 2. Creates hubmq user
# 3. Creates /etc/hubmq, /var/lib/hubmq, /var/log/hubmq
# 4. Installs binary + systemd units
# 5. Enables and starts service
# 6. Verifies health
```

### 4.3 Populate credentials

```bash
# Gmail App Password
printf "YOUR_16_CHAR_APP_PASSWORD" | \
  ssh hubmq "sudo tee /etc/hubmq/credentials/gmail-app-password" > /dev/null
ssh hubmq "sudo chmod 600 /etc/hubmq/credentials/gmail-app-password"
ssh hubmq "sudo chown root:root /etc/hubmq/credentials/gmail-app-password"

# Telegram bot token
printf "YOUR_BOT_TOKEN_FROM_BOTFATHER" | \
  ssh hubmq "sudo tee /etc/hubmq/credentials/telegram-bot-token" > /dev/null
ssh hubmq "sudo chmod 600 /etc/hubmq/credentials/telegram-bot-token"
ssh hubmq "sudo chown root:root /etc/hubmq/credentials/telegram-bot-token"
```

### 4.4 Configure config.toml

```bash
# Edit on LXC 500
scp hubmq:/etc/hubmq/config.toml /tmp/hubmq-config.toml

# Edit /tmp/hubmq-config.toml
# - Set [telegram] allowed_chat_ids = [YOUR_CHAT_ID]
# - Verify [nats] url = "nats://localhost:4222"
# - Verify [smtp] host = "smtp.gmail.com", username = "mymomot74@gmail.com"

scp /tmp/hubmq-config.toml hubmq:/tmp/
ssh hubmq "sudo install -m 640 /tmp/hubmq-config.toml /etc/hubmq/config.toml"
```

### 4.5 Start and verify

```bash
ssh hubmq "sudo systemctl restart hubmq.service"
ssh hubmq "sleep 2 && sudo systemctl status hubmq.service"

# Check health endpoint
curl -sf http://192.168.10.15:8470/health && echo " OK"

# Check logs
ssh hubmq "sudo journalctl -u hubmq.service -n 50 -f"
```

## 5. Integration with Source Systems

### 5.1 Wazuh webhook integration

In Wazuh manager (LXC 412), add custom integration:

```bash
# Create integration script at /var/ossec/integrations/hubmq.sh
cat > /var/ossec/integrations/hubmq.sh <<'SCRIPT'
#!/bin/bash
WEBHOOK_URL="http://192.168.10.15:8470/in/wazuh"

# Parse Wazuh webhook JSON from stdin
curl -X POST -H "Content-Type: application/json" \
  -d @- "$WEBHOOK_URL"
SCRIPT

chmod 750 /var/ossec/integrations/hubmq.sh
chown root:ossec /var/ossec/integrations/hubmq.sh
```

Then add to `/var/ossec/etc/ossec.conf`:
```xml
<integration>
  <name>hubmq</name>
  <hook_url>http://192.168.10.15:8470/in/wazuh</hook_url>
  <level>3</level>
  <group>sysmon,process_creation</group>
</integration>
```

### 5.2 Forgejo webhook integration

In Forgejo `motreffs/hubmq` repo settings > Webhooks, add:
- **URL**: `http://192.168.10.15:8470/in/forgejo`
- **Events**: Runs (CI/CD failures)
- **Content type**: application/json

## 6. Health Checks

```bash
# NATS connectivity
curl -s http://192.168.10.15:8222/jsz | jq '.streams | length'

# HubMQ health
curl -sf http://192.168.10.15:8470/health

# Logs
ssh hubmq "sudo journalctl -u hubmq.service -n 100"
ssh hubmq "sudo journalctl -u nats.service -n 50"

# SQLite queue status
ssh hubmq "sqlite3 /var/lib/hubmq/queue.db 'SELECT status, COUNT(*) FROM outbox GROUP BY status;'"
```

## Troubleshooting

| Issue | Check | Fix |
|---|---|---|
| HubMQ won't start | `journalctl -u hubmq` | verify config.toml, credentials exist, NATS is up |
| NATS connection fails | `nats stream list` | verify /etc/nats/nats-server.conf, NKey seeds |
| Email not sending | `maillog` via Wazuh | verify Gmail App Password, less-secure-app-access disabled |
| Telegram bot not responding | `journalctl -u hubmq \| grep telegram` | verify chat_id in config, bot token valid |
| Port 4222 not reachable | `nmap 192.168.10.15 -p 4222` | check UFW, NATS service status |
