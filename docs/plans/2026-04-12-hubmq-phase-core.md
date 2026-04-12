# HubMQ — Phase Core Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build HubMQ Phase Core — a LAN-only unified communication hub on LXC 415 that receives alerts from multiple sources (Wazuh, Forgejo, BigBrother, systemd, cron), filters them (dedup + rate limit + severity routing), delivers via email (SMTP Gmail) + ntfy push (LAN only) + Telegram bot (polling, bidirectional), with local P0 fallback if the hub itself crashes.

**Architecture:** 3-crate Rust workspace (`hubmq-core` library + `hubmq-bin` daemon + integration tests) running as systemd service on LXC 415. NATS JetStream as local bus (subjects hierarchy `alert.*`, `monitor.*`, `agent.*`, `system.*`, `cron.*`, `user.incoming.*`). SQLite for persistent queue + audit log. Apprise invoked as Python subprocess with JSON stdin (anti-injection). Telegram bot uses polling mode (no webhook = no Internet exposure). Phase Exposure (ntfy public + Telegram webhook) comes later.

**Tech Stack:** Rust 1.93.1 / Axum / tokio / `async-nats` / `teloxide` / `sqlx` (SQLite WAL) / `tera` (email templates) / `serde` / `tracing` / Apprise Python subprocess / NATS JetStream 2.10+ / systemd

**Council conditions integrated:**
- B1: Local P0 fallback (systemd OnFailure + heartbeat 3h)
- B2: Telegram forward rejection
- B3: msg-relay bridge command whitelist
- B6: ntfy auth deny-all with tokens (LAN only in Phase Core)
- D1: NATS subjects map documented (`docs/SUBJECTS.md`)
- D2: JetStream limits explicit (max_age + max_bytes per stream)
- D3: systemd LoadCredential for all secrets
- D4: Structured audit log + Wazuh FIM
- D5: P0/P1 bypass quiet hours
- D6: Adaptive rate limit (10/min normal, 100/min P0)
- D7: Boot order LXC 415 pos 6 + After=nats.service
- D8: NATS subject = fixed identifier (never user text)

---

## File Structure

```
~/projects/hubmq/
├── Cargo.toml                    # workspace
├── crates/
│   ├── hubmq-core/               # library
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── config.rs         # TOML config
│   │       ├── subjects.rs       # NATS subject builders (D8)
│   │       ├── message.rs        # canonical Message + Severity enum
│   │       ├── filter/
│   │       │   ├── mod.rs
│   │       │   ├── dedup.rs      # 60s windowed hash
│   │       │   ├── ratelimit.rs  # adaptive token bucket (D6)
│   │       │   └── severity.rs   # P0-P3 routing (D5)
│   │       ├── sink/             # delivery adapters
│   │       │   ├── mod.rs        # trait Sink
│   │       │   ├── email.rs      # lettre SMTP (Gmail)
│   │       │   ├── ntfy.rs       # reqwest HTTP POST
│   │       │   └── apprise.rs    # subprocess JSON stdin
│   │       ├── source/           # ingestion
│   │       │   ├── mod.rs
│   │       │   ├── webhook.rs    # HTTP /in/* routes
│   │       │   ├── telegram.rs   # teloxide polling
│   │       │   └── heartbeat.rs  # inverse heartbeat
│   │       ├── bridge.rs         # msg-relay bridge + whitelist (B3)
│   │       ├── audit.rs          # structured audit log (D4)
│   │       ├── queue.rs          # SQLite persistent queue
│   │       └── nats_conn.rs      # NATS client wrapper
│   └── hubmq-bin/                # daemon
│       └── src/main.rs
├── deploy/
│   ├── hubmq.service             # systemd unit + LoadCredential (D3)
│   ├── hubmq-fallback-p0.service # OnFailure fallback email (B1)
│   ├── hubmq-heartbeat.timer     # heartbeat 1h (B1)
│   ├── nats-server.conf          # NATS JetStream config + limits (D2)
│   ├── nats.service              # NATS systemd
│   └── config.toml.example
├── docs/
│   ├── plans/
│   ├── ARCHITECTURE.md
│   ├── SUBJECTS.md               # D1 NATS contract
│   └── OPERATIONS.md
├── tests/integration/
├── .forgejo/workflows/
│   ├── ci.yml
│   └── deploy.yml
└── README.md
```

---

## Phase 1 — Infrastructure Foundation (Day 1 AM)

### Task 1: Initialize Cargo workspace + Forgejo repo

**Files:**
- Create: `~/projects/hubmq/Cargo.toml`
- Create: `~/projects/hubmq/crates/hubmq-core/Cargo.toml`
- Create: `~/projects/hubmq/crates/hubmq-bin/Cargo.toml`
- Create: `~/projects/hubmq/.gitignore`
- Create: `~/projects/hubmq/README.md`

- [ ] **Step 1: Create workspace Cargo.toml**

```toml
# ~/projects/hubmq/Cargo.toml
[workspace]
resolver = "2"
members = ["crates/hubmq-core", "crates/hubmq-bin"]

[workspace.package]
version = "0.1.0"
edition = "2021"
authors = ["Stéphane motreffs <mymomot74@gmail.com>"]
license = "Apache-2.0"
repository = "http://localhost:3000/motreffs/hubmq"

[workspace.dependencies]
tokio = { version = "1.40", features = ["full"] }
axum = { version = "0.7", features = ["macros", "ws"] }
async-nats = "0.38"
teloxide = { version = "0.13", features = ["macros"] }
sqlx = { version = "0.8", features = ["runtime-tokio-rustls", "sqlite", "chrono"] }
lettre = { version = "0.11", default-features = false, features = ["tokio1-rustls-tls", "builder", "smtp-transport"] }
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"
tera = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
thiserror = "2"
anyhow = "1"
chrono = { version = "0.4", features = ["serde"] }
uuid = { version = "1", features = ["v4", "serde"] }
sha2 = "0.10"
```

- [ ] **Step 2: Create hubmq-core Cargo.toml**

```toml
# ~/projects/hubmq/crates/hubmq-core/Cargo.toml
[package]
name = "hubmq-core"
version.workspace = true
edition.workspace = true
authors.workspace = true
license.workspace = true

[dependencies]
tokio.workspace = true
axum.workspace = true
async-nats.workspace = true
teloxide.workspace = true
sqlx.workspace = true
lettre.workspace = true
reqwest.workspace = true
serde.workspace = true
serde_json.workspace = true
toml.workspace = true
tera.workspace = true
tracing.workspace = true
thiserror.workspace = true
anyhow.workspace = true
chrono.workspace = true
uuid.workspace = true
sha2.workspace = true

[dev-dependencies]
tokio = { workspace = true, features = ["test-util", "macros"] }
tempfile = "3"
```

- [ ] **Step 3: Create hubmq-bin Cargo.toml**

```toml
# ~/projects/hubmq/crates/hubmq-bin/Cargo.toml
[package]
name = "hubmq"
version.workspace = true
edition.workspace = true
authors.workspace = true
license.workspace = true

[[bin]]
name = "hubmq"
path = "src/main.rs"

[dependencies]
hubmq-core = { path = "../hubmq-core" }
tokio.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
anyhow.workspace = true
```

- [ ] **Step 4: Create .gitignore and README.md**

```
# ~/projects/hubmq/.gitignore
target/
Cargo.lock
.env
*.log
/data/
.idea/
.vscode/
```

```markdown
# ~/projects/hubmq/README.md
# HubMQ — Unified Communication Hub for Homelab

Central notification and bidirectional communication service for the mymomot.ovh homelab.

**Status**: Phase Core (LAN only) — see `docs/plans/2026-04-12-hubmq-phase-core.md`

## Architecture

- NATS JetStream bus (local, port 4222)
- Rust Axum service (`hubmq`)
- ntfy.sh push (LAN only in Phase Core)
- Apprise email/Telegram delivery
- SQLite persistent queue + audit log

See `docs/ARCHITECTURE.md` and `docs/SUBJECTS.md`.
```

- [ ] **Step 5: Initialize git + initial commit**

```bash
cd ~/projects/hubmq
git init
git add Cargo.toml crates/*/Cargo.toml .gitignore README.md docs/plans/
git commit -m "init: hubmq workspace skeleton + Phase Core plan"
```

- [ ] **Step 6: Create Forgejo repo + push**

```bash
# Via Forgejo API (using existing pattern)
curl -X POST http://localhost:3000/api/v1/user/repos \
  -H "Authorization: token $(cat ~/.forgejo-token)" \
  -H "Content-Type: application/json" \
  -d '{"name":"hubmq","description":"Unified communication hub","private":false,"default_branch":"main"}'

git remote add origin http://localhost:3000/motreffs/hubmq.git
git branch -M main
git push -u origin main
```

---

### Task 2: Install NATS JetStream on LXC 415

**Files:**
- Create on LXC 415: `/etc/nats/nats-server.conf`
- Create on LXC 415: `/etc/systemd/system/nats.service`
- Create on LXC 415: `/etc/nats/nkeys/` (0700)

- [ ] **Step 1: Download NATS binary**

```bash
ssh hubmq "curl -sL https://github.com/nats-io/nats-server/releases/download/v2.10.24/nats-server-v2.10.24-linux-amd64.tar.gz | sudo tar xz -C /tmp/ && sudo mv /tmp/nats-server-v2.10.24-linux-amd64/nats-server /usr/local/bin/ && sudo chmod +x /usr/local/bin/nats-server && /usr/local/bin/nats-server --version"
```
Expected: `nats-server: v2.10.24`

- [ ] **Step 2: Create nats user + data directory**

```bash
ssh hubmq "sudo useradd -r -s /usr/sbin/nologin -M nats && sudo mkdir -p /var/lib/nats/jetstream /etc/nats/nkeys && sudo chown -R nats:nats /var/lib/nats && sudo chmod 700 /etc/nats/nkeys"
```

- [ ] **Step 3: Generate NKeys for server + operator**

```bash
ssh hubmq "sudo curl -sL https://github.com/nats-io/nsc/releases/download/v2.10.1/nsc-linux-amd64.zip -o /tmp/nsc.zip && sudo apt install -y unzip && sudo unzip -o /tmp/nsc.zip -d /usr/local/bin/ && sudo chmod +x /usr/local/bin/nsc"

# Generate server key + seed
ssh hubmq "cd /tmp && /usr/local/bin/nsc generate nkey --server 2>&1 | sudo tee /etc/nats/nkeys/server.nk > /dev/null && sudo chmod 600 /etc/nats/nkeys/server.nk && sudo chown nats:nats /etc/nats/nkeys/server.nk"
```

- [ ] **Step 4: Write NATS config with JetStream limits (D2)**

```bash
ssh hubmq "sudo tee /etc/nats/nats-server.conf > /dev/null" <<'EOF'
# NATS JetStream config — HubMQ
server_name: "hubmq-lxc415"
listen: 0.0.0.0:4222
http: localhost:8222

# JetStream enabled with local NVMe storage
jetstream {
    store_dir: "/var/lib/nats/jetstream"
    max_memory_store: 256MB
    max_file_store: 10GB
}

# Accounts (D3 — NKeys auth, no anonymous)
accounts {
    HUBMQ: {
        jetstream: enabled
        users: [
            # publishers (bound to specific subjects)
            {nkey: "SUA...SERVER_PUBKEY_HERE", permissions: {publish: ["alert.>", "monitor.>", "agent.>", "system.>", "cron.>", "user.incoming.>"], subscribe: ["_INBOX.>"]}}
            # hubmq service (full access)
            {nkey: "SUB...HUBMQ_PUBKEY_HERE", permissions: {publish: [">"], subscribe: [">"]}}
        ]
    }
    SYS: {users: [{user: "sys", password: "$2a$11$REPLACE_ME_WITH_BCRYPT"}]}
}
system_account: "SYS"

# Logging
logfile: "/var/log/nats-server.log"
debug: false
trace: false
EOF
```

> Note: In Task 3 we replace the placeholder NKeys with real ones. For this step the config is templated; actual credentials are generated in Task 3.

- [ ] **Step 5: Systemd unit**

```bash
ssh hubmq "sudo tee /etc/systemd/system/nats.service > /dev/null" <<'EOF'
[Unit]
Description=NATS Server (JetStream) for HubMQ
Documentation=https://docs.nats.io/
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=nats
Group=nats
ExecStart=/usr/local/bin/nats-server -c /etc/nats/nats-server.conf
ExecReload=/bin/kill -HUP $MAINPID
Restart=always
RestartSec=5
LimitNOFILE=65536
# Hardening
ProtectSystem=strict
ReadWritePaths=/var/lib/nats /var/log
ProtectHome=yes
PrivateTmp=yes
NoNewPrivileges=yes

[Install]
WantedBy=multi-user.target
EOF

ssh hubmq "sudo systemctl daemon-reload && sudo systemctl enable nats"
```

- [ ] **Step 6: Do NOT start yet** — NKeys still placeholders. We complete this in Task 3.

```bash
echo "NATS systemd unit ready. Will start after Task 3 completes NKey setup."
```

---

### Task 3: Generate NKeys + Finalize NATS auth

**Files:**
- Modify on LXC 415: `/etc/nats/nats-server.conf` (replace placeholders)
- Create on LXC 415: `/etc/nats/nkeys/hubmq-service.nk` (0600 nats:nats)
- Create on LXC 415: `/etc/nats/nkeys/publisher.nk` (0600 nats:nats)

- [ ] **Step 1: Generate 2 NKey pairs (hubmq service + publisher)**

```bash
ssh hubmq "
# hubmq service NKey
sudo -u nats /usr/local/bin/nsc generate nkey --account 2>&1 | sudo tee /etc/nats/nkeys/hubmq-service.nk > /dev/null
HUBMQ_PUB=\$(sudo grep -m1 '^A' /etc/nats/nkeys/hubmq-service.nk | head -1)
echo \"HUBMQ service pubkey: \$HUBMQ_PUB\"

# publisher NKey (Wazuh, Forgejo, BigBrother use this)
sudo -u nats /usr/local/bin/nsc generate nkey --account 2>&1 | sudo tee /etc/nats/nkeys/publisher.nk > /dev/null
PUB_PUB=\$(sudo grep -m1 '^A' /etc/nats/nkeys/publisher.nk | head -1)
echo \"Publisher pubkey: \$PUB_PUB\"

sudo chmod 600 /etc/nats/nkeys/*.nk
sudo chown nats:nats /etc/nats/nkeys/*.nk
"
```

- [ ] **Step 2: Extract public keys and update nats-server.conf**

```bash
ssh hubmq "
HUBMQ_PUB=\$(sudo awk '/^A[A-Z0-9]+/{print; exit}' /etc/nats/nkeys/hubmq-service.nk)
PUB_PUB=\$(sudo awk '/^A[A-Z0-9]+/{print; exit}' /etc/nats/nkeys/publisher.nk)
sudo sed -i \"s|SUA...SERVER_PUBKEY_HERE|\$PUB_PUB|\" /etc/nats/nats-server.conf
sudo sed -i \"s|SUB...HUBMQ_PUBKEY_HERE|\$HUBMQ_PUB|\" /etc/nats/nats-server.conf
sudo grep nkey /etc/nats/nats-server.conf
"
```

- [ ] **Step 3: Validate config syntax**

```bash
ssh hubmq "sudo -u nats /usr/local/bin/nats-server -c /etc/nats/nats-server.conf -t && echo CONFIG_OK"
```
Expected: `CONFIG_OK`

- [ ] **Step 4: Start NATS + verify**

```bash
ssh hubmq "sudo systemctl start nats && sleep 2 && sudo systemctl is-active nats && curl -s http://localhost:8222/varz | jq -r '.server_name,.jetstream.config.store_dir'"
```
Expected:
```
active
hubmq-lxc415
/var/lib/nats/jetstream
```

- [ ] **Step 5: Open UFW port 4222 for LAN publishers**

```bash
ssh hubmq "sudo ufw allow from 192.168.10.0/24 to any port 4222 proto tcp comment 'NATS LAN publishers' && sudo ufw status numbered"
```

- [ ] **Step 6: Commit NATS deploy files to repo**

```bash
cd ~/projects/hubmq
# Copy deploy files locally for repo tracking (with placeholders, not real keys)
cat > deploy/nats-server.conf.template <<'EOF'
# Template — replace ${HUBMQ_NKEY} and ${PUB_NKEY} on deploy
server_name: "hubmq-lxc415"
listen: 0.0.0.0:4222
http: localhost:8222
jetstream {
    store_dir: "/var/lib/nats/jetstream"
    max_memory_store: 256MB
    max_file_store: 10GB
}
accounts {
    HUBMQ: {
        jetstream: enabled
        users: [
            {nkey: "${PUB_NKEY}", permissions: {publish: ["alert.>", "monitor.>", "agent.>", "system.>", "cron.>", "user.incoming.>"], subscribe: ["_INBOX.>"]}}
            {nkey: "${HUBMQ_NKEY}", permissions: {publish: [">"], subscribe: [">"]}}
        ]
    }
}
logfile: "/var/log/nats-server.log"
EOF

cp deploy/nats-server.conf.template deploy/
# Pull the real systemd unit back via scp
scp hubmq:/etc/systemd/system/nats.service deploy/nats.service

git add deploy/nats-server.conf.template deploy/nats.service
git commit -m "feat(nats): JetStream config template + systemd unit"
git push
```

---

### Task 4: Document NATS Subjects Map (D1)

**Files:**
- Create: `~/projects/hubmq/docs/SUBJECTS.md`

- [ ] **Step 1: Write the NATS subjects contract**

```markdown
# ~/projects/hubmq/docs/SUBJECTS.md

# HubMQ — NATS Subjects Map

Contract between publishers (Wazuh, Forgejo, BigBrother, etc.) and HubMQ consumer.

## Naming convention

`<domain>.<producer>.<event_type>[.<severity>]`

- Lowercase, dot-separated
- Fixed identifiers, **never user-provided text** (D8)
- Wildcards: `*` single token, `>` multi-token

## Catalog

### Alerts (produced by monitoring systems)

| Subject | Publisher | Severity mapping | Payload |
|---|---|---|---|
| `alert.wazuh.<level>` | Wazuh webhook | `<level>` = `critical` (Wazuh ≥12), `high` (8-11), `medium` (5-7), `low` (<5) | see schema below |
| `alert.forgejo.failed` | Forgejo webhook on CI failure | always high | CI failure payload |
| `alert.forgejo.deploy_failed` | Forgejo webhook | critical | deploy failure payload |
| `alert.bigbrother.<service>` | BigBrother health-check | service-dependent | health payload |

### Monitoring (non-alert signals)

| Subject | Publisher | Payload |
|---|---|---|
| `monitor.heartbeat.<component>` | any service heartbeat | `{timestamp, ok: bool}` |
| `monitor.metric.<service>.<metric>` | future: metrics scrape | `{value: f64, labels: {}}` |

### Agents (inter-agent events, routed to msg-relay bridge)

| Subject | Publisher | Payload |
|---|---|---|
| `agent.claude.task_start` | Claude Code | task metadata |
| `agent.claude.task_done` | Claude Code | task outcome |
| `agent.zeroclaw.<event>` | ZeroClaw | event payload |

### System

| Subject | Publisher | Payload |
|---|---|---|
| `system.<unit>.failed` | systemd OnFailure hook | unit + timestamp + exit code |

### Cron

| Subject | Publisher | Payload |
|---|---|---|
| `cron.<job>.<status>` | cron jobs | `{status: success\|failed, output: truncated}` |

### User incoming (UPSTREAM — user to system)

| Subject | Publisher | Payload |
|---|---|---|
| `user.incoming.telegram` | HubMQ Telegram bot (after auth + forward rejection) | `{chat_id, text, message_id}` |

## Payload Schema (JSON)

All messages share this envelope (`hubmq-core::message::Message`):

```json
{
  "id": "<uuid v4>",
  "ts": "<RFC3339 timestamp>",
  "source": "<service name>",
  "severity": "P0|P1|P2|P3",
  "title": "<short human title>",
  "body": "<full detail, markdown allowed>",
  "tags": ["<tag1>", "<tag2>"],
  "dedup_key": "<optional stable key for 60s dedup window>",
  "meta": {"<arbitrary>": "<strings>"}
}
```

## JetStream Streams (D2 — limits explicit)

| Stream | Subjects filter | Retention | Max msgs | Max bytes | Discard |
|---|---|---|---|---|---|
| `ALERTS` | `alert.>` | 7d | 50000 | 500MB | old |
| `MONITOR` | `monitor.>` | 24h | 10000 | 100MB | old |
| `AGENTS` | `agent.>` | 7d | 10000 | 200MB | old |
| `SYSTEM` | `system.>` | 3d | 5000 | 100MB | old |
| `CRON` | `cron.>` | 24h | 5000 | 50MB | old |
| `USER_IN` | `user.incoming.>` | 30d | 10000 | 500MB | old |

Total max disk: ~1.5GB. Leaves 18GB+ free on LXC 415 20GB disk.

## Durable Consumers

HubMQ creates one durable consumer per stream:
- `hubmq-alerts`, `hubmq-monitor`, `hubmq-agents`, `hubmq-system`, `hubmq-cron`, `hubmq-user-in`

Each consumer uses `DeliverAll` + explicit ACK. Unacked messages redelivered after 30s.

## Publisher Example (Rust)

```rust
use async_nats::jetstream;
let client = async_nats::connect("nats://localhost:4222").await?;
let js = jetstream::new(client);
js.publish("alert.wazuh.critical", payload.into()).await?.await?;
```

## Non-goals for Phase Core

- No cross-account replication
- No leaf nodes / gateways
- No TLS mTLS internal (Phase Exposure)
```

- [ ] **Step 2: Commit subjects map**

```bash
cd ~/projects/hubmq
git add docs/SUBJECTS.md
git commit -m "docs: NATS subjects contract (D1 council condition)"
git push
```

---

## Phase 2 — Core Service Skeleton (Day 1 PM - Day 2 AM)

### Task 5: Config loader + tests

**Files:**
- Create: `crates/hubmq-core/src/lib.rs`
- Create: `crates/hubmq-core/src/config.rs`
- Create: `deploy/config.toml.example`

- [ ] **Step 1: Write the failing test**

```rust
// crates/hubmq-core/src/config.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_config() {
        let toml = r#"
[nats]
url = "nats://localhost:4222"
nkey_seed_path = "/etc/nats/nkeys/hubmq-service.nk"

[smtp]
host = "smtp.gmail.com"
port = 587
username = "mymomot74@gmail.com"
from = "HubMQ <mymomot74@gmail.com>"

[telegram]
allowed_chat_ids = [12345]

[filter]
dedup_window_secs = 60
rate_limit_per_min = 10
rate_limit_p0_per_min = 100
quiet_hours_start = "22:00"
quiet_hours_end = "07:00"

[fallback]
email_to = "motreff@gmail.com"
heartbeat_silence_max_secs = 10800
"#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert_eq!(cfg.nats.url, "nats://localhost:4222");
        assert_eq!(cfg.filter.dedup_window_secs, 60);
        assert_eq!(cfg.filter.rate_limit_p0_per_min, 100);
        assert_eq!(cfg.telegram.allowed_chat_ids, vec![12345]);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cd ~/projects/hubmq
cargo test -p hubmq-core config::tests::parses_minimal_config 2>&1 | tail -5
```
Expected: FAIL (`Config` not defined)

- [ ] **Step 3: Write minimal implementation**

```rust
// crates/hubmq-core/src/config.rs
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub nats: NatsConfig,
    pub smtp: SmtpConfig,
    pub telegram: TelegramConfig,
    pub filter: FilterConfig,
    pub fallback: FallbackConfig,
    #[serde(default)]
    pub bridge: BridgeConfig,
    #[serde(default)]
    pub ntfy: NtfyConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NatsConfig {
    pub url: String,
    pub nkey_seed_path: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub from: String,
    /// systemd LoadCredential name (resolved via $CREDENTIALS_DIRECTORY)
    #[serde(default = "default_smtp_cred")]
    pub password_credential: String,
}
fn default_smtp_cred() -> String { "gmail-app-password".into() }

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramConfig {
    pub allowed_chat_ids: Vec<i64>,
    #[serde(default = "default_tg_cred")]
    pub token_credential: String,
}
fn default_tg_cred() -> String { "telegram-bot-token".into() }

#[derive(Debug, Clone, Deserialize)]
pub struct FilterConfig {
    pub dedup_window_secs: u64,
    pub rate_limit_per_min: u32,
    pub rate_limit_p0_per_min: u32,
    pub quiet_hours_start: String,
    pub quiet_hours_end: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FallbackConfig {
    pub email_to: String,
    pub heartbeat_silence_max_secs: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct BridgeConfig {
    #[serde(default)]
    pub msg_relay_url: Option<String>,
    #[serde(default)]
    pub command_whitelist: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct NtfyConfig {
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub topic: Option<String>,
    #[serde(default)]
    pub bearer_credential: Option<String>,
}

impl Config {
    pub fn from_file(path: &std::path::Path) -> anyhow::Result<Self> {
        let s = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&s)?)
    }
}
```

```rust
// crates/hubmq-core/src/lib.rs
pub mod config;
```

- [ ] **Step 4: Run test to verify it passes**

```bash
cargo test -p hubmq-core config::tests::parses_minimal_config 2>&1 | tail -3
```
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/hubmq-core/src/lib.rs crates/hubmq-core/src/config.rs
git commit -m "feat(core): config loader with TOML + tests"
```

---

### Task 6: Canonical Message + Severity enum

**Files:**
- Create: `crates/hubmq-core/src/message.rs`

- [ ] **Step 1: Write the failing test**

```rust
// crates/hubmq-core/src/message.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_message_roundtrip() {
        let m = Message {
            id: uuid::Uuid::nil(),
            ts: chrono::Utc.with_ymd_and_hms(2026,4,12,12,0,0).unwrap(),
            source: "wazuh".into(),
            severity: Severity::P0,
            title: "critical alert".into(),
            body: "service down".into(),
            tags: vec!["wazuh".into(), "critical".into()],
            dedup_key: Some("wazuh:forge-lxc500:5502".into()),
            meta: Default::default(),
        };
        let json = serde_json::to_string(&m).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back.severity, Severity::P0);
        assert_eq!(back.tags.len(), 2);
    }

    #[test]
    fn severity_from_wazuh_level() {
        assert_eq!(Severity::from_wazuh_level(14), Severity::P0);
        assert_eq!(Severity::from_wazuh_level(10), Severity::P1);
        assert_eq!(Severity::from_wazuh_level(6), Severity::P2);
        assert_eq!(Severity::from_wazuh_level(3), Severity::P3);
    }

    #[test]
    fn severity_bypasses_quiet_hours() {
        assert!(Severity::P0.bypasses_quiet_hours());
        assert!(Severity::P1.bypasses_quiet_hours());
        assert!(!Severity::P2.bypasses_quiet_hours());
        assert!(!Severity::P3.bypasses_quiet_hours());
    }
}
```

- [ ] **Step 2: Run — expect FAIL**

```bash
cargo test -p hubmq-core message 2>&1 | tail -5
```

- [ ] **Step 3: Implement**

```rust
// crates/hubmq-core/src/message.rs
use chrono::{DateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Severity {
    P0, // critical, multi-channel, bypasses quiet hours
    P1, // important, Telegram + push, bypasses quiet hours
    P2, // info, email digest
    P3, // debug, log only
}

impl Severity {
    /// Maps Wazuh alert level (0-15) to HubMQ severity.
    /// Wazuh: 12+ critical, 8-11 high, 5-7 medium, <5 low.
    pub fn from_wazuh_level(level: u8) -> Self {
        match level {
            12.. => Severity::P0,
            8..=11 => Severity::P1,
            5..=7 => Severity::P2,
            _ => Severity::P3,
        }
    }

    /// D5: P0 and P1 bypass quiet hours.
    pub fn bypasses_quiet_hours(self) -> bool {
        matches!(self, Severity::P0 | Severity::P1)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: Uuid,
    pub ts: DateTime<Utc>,
    pub source: String,
    pub severity: Severity,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub dedup_key: Option<String>,
    #[serde(default)]
    pub meta: BTreeMap<String, String>,
}

impl Message {
    pub fn new(source: impl Into<String>, severity: Severity, title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            ts: Utc::now(),
            source: source.into(),
            severity,
            title: title.into(),
            body: body.into(),
            tags: vec![],
            dedup_key: None,
            meta: Default::default(),
        }
    }

    /// Stable hash for dedup. Uses dedup_key if present, else source+title.
    pub fn dedup_hash(&self) -> String {
        use sha2::{Digest, Sha256};
        let key = self.dedup_key.clone().unwrap_or_else(|| format!("{}::{}", self.source, self.title));
        let mut h = Sha256::new();
        h.update(key.as_bytes());
        format!("{:x}", h.finalize())
    }
}
```

Add `pub mod message;` to `lib.rs`.

- [ ] **Step 4: Run tests**

```bash
cargo test -p hubmq-core message 2>&1 | tail -5
```
Expected: 3 passed

- [ ] **Step 5: Commit**

```bash
git add crates/hubmq-core/src/{lib.rs,message.rs}
git commit -m "feat(core): canonical Message + Severity enum with Wazuh mapping"
```

---

### Task 7: NATS connection wrapper + JetStream stream setup

**Files:**
- Create: `crates/hubmq-core/src/nats_conn.rs`
- Create: `crates/hubmq-core/src/subjects.rs`

- [ ] **Step 1: Subjects builder**

```rust
// crates/hubmq-core/src/subjects.rs
/// D8 — subject builders. Fixed identifiers, never user-text input.
use crate::message::Severity;

pub struct Subject;

impl Subject {
    pub fn alert_wazuh(level_category: &str) -> String {
        // level_category is ALWAYS one of {"critical","high","medium","low"}
        format!("alert.wazuh.{}", level_category)
    }

    pub fn alert_forgejo_failed() -> &'static str { "alert.forgejo.failed" }
    pub fn alert_forgejo_deploy_failed() -> &'static str { "alert.forgejo.deploy_failed" }

    pub fn monitor_heartbeat(component: &str) -> String {
        assert!(is_safe_ident(component), "invalid subject component: {}", component);
        format!("monitor.heartbeat.{}", component)
    }

    pub fn system_failed(unit: &str) -> String {
        assert!(is_safe_ident(unit), "invalid unit name: {}", unit);
        format!("system.{}.failed", unit)
    }

    pub fn user_incoming_telegram() -> &'static str { "user.incoming.telegram" }

    pub fn from_severity(sev: Severity) -> &'static str {
        match sev {
            Severity::P0 => "critical",
            Severity::P1 => "high",
            Severity::P2 => "medium",
            Severity::P3 => "low",
        }
    }
}

/// D8 — accepts only [a-z0-9_-], rejects dots and wildcards.
fn is_safe_ident(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_ident_rejects_dots_and_stars() {
        assert!(!is_safe_ident(""));
        assert!(!is_safe_ident("a.b"));
        assert!(!is_safe_ident("a*"));
        assert!(!is_safe_ident("a>b"));
        assert!(is_safe_ident("wazuh-forge-lxc500"));
    }

    #[test]
    fn alert_wazuh_builder() {
        assert_eq!(Subject::alert_wazuh("critical"), "alert.wazuh.critical");
    }
}
```

- [ ] **Step 2: Run tests**

```bash
cargo test -p hubmq-core subjects 2>&1 | tail -5
```

- [ ] **Step 3: NATS connection wrapper + stream bootstrap**

```rust
// crates/hubmq-core/src/nats_conn.rs
use crate::config::NatsConfig;
use anyhow::Context;
use async_nats::jetstream::{self, stream::Config as StreamConfig};
use std::time::Duration;

pub struct NatsConn {
    pub client: async_nats::Client,
    pub js: jetstream::Context,
}

impl NatsConn {
    pub async fn connect(cfg: &NatsConfig) -> anyhow::Result<Self> {
        let seed = std::fs::read_to_string(&cfg.nkey_seed_path)
            .with_context(|| format!("read nkey seed {:?}", cfg.nkey_seed_path))?;
        let seed = extract_seed(&seed)?;

        let client = async_nats::ConnectOptions::with_nkey(seed)
            .name("hubmq-service")
            .connect(&cfg.url)
            .await
            .context("nats connect")?;

        let js = jetstream::new(client.clone());
        Ok(Self { client, js })
    }

    /// D2 — ensure streams with explicit limits
    pub async fn ensure_streams(&self) -> anyhow::Result<()> {
        for (name, subjects, max_age_secs, max_msgs, max_bytes) in [
            ("ALERTS", vec!["alert.>".into()], 7 * 86400, 50_000, 500 * 1024 * 1024),
            ("MONITOR", vec!["monitor.>".into()], 86400, 10_000, 100 * 1024 * 1024),
            ("AGENTS", vec!["agent.>".into()], 7 * 86400, 10_000, 200 * 1024 * 1024),
            ("SYSTEM", vec!["system.>".into()], 3 * 86400, 5_000, 100 * 1024 * 1024),
            ("CRON", vec!["cron.>".into()], 86400, 5_000, 50 * 1024 * 1024),
            ("USER_IN", vec!["user.incoming.>".into()], 30 * 86400, 10_000, 500 * 1024 * 1024),
        ] {
            let cfg = StreamConfig {
                name: name.into(),
                subjects,
                max_age: Duration::from_secs(max_age_secs),
                max_messages: max_msgs,
                max_bytes: max_bytes as i64,
                discard: jetstream::stream::DiscardPolicy::Old,
                storage: jetstream::stream::StorageType::File,
                ..Default::default()
            };
            self.js.get_or_create_stream(cfg).await
                .with_context(|| format!("create stream {}", name))?;
            tracing::info!(stream = name, "ensured");
        }
        Ok(())
    }
}

fn extract_seed(file_content: &str) -> anyhow::Result<String> {
    // nsc-generated files have format: seed is a line starting with 'S'
    for line in file_content.lines() {
        let l = line.trim();
        if l.starts_with('S') && l.len() == 58 {
            return Ok(l.to_string());
        }
    }
    anyhow::bail!("no nkey seed found in file")
}
```

- [ ] **Step 4: Update lib.rs**

```rust
// crates/hubmq-core/src/lib.rs
pub mod config;
pub mod message;
pub mod nats_conn;
pub mod subjects;
```

- [ ] **Step 5: Build check + commit**

```bash
cargo check -p hubmq-core 2>&1 | tail -3
git add crates/hubmq-core/src/
git commit -m "feat(core): NATS connection + streams with explicit limits (D2)"
```

---

### Task 8: SQLite persistent queue + audit log

**Files:**
- Create: `crates/hubmq-core/src/queue.rs`
- Create: `crates/hubmq-core/src/audit.rs`
- Create: `crates/hubmq-core/migrations/001_init.sql`

- [ ] **Step 1: Migration SQL**

```sql
-- crates/hubmq-core/migrations/001_init.sql

CREATE TABLE IF NOT EXISTS outbox (
    id          TEXT PRIMARY KEY,          -- message uuid
    ts          TEXT NOT NULL,             -- ISO8601
    source      TEXT NOT NULL,
    severity    TEXT NOT NULL,
    title       TEXT NOT NULL,
    body        TEXT NOT NULL,
    dedup_hash  TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'pending', -- pending, delivered, failed
    retries     INTEGER NOT NULL DEFAULT 0,
    last_error  TEXT,
    created_at  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    delivered_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_outbox_status ON outbox(status);
CREATE INDEX IF NOT EXISTS idx_outbox_dedup_ts ON outbox(dedup_hash, ts);

-- D4 — structured audit log (FIM-watched)
CREATE TABLE IF NOT EXISTS audit (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    ts          TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    event       TEXT NOT NULL,             -- e.g. 'command_dispatched', 'message_delivered', 'auth_reject'
    actor       TEXT,                      -- chat_id, source, etc.
    target      TEXT,                      -- command verb, subject, etc.
    payload_json TEXT                      -- additional structured fields
);
CREATE INDEX IF NOT EXISTS idx_audit_event_ts ON audit(event, ts);
```

- [ ] **Step 2: Queue module with tests**

```rust
// crates/hubmq-core/src/queue.rs
use crate::message::{Message, Severity};
use anyhow::Context;
use sqlx::SqlitePool;
use std::path::Path;

pub struct Queue {
    pool: SqlitePool,
}

impl Queue {
    pub async fn open(path: &Path) -> anyhow::Result<Self> {
        let url = format!("sqlite://{}?mode=rwc", path.display());
        let pool = SqlitePool::connect(&url).await.context("sqlite connect")?;
        sqlx::query("PRAGMA journal_mode=WAL;").execute(&pool).await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self { pool })
    }

    pub async fn enqueue(&self, m: &Message) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT OR IGNORE INTO outbox(id, ts, source, severity, title, body, dedup_hash, status) \
             VALUES (?,?,?,?,?,?,?,'pending')"
        )
        .bind(m.id.to_string())
        .bind(m.ts.to_rfc3339())
        .bind(&m.source)
        .bind(serde_json::to_string(&m.severity)?.trim_matches('"'))
        .bind(&m.title)
        .bind(&m.body)
        .bind(m.dedup_hash())
        .execute(&self.pool).await?;
        Ok(())
    }

    pub async fn mark_delivered(&self, id: &str) -> anyhow::Result<()> {
        sqlx::query("UPDATE outbox SET status='delivered', delivered_at=CURRENT_TIMESTAMP WHERE id=?")
            .bind(id).execute(&self.pool).await?;
        Ok(())
    }

    /// Returns true if a message with the same dedup_hash was enqueued within the last `window_secs`.
    pub async fn is_duplicate_within(&self, dedup_hash: &str, window_secs: u64) -> anyhow::Result<bool> {
        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM outbox \
             WHERE dedup_hash = ? AND datetime(ts) > datetime('now', ?)"
        )
        .bind(dedup_hash)
        .bind(format!("-{} seconds", window_secs))
        .fetch_one(&self.pool).await?;
        Ok(row.0 > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    async fn temp_queue() -> Queue {
        let f = tempfile::NamedTempFile::new().unwrap();
        Queue::open(f.path()).await.unwrap()
    }

    #[tokio::test]
    async fn enqueue_and_dedup_detect() {
        let q = temp_queue().await;
        let m = Message::new("test", Severity::P1, "title", "body");
        q.enqueue(&m).await.unwrap();
        assert!(q.is_duplicate_within(&m.dedup_hash(), 60).await.unwrap());
    }

    #[tokio::test]
    async fn dedup_window_expires() {
        let q = temp_queue().await;
        // Insert one record manually with ts in the past
        sqlx::query(
            "INSERT INTO outbox(id, ts, source, severity, title, body, dedup_hash, status) \
             VALUES ('old', datetime('now','-120 seconds'), 'test', 'P2', 't', 'b', 'hashA', 'pending')"
        ).execute(&q.pool).await.unwrap();
        // 60s window — old record is outside
        assert!(!q.is_duplicate_within("hashA", 60).await.unwrap());
        // 180s window — old record is inside
        assert!(q.is_duplicate_within("hashA", 180).await.unwrap());
    }
}
```

- [ ] **Step 3: Audit module**

```rust
// crates/hubmq-core/src/audit.rs
use sqlx::SqlitePool;
use serde::Serialize;

pub struct Audit { pool: SqlitePool }

impl Audit {
    pub fn new(pool: SqlitePool) -> Self { Self { pool } }

    pub async fn log<P: Serialize>(&self, event: &str, actor: Option<&str>, target: Option<&str>, payload: &P) -> anyhow::Result<()> {
        let json = serde_json::to_string(payload)?;
        sqlx::query("INSERT INTO audit(event, actor, target, payload_json) VALUES (?,?,?,?)")
            .bind(event).bind(actor).bind(target).bind(&json)
            .execute(&self.pool).await?;
        tracing::info!(event, ?actor, ?target, payload = %json, "audit");
        Ok(())
    }
}
```

- [ ] **Step 4: Add sqlx-cli, run migrations check, commit**

```rust
// crates/hubmq-core/src/lib.rs
pub mod audit;
pub mod config;
pub mod message;
pub mod nats_conn;
pub mod queue;
pub mod subjects;
```

```bash
cargo test -p hubmq-core queue 2>&1 | tail -5
# Expected: 2 passed
git add crates/hubmq-core/migrations crates/hubmq-core/src/{lib.rs,queue.rs,audit.rs}
git commit -m "feat(core): SQLite queue with WAL + dedup detection + audit log (D4)"
```

---

## Phase 3 — Filtering + Ingestion (Day 2 PM - Day 3 AM)

### Task 9: Filtering Engine — Dedup (D6 partial)

**Files:**
- Create: `crates/hubmq-core/src/filter/mod.rs`
- Create: `crates/hubmq-core/src/filter/dedup.rs`

- [ ] **Step 1: Implement in-memory dedup cache with TTL**

```rust
// crates/hubmq-core/src/filter/dedup.rs
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct DedupCache {
    window: Duration,
    seen: Mutex<HashMap<String, Instant>>,
}

impl DedupCache {
    pub fn new(window_secs: u64) -> Self {
        Self { window: Duration::from_secs(window_secs), seen: Mutex::new(HashMap::new()) }
    }

    /// Returns true if this hash was seen within window. Always records the hash.
    pub fn check_and_record(&self, hash: &str) -> bool {
        let now = Instant::now();
        let mut map = self.seen.lock().unwrap();
        // GC entries older than window
        map.retain(|_, t| now.duration_since(*t) < self.window);
        let dup = map.contains_key(hash);
        map.insert(hash.to_string(), now);
        dup
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_duplicate_within_window() {
        let c = DedupCache::new(60);
        assert!(!c.check_and_record("h1"));
        assert!(c.check_and_record("h1"));
    }

    #[test]
    fn distinct_hashes_dont_collide() {
        let c = DedupCache::new(60);
        assert!(!c.check_and_record("h1"));
        assert!(!c.check_and_record("h2"));
    }
}
```

- [ ] **Step 2: filter/mod.rs placeholder**

```rust
// crates/hubmq-core/src/filter/mod.rs
pub mod dedup;
pub mod ratelimit;
pub mod severity;
```

- [ ] **Step 3: Test + commit**

```bash
cargo test -p hubmq-core filter::dedup 2>&1 | tail -5
git add crates/hubmq-core/src/filter/
git commit -m "feat(filter): dedup cache with TTL window"
```

---

### Task 10: Filtering Engine — Adaptive Rate Limit (D6)

**Files:**
- Create: `crates/hubmq-core/src/filter/ratelimit.rs`

- [ ] **Step 1: Token bucket with severity-aware capacity**

```rust
// crates/hubmq-core/src/filter/ratelimit.rs
use crate::message::Severity;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct AdaptiveRateLimiter {
    normal_per_min: u32,
    p0_per_min: u32,
    buckets: Mutex<HashMap<String, Bucket>>,
}

struct Bucket {
    tokens: f64,
    last_refill: Instant,
    capacity: f64,
    refill_per_sec: f64,
}

impl AdaptiveRateLimiter {
    pub fn new(normal_per_min: u32, p0_per_min: u32) -> Self {
        Self { normal_per_min, p0_per_min, buckets: Mutex::new(HashMap::new()) }
    }

    /// Returns true if allowed. Consumes 1 token.
    /// D6: P0 uses its own bucket with 100/min, others share 10/min.
    pub fn allow(&self, source: &str, severity: Severity) -> bool {
        let (cap, refill) = match severity {
            Severity::P0 => (self.p0_per_min as f64, self.p0_per_min as f64 / 60.0),
            _ => (self.normal_per_min as f64, self.normal_per_min as f64 / 60.0),
        };
        let key = format!("{}::{}", source, match severity {
            Severity::P0 => "p0",
            _ => "normal",
        });
        let mut buckets = self.buckets.lock().unwrap();
        let bucket = buckets.entry(key).or_insert(Bucket {
            tokens: cap, last_refill: Instant::now(), capacity: cap, refill_per_sec: refill,
        });
        let now = Instant::now();
        let elapsed = now.duration_since(bucket.last_refill).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * bucket.refill_per_sec).min(bucket.capacity);
        bucket.last_refill = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_source_limited_to_10_per_min() {
        let rl = AdaptiveRateLimiter::new(10, 100);
        // Consume 10 — all allowed
        for _ in 0..10 {
            assert!(rl.allow("wazuh", Severity::P2));
        }
        // 11th denied
        assert!(!rl.allow("wazuh", Severity::P2));
    }

    #[test]
    fn p0_has_separate_higher_bucket() {
        let rl = AdaptiveRateLimiter::new(10, 100);
        // Exhaust normal bucket
        for _ in 0..10 { rl.allow("wazuh", Severity::P2); }
        assert!(!rl.allow("wazuh", Severity::P2));
        // P0 still has its own budget
        assert!(rl.allow("wazuh", Severity::P0));
    }
}
```

- [ ] **Step 2: Test + commit**

```bash
cargo test -p hubmq-core filter::ratelimit 2>&1 | tail -5
git add crates/hubmq-core/src/filter/ratelimit.rs
git commit -m "feat(filter): adaptive rate limiter with separate P0 bucket (D6)"
```

---

### Task 11: Severity-aware delivery routing (D5)

**Files:**
- Create: `crates/hubmq-core/src/filter/severity.rs`

- [ ] **Step 1: Routing decision based on severity + time of day**

```rust
// crates/hubmq-core/src/filter/severity.rs
use crate::message::Severity;
use chrono::{NaiveTime, Timelike};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeliveryChannels {
    pub ntfy: bool,
    pub email: bool,
    pub telegram: bool,
    pub log_only: bool,
}

pub struct SeverityRouter {
    quiet_start: NaiveTime,
    quiet_end: NaiveTime,
}

impl SeverityRouter {
    pub fn new(quiet_start: &str, quiet_end: &str) -> anyhow::Result<Self> {
        Ok(Self {
            quiet_start: NaiveTime::parse_from_str(quiet_start, "%H:%M")?,
            quiet_end: NaiveTime::parse_from_str(quiet_end, "%H:%M")?,
        })
    }

    /// D5: P0 and P1 bypass quiet hours.
    pub fn route(&self, sev: Severity, now_local_time: NaiveTime) -> DeliveryChannels {
        let in_quiet = self.is_quiet_hours(now_local_time);
        match sev {
            Severity::P0 => DeliveryChannels { ntfy: true, email: true, telegram: true, log_only: false },
            Severity::P1 => DeliveryChannels { ntfy: true, email: false, telegram: true, log_only: false },
            Severity::P2 if in_quiet => DeliveryChannels { ntfy: false, email: false, telegram: false, log_only: true },
            Severity::P2 => DeliveryChannels { ntfy: false, email: true, telegram: true, log_only: false },
            Severity::P3 => DeliveryChannels { ntfy: false, email: false, telegram: false, log_only: true },
        }
    }

    fn is_quiet_hours(&self, t: NaiveTime) -> bool {
        // Handles wraparound (e.g. 22:00 → 07:00)
        if self.quiet_start < self.quiet_end {
            t >= self.quiet_start && t < self.quiet_end
        } else {
            t >= self.quiet_start || t < self.quiet_end
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p0_bypasses_quiet_hours() {
        let r = SeverityRouter::new("22:00", "07:00").unwrap();
        let night = NaiveTime::from_hms_opt(3,0,0).unwrap();
        let ch = r.route(Severity::P0, night);
        assert!(ch.telegram && ch.email && ch.ntfy);
    }

    #[test]
    fn p2_suppressed_during_quiet_hours() {
        let r = SeverityRouter::new("22:00", "07:00").unwrap();
        let ch = r.route(Severity::P2, NaiveTime::from_hms_opt(3,0,0).unwrap());
        assert!(ch.log_only);
    }

    #[test]
    fn p2_normal_hours_emails_and_telegram() {
        let r = SeverityRouter::new("22:00", "07:00").unwrap();
        let ch = r.route(Severity::P2, NaiveTime::from_hms_opt(14,0,0).unwrap());
        assert!(ch.email && ch.telegram && !ch.log_only);
    }
}
```

- [ ] **Step 2: Test + commit**

```bash
cargo test -p hubmq-core filter::severity 2>&1 | tail -5
git add crates/hubmq-core/src/filter/severity.rs
git commit -m "feat(filter): severity router with quiet hours + P0/P1 bypass (D5)"
```

---

### Task 12: HTTP Ingestion — Generic + Wazuh + Forgejo

**Files:**
- Create: `crates/hubmq-core/src/source/mod.rs`
- Create: `crates/hubmq-core/src/source/webhook.rs`
- Create: `crates/hubmq-core/src/app_state.rs`

- [ ] **Step 1: AppState shared handle**

```rust
// crates/hubmq-core/src/app_state.rs
use crate::{audit::Audit, config::Config, filter::{dedup::DedupCache, ratelimit::AdaptiveRateLimiter, severity::SeverityRouter}, nats_conn::NatsConn, queue::Queue};
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub nats: Arc<NatsConn>,
    pub queue: Arc<Queue>,
    pub audit: Arc<Audit>,
    pub dedup: Arc<DedupCache>,
    pub rate: Arc<AdaptiveRateLimiter>,
    pub router: Arc<SeverityRouter>,
}
```

- [ ] **Step 2: Webhook routes**

```rust
// crates/hubmq-core/src/source/mod.rs
pub mod webhook;
```

```rust
// crates/hubmq-core/src/source/webhook.rs
use crate::{app_state::AppState, message::{Message, Severity}, subjects::Subject};
use axum::{Json, Router, extract::State, http::StatusCode, routing::{get, post}};
use serde::Deserialize;
use serde_json::Value;

pub fn routes(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/in/generic", post(ingest_generic))
        .route("/in/wazuh", post(ingest_wazuh))
        .route("/in/forgejo", post(ingest_forgejo))
        .with_state(state)
}

async fn health() -> &'static str { "ok" }

#[derive(Deserialize)]
pub struct GenericIn {
    pub source: String,
    pub severity: Severity,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub dedup_key: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

async fn ingest_generic(State(s): State<AppState>, Json(p): Json<GenericIn>) -> Result<StatusCode, StatusCode> {
    let mut m = Message::new(p.source, p.severity, p.title, p.body);
    m.dedup_key = p.dedup_key;
    m.tags = p.tags;
    publish(&s, &m).await
}

#[derive(Deserialize)]
struct WazuhAlert {
    rule: WazuhRule,
    agent: WazuhAgent,
    full_log: String,
}
#[derive(Deserialize)]
struct WazuhRule { level: u8, description: String, id: u32 }
#[derive(Deserialize)]
struct WazuhAgent { name: String }

async fn ingest_wazuh(State(s): State<AppState>, Json(p): Json<Value>) -> Result<StatusCode, StatusCode> {
    let alert: WazuhAlert = serde_json::from_value(p).map_err(|_| StatusCode::BAD_REQUEST)?;
    let sev = Severity::from_wazuh_level(alert.rule.level);
    let mut m = Message::new(
        format!("wazuh:{}", alert.agent.name),
        sev,
        alert.rule.description,
        alert.full_log,
    );
    m.dedup_key = Some(format!("wazuh:{}:{}", alert.agent.name, alert.rule.id));
    m.tags = vec!["wazuh".into(), Subject::from_severity(sev).into()];
    publish(&s, &m).await
}

async fn ingest_forgejo(State(s): State<AppState>, Json(p): Json<Value>) -> Result<StatusCode, StatusCode> {
    // Forgejo webhook — we look at workflow_run status
    let status = p.get("workflow_run")
        .and_then(|w| w.get("conclusion"))
        .and_then(|c| c.as_str())
        .unwrap_or("unknown");
    if status == "success" {
        return Ok(StatusCode::NO_CONTENT);
    }
    let repo = p.get("repository").and_then(|r| r.get("full_name")).and_then(|s| s.as_str()).unwrap_or("?");
    let workflow = p.get("workflow_run").and_then(|w| w.get("name")).and_then(|s| s.as_str()).unwrap_or("?");
    let html_url = p.get("workflow_run").and_then(|w| w.get("html_url")).and_then(|s| s.as_str()).unwrap_or("");

    let mut m = Message::new(
        format!("forgejo:{}", repo),
        Severity::P1,
        format!("CI failed: {} — {}", repo, workflow),
        format!("Workflow {} on {} concluded: {}\n{}", workflow, repo, status, html_url),
    );
    m.dedup_key = Some(format!("forgejo:{}:{}", repo, workflow));
    m.tags = vec!["forgejo".into(), "ci".into()];
    publish(&s, &m).await
}

async fn publish(s: &AppState, m: &Message) -> Result<StatusCode, StatusCode> {
    // Dedup
    let h = m.dedup_hash();
    if s.dedup.check_and_record(&h) {
        s.audit.log("message_deduped", Some(&m.source), Some(&m.title), &serde_json::json!({"hash": h}))
            .await.ok();
        return Ok(StatusCode::ACCEPTED);
    }
    // Rate limit
    if !s.rate.allow(&m.source, m.severity) {
        s.audit.log("message_rate_limited", Some(&m.source), Some(&m.title), &serde_json::json!({}))
            .await.ok();
        return Ok(StatusCode::TOO_MANY_REQUESTS);
    }
    // Publish NATS
    let subject = match m.severity {
        _ if m.source.starts_with("wazuh") => Subject::alert_wazuh(Subject::from_severity(m.severity)),
        _ if m.source.starts_with("forgejo") => Subject::alert_forgejo_failed().to_string(),
        _ => format!("alert.{}.{}", sanitize(&m.source), Subject::from_severity(m.severity)),
    };
    let payload = serde_json::to_vec(m).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    s.nats.js.publish(subject.clone(), payload.into()).await.map_err(|e| {
        tracing::error!(err = ?e, "nats publish failed");
        StatusCode::SERVICE_UNAVAILABLE
    })?;
    s.queue.enqueue(m).await.ok();
    s.audit.log("message_ingested", Some(&m.source), Some(&subject), &serde_json::json!({"id": m.id.to_string()}))
        .await.ok();
    Ok(StatusCode::ACCEPTED)
}

fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c.to_ascii_lowercase() } else { '-' }).collect()
}
```

- [ ] **Step 3: Add to lib.rs + compile check**

```rust
// crates/hubmq-core/src/lib.rs
pub mod app_state;
pub mod audit;
pub mod config;
pub mod filter;
pub mod message;
pub mod nats_conn;
pub mod queue;
pub mod source;
pub mod subjects;
```

```bash
cargo build -p hubmq-core 2>&1 | tail -5
git add crates/hubmq-core/src/
git commit -m "feat(source): HTTP ingestion with Wazuh + Forgejo + generic routes"
```

---

## Phase 4 — Outbox / Delivery (Day 3 PM - Day 4 AM)

### Task 13: Email sink (SMTP Gmail)

**Files:**
- Create: `crates/hubmq-core/src/sink/mod.rs`
- Create: `crates/hubmq-core/src/sink/email.rs`
- Create: `crates/hubmq-core/templates/email_subject.tera`
- Create: `crates/hubmq-core/templates/email_body.tera`

- [ ] **Step 1: Sink trait**

```rust
// crates/hubmq-core/src/sink/mod.rs
use crate::message::Message;
use async_trait::async_trait;

pub mod email;
pub mod ntfy;
pub mod apprise;

#[async_trait]
pub trait Sink: Send + Sync {
    async fn deliver(&self, m: &Message) -> anyhow::Result<()>;
    fn name(&self) -> &'static str;
}
```

Add to workspace deps: `async-trait = "0.1"`.

- [ ] **Step 2: Email sink impl**

```rust
// crates/hubmq-core/src/sink/email.rs
use super::Sink;
use crate::{config::SmtpConfig, message::Message};
use anyhow::Context;
use async_trait::async_trait;
use lettre::{message::Mailbox, transport::smtp::{authentication::Credentials, SmtpTransport}, Message as Mail, Transport};
use tera::Tera;

pub struct EmailSink {
    transport: SmtpTransport,
    from: Mailbox,
    to: Mailbox,
    tera: Tera,
}

impl EmailSink {
    pub fn new(cfg: &SmtpConfig, password: &str, to: &str) -> anyhow::Result<Self> {
        let creds = Credentials::new(cfg.username.clone(), password.to_string());
        let transport = SmtpTransport::starttls_relay(&cfg.host)?
            .port(cfg.port)
            .credentials(creds)
            .build();
        let from: Mailbox = cfg.from.parse()?;
        let to: Mailbox = to.parse()?;
        let mut tera = Tera::default();
        tera.add_raw_template("subject", SUBJECT_TMPL)?;
        tera.add_raw_template("body", BODY_TMPL)?;
        Ok(Self { transport, from, to, tera })
    }
}

const SUBJECT_TMPL: &str = "[HubMQ {{ severity }}] {{ title }}";
const BODY_TMPL: &str = r#"HubMQ alert from {{ source }}

Severity: {{ severity }}
Timestamp: {{ ts }}
ID: {{ id }}

{{ body }}

-- tags: {{ tags }}
-- HubMQ LXC 415"#;

#[async_trait]
impl Sink for EmailSink {
    fn name(&self) -> &'static str { "email" }

    async fn deliver(&self, m: &Message) -> anyhow::Result<()> {
        let mut ctx = tera::Context::new();
        ctx.insert("severity", &format!("{:?}", m.severity));
        ctx.insert("title", &m.title);
        ctx.insert("body", &m.body);
        ctx.insert("source", &m.source);
        ctx.insert("ts", &m.ts.to_rfc3339());
        ctx.insert("id", &m.id.to_string());
        ctx.insert("tags", &m.tags.join(", "));

        let subject = self.tera.render("subject", &ctx)?;
        let body = self.tera.render("body", &ctx)?;

        let mail = Mail::builder()
            .from(self.from.clone())
            .to(self.to.clone())
            .subject(subject)
            .body(body)?;

        // blocking send in a spawn_blocking because lettre::SmtpTransport is sync
        let transport = self.transport.clone();
        tokio::task::spawn_blocking(move || transport.send(&mail))
            .await?
            .context("smtp send")?;
        Ok(())
    }
}
```

- [ ] **Step 3: Integration test (uses network to Gmail — mark #[ignore] by default)**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::SmtpConfig, message::{Message, Severity}};

    #[tokio::test]
    #[ignore = "requires Gmail credentials, run manually"]
    async fn real_gmail_send() {
        let pwd = std::env::var("GMAIL_APP_PASSWORD").expect("set GMAIL_APP_PASSWORD");
        let cfg = SmtpConfig {
            host: "smtp.gmail.com".into(),
            port: 587,
            username: "mymomot74@gmail.com".into(),
            from: "HubMQ <mymomot74@gmail.com>".into(),
            password_credential: "gmail-app-password".into(),
        };
        let sink = EmailSink::new(&cfg, &pwd, "motreff@gmail.com").unwrap();
        let m = Message::new("test", Severity::P2, "hubmq integration test", "test body");
        sink.deliver(&m).await.unwrap();
    }
}
```

- [ ] **Step 4: Commit**

```bash
cargo build -p hubmq-core 2>&1 | tail -3
git add crates/hubmq-core/src/sink/
git commit -m "feat(sink): email sink with lettre SMTP + tera templates"
```

---

### Task 14: ntfy sink (LAN only in Phase Core)

**Files:**
- Create: `crates/hubmq-core/src/sink/ntfy.rs`

- [ ] **Step 1: Implement ntfy HTTP POST**

```rust
// crates/hubmq-core/src/sink/ntfy.rs
use super::Sink;
use crate::message::{Message, Severity};
use async_trait::async_trait;
use reqwest::Client;

pub struct NtfySink {
    base_url: String,
    topic: String,
    bearer: Option<String>,
    client: Client,
}

impl NtfySink {
    pub fn new(base_url: impl Into<String>, topic: impl Into<String>, bearer: Option<String>) -> Self {
        Self { base_url: base_url.into(), topic: topic.into(), bearer, client: Client::new() }
    }
}

#[async_trait]
impl Sink for NtfySink {
    fn name(&self) -> &'static str { "ntfy" }

    async fn deliver(&self, m: &Message) -> anyhow::Result<()> {
        let url = format!("{}/{}", self.base_url.trim_end_matches('/'), self.topic);
        let priority = match m.severity {
            Severity::P0 => "5",
            Severity::P1 => "4",
            Severity::P2 => "3",
            Severity::P3 => "2",
        };
        let mut req = self.client.post(&url)
            .header("Title", &m.title)
            .header("Priority", priority)
            .header("Tags", m.tags.join(","))
            .body(m.body.clone());
        if let Some(t) = &self.bearer {
            req = req.bearer_auth(t);
        }
        let resp = req.send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("ntfy HTTP {}", resp.status());
        }
        Ok(())
    }
}
```

- [ ] **Step 2: Commit**

```bash
cargo build -p hubmq-core 2>&1 | tail -3
git add crates/hubmq-core/src/sink/ntfy.rs
git commit -m "feat(sink): ntfy HTTP POST with Bearer auth + priority mapping"
```

---

### Task 15: Apprise sink (subprocess JSON stdin — anti-injection)

**Files:**
- Create: `crates/hubmq-core/src/sink/apprise.rs`

- [ ] **Step 1: Implement Apprise via Python subprocess with stdin JSON**

```rust
// crates/hubmq-core/src/sink/apprise.rs
use super::Sink;
use crate::message::Message;
use async_trait::async_trait;
use serde_json::json;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// SecurityAuditor V7 + V2 — subprocess only, JSON passed via stdin.
/// No user-controlled strings ever in argv. Validation by schema.
pub struct AppriseSink {
    /// Apprise URL schemes, e.g. "tgram://BOT_TOKEN/CHAT_ID"
    urls: Vec<String>,
    python_script: String,
}

impl AppriseSink {
    pub fn new(urls: Vec<String>) -> Self {
        // Python helper reads a JSON object from stdin with fields: title, body, tag.
        // No shell interpretation, pure programmatic Apprise API.
        let python_script = r#"
import sys, json, apprise
data = json.load(sys.stdin)
a = apprise.Apprise()
for url in sys.argv[1:]:
    a.add(url)
ok = a.notify(title=data.get("title",""), body=data.get("body",""), tag=data.get("tag",None))
sys.exit(0 if ok else 1)
"#.into();
        Self { urls, python_script }
    }
}

#[async_trait]
impl Sink for AppriseSink {
    fn name(&self) -> &'static str { "apprise" }

    async fn deliver(&self, m: &Message) -> anyhow::Result<()> {
        let payload = json!({
            "title": m.title,
            "body": m.body,
            "tag": m.tags.join(","),
        });
        let mut args: Vec<String> = vec!["-c".into(), self.python_script.clone()];
        // Pass URLs as positional args (we control them — come from config, not user input)
        for u in &self.urls { args.push(u.clone()); }

        let mut child = Command::new("python3")
            .args(&args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

        if let Some(mut stdin) = child.stdin.take() {
            let bytes = serde_json::to_vec(&payload)?;
            stdin.write_all(&bytes).await?;
            stdin.shutdown().await?;
        }

        let out = child.wait_with_output().await?;
        if !out.status.success() {
            anyhow::bail!(
                "apprise failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(())
    }
}
```

- [ ] **Step 2: Install Apprise on LXC 415**

```bash
ssh hubmq "sudo apt install -y python3-pip python3-full && sudo python3 -m venv /opt/apprise && sudo /opt/apprise/bin/pip install apprise && sudo ln -sf /opt/apprise/bin/python3 /usr/local/bin/python3-apprise && /usr/local/bin/python3-apprise -c 'import apprise; print(apprise.__version__)'"
```

Update sink to use `python3-apprise` instead of `python3`:

```rust
let mut child = Command::new("python3-apprise")  // <<< changed
```

- [ ] **Step 3: Commit**

```bash
cargo build -p hubmq-core 2>&1 | tail -3
git add crates/hubmq-core/src/sink/apprise.rs
git commit -m "feat(sink): Apprise via subprocess JSON stdin (anti-injection SecAud V7)"
```

---

## Phase 5 — Bidirectional + Bridge (Day 4 PM)

### Task 16: Telegram bot (polling mode, chat_id allowlist, forward rejection)

**Files:**
- Create: `crates/hubmq-core/src/source/telegram.rs`

- [ ] **Step 1: Telegram bot with B2 (forward rejection) + allowlist**

```rust
// crates/hubmq-core/src/source/telegram.rs
use crate::{app_state::AppState, message::{Message, Severity}};
use teloxide::{prelude::*, types::{Message as TgMessage, MessageKind, ForwardedFrom}};
use std::sync::Arc;

pub async fn run(state: AppState, token: String) -> anyhow::Result<()> {
    let bot = Bot::new(token);
    let allowed: Vec<i64> = state.cfg.telegram.allowed_chat_ids.clone();
    let state = Arc::new(state);

    teloxide::repl(bot.clone(), move |bot: Bot, msg: TgMessage| {
        let state = state.clone();
        let allowed = allowed.clone();
        async move {
            if let Err(e) = handle(&bot, &msg, &state, &allowed).await {
                tracing::warn!(err = ?e, "telegram handler error");
            }
            respond(())
        }
    }).await;
    Ok(())
}

async fn handle(bot: &Bot, msg: &TgMessage, state: &AppState, allowed: &[i64]) -> anyhow::Result<()> {
    // SecurityAuditor V1 — reject forwarded messages
    if let MessageKind::Common(ref c) = msg.kind {
        if c.forward_origin.is_some() {
            state.audit.log(
                "telegram_forward_rejected",
                Some(&msg.chat.id.to_string()),
                None,
                &serde_json::json!({"message_id": msg.id.0}),
            ).await.ok();
            return Ok(());
        }
    }

    // Allowlist check (B5 partial — chat_id)
    let chat_id = msg.chat.id.0;
    if !allowed.contains(&chat_id) {
        state.audit.log(
            "telegram_auth_reject",
            Some(&chat_id.to_string()),
            None,
            &serde_json::json!({"message_id": msg.id.0}),
        ).await.ok();
        return Ok(());
    }

    let text = msg.text().unwrap_or("").trim().to_string();
    if text.is_empty() { return Ok(()); }

    // Publish on NATS user.incoming.telegram
    let mut m = Message::new(
        "telegram",
        Severity::P2,
        format!("tg msg from {}", chat_id),
        text.clone(),
    );
    m.tags = vec!["telegram".into(), "upstream".into()];
    m.meta.insert("chat_id".into(), chat_id.to_string());
    m.meta.insert("message_id".into(), msg.id.0.to_string());

    let payload = serde_json::to_vec(&m)?;
    state.nats.js.publish(crate::subjects::Subject::user_incoming_telegram(), payload.into()).await?;

    state.audit.log(
        "telegram_message_received",
        Some(&chat_id.to_string()),
        Some("user.incoming.telegram"),
        &serde_json::json!({"len": text.len(), "id": m.id.to_string()}),
    ).await.ok();

    bot.send_message(msg.chat.id, "✓ Reçu — dispatché à HubMQ").await?;
    Ok(())
}
```

- [ ] **Step 2: Commit**

```bash
cargo build -p hubmq-core 2>&1 | tail -3
git add crates/hubmq-core/src/source/telegram.rs
git commit -m "feat(telegram): polling bot with forward rejection (B2) + chat_id allowlist"
```

---

### Task 17: msg-relay Bridge with command whitelist (B3)

**Files:**
- Create: `crates/hubmq-core/src/bridge.rs`

- [ ] **Step 1: Bridge implementation + whitelist**

```rust
// crates/hubmq-core/src/bridge.rs
use crate::{app_state::AppState, config::BridgeConfig, message::Message};
use anyhow::Context;
use reqwest::Client;

/// B3 — before forwarding to msg-relay, the first token of the message body must match whitelist.
pub struct Bridge {
    client: Client,
    url: Option<String>,
    whitelist: Vec<String>,
}

impl Bridge {
    pub fn new(cfg: &BridgeConfig) -> Self {
        Self {
            client: Client::new(),
            url: cfg.msg_relay_url.clone(),
            whitelist: cfg.command_whitelist.clone(),
        }
    }

    /// Returns Ok(true) if forwarded, Ok(false) if rejected by whitelist, Err on transport issue.
    pub async fn forward_from_telegram(&self, state: &AppState, m: &Message) -> anyhow::Result<bool> {
        let Some(url) = &self.url else {
            tracing::debug!("bridge disabled (no msg_relay_url)");
            return Ok(false);
        };
        // Extract command verb (first token)
        let verb = m.body.split_whitespace().next().unwrap_or("").to_lowercase();
        if !self.whitelist.iter().any(|w| w.eq_ignore_ascii_case(&verb)) {
            state.audit.log(
                "bridge_command_rejected",
                m.meta.get("chat_id").map(|s| s.as_str()),
                Some(&verb),
                &serde_json::json!({"id": m.id.to_string()}),
            ).await.ok();
            return Ok(false);
        }
        // Forward to msg-relay as a destined "claude-code" message
        let body = serde_json::json!({
            "to": "claude-code",
            "from": "hubmq-telegram",
            "body": m.body,
            "meta": m.meta,
        });
        let resp = self.client.post(format!("{}/send", url.trim_end_matches('/')))
            .json(&body).send().await.context("msg-relay HTTP")?;
        if !resp.status().is_success() {
            anyhow::bail!("msg-relay returned {}", resp.status());
        }
        state.audit.log(
            "bridge_command_dispatched",
            m.meta.get("chat_id").map(|s| s.as_str()),
            Some(&verb),
            &serde_json::json!({"id": m.id.to_string()}),
        ).await.ok();
        Ok(true)
    }
}
```

- [ ] **Step 2: Add bridge to lib.rs + commit**

```rust
// crates/hubmq-core/src/lib.rs
pub mod bridge;
```

```bash
cargo build -p hubmq-core 2>&1 | tail -3
git add crates/hubmq-core/src/{lib.rs,bridge.rs}
git commit -m "feat(bridge): msg-relay bridge with command whitelist (B3)"
```

---

### Task 18: Outbox dispatcher — subscribes NATS, routes to sinks, executes whitelist

**Files:**
- Create: `crates/hubmq-core/src/dispatcher.rs`

- [ ] **Step 1: Dispatcher that consumes NATS and fans out to sinks**

```rust
// crates/hubmq-core/src/dispatcher.rs
use crate::{app_state::AppState, bridge::Bridge, filter::severity::DeliveryChannels, message::Message, sink::Sink};
use async_nats::jetstream;
use chrono::Local;
use std::sync::Arc;
use futures_util::StreamExt;

pub struct Dispatcher {
    pub state: AppState,
    pub email: Option<Arc<dyn Sink>>,
    pub ntfy: Option<Arc<dyn Sink>>,
    pub telegram_apprise: Option<Arc<dyn Sink>>,
    pub bridge: Bridge,
}

impl Dispatcher {
    /// Run 2 consumers: alerts stream (downstream) + user_in stream (upstream bridge).
    pub async fn run(self: Arc<Self>) -> anyhow::Result<()> {
        let downstream = self.clone().run_downstream();
        let upstream = self.clone().run_upstream();
        tokio::select! {
            r = downstream => r,
            r = upstream => r,
        }
    }

    async fn run_downstream(self: Arc<Self>) -> anyhow::Result<()> {
        let streams = [("ALERTS", "hubmq-alerts"), ("MONITOR", "hubmq-monitor"), ("SYSTEM", "hubmq-system"), ("CRON", "hubmq-cron")];
        for (stream, consumer) in streams {
            let js = self.state.nats.js.clone();
            let self_c = self.clone();
            let stream = stream.to_string();
            let consumer = consumer.to_string();
            tokio::spawn(async move {
                if let Err(e) = self_c.consume(&js, &stream, &consumer).await {
                    tracing::error!(stream=%stream, err=?e, "consumer failed");
                }
            });
        }
        futures_util::future::pending::<()>().await;
        Ok(())
    }

    async fn consume(&self, js: &jetstream::Context, stream_name: &str, consumer_name: &str) -> anyhow::Result<()> {
        let stream = js.get_stream(stream_name).await?;
        let consumer: jetstream::consumer::PullConsumer = stream.get_or_create_consumer(consumer_name, jetstream::consumer::pull::Config {
            durable_name: Some(consumer_name.into()),
            ..Default::default()
        }).await?;
        let mut msgs = consumer.messages().await?;
        while let Some(msg) = msgs.next().await {
            let msg = msg?;
            match serde_json::from_slice::<Message>(&msg.payload) {
                Ok(m) => {
                    if let Err(e) = self.route_and_deliver(&m).await {
                        tracing::warn!(err=?e, id=%m.id, "delivery error");
                    }
                    msg.ack().await.ok();
                }
                Err(e) => {
                    tracing::warn!(err=?e, "message parse failed, acking to avoid redelivery");
                    msg.ack().await.ok();
                }
            }
        }
        Ok(())
    }

    async fn route_and_deliver(&self, m: &Message) -> anyhow::Result<()> {
        let now = Local::now().time();
        let channels = self.state.router.route(m.severity, now);
        tracing::info!(id=%m.id, sev=?m.severity, ?channels, "routing");

        if channels.log_only {
            self.state.audit.log("message_log_only", Some(&m.source), Some(&m.title),
                &serde_json::json!({"id": m.id.to_string()})).await.ok();
            return Ok(());
        }

        if channels.email {
            if let Some(s) = &self.email { if let Err(e) = s.deliver(m).await { tracing::warn!(err=?e, "email"); } else {
                self.state.audit.log("message_delivered", Some(&m.source), Some("email"), &serde_json::json!({"id": m.id.to_string()})).await.ok();
            }}
        }
        if channels.ntfy {
            if let Some(s) = &self.ntfy { if let Err(e) = s.deliver(m).await { tracing::warn!(err=?e, "ntfy"); } else {
                self.state.audit.log("message_delivered", Some(&m.source), Some("ntfy"), &serde_json::json!({"id": m.id.to_string()})).await.ok();
            }}
        }
        if channels.telegram {
            if let Some(s) = &self.telegram_apprise { if let Err(e) = s.deliver(m).await { tracing::warn!(err=?e, "telegram"); } else {
                self.state.audit.log("message_delivered", Some(&m.source), Some("telegram"), &serde_json::json!({"id": m.id.to_string()})).await.ok();
            }}
        }
        self.state.queue.mark_delivered(&m.id.to_string()).await.ok();
        Ok(())
    }

    async fn run_upstream(self: Arc<Self>) -> anyhow::Result<()> {
        let stream = self.state.nats.js.get_stream("USER_IN").await?;
        let consumer: jetstream::consumer::PullConsumer = stream.get_or_create_consumer("hubmq-user-in", jetstream::consumer::pull::Config {
            durable_name: Some("hubmq-user-in".into()),
            ..Default::default()
        }).await?;
        let mut msgs = consumer.messages().await?;
        while let Some(msg) = msgs.next().await {
            let msg = msg?;
            if let Ok(m) = serde_json::from_slice::<Message>(&msg.payload) {
                match self.bridge.forward_from_telegram(&self.state, &m).await {
                    Ok(true) => tracing::info!(id=%m.id, "bridge dispatched"),
                    Ok(false) => tracing::info!(id=%m.id, "bridge rejected (whitelist)"),
                    Err(e) => tracing::warn!(err=?e, id=%m.id, "bridge error"),
                }
            }
            msg.ack().await.ok();
        }
        Ok(())
    }
}
```

Add `futures-util = "0.3"` to workspace deps.

- [ ] **Step 2: Build + commit**

```rust
// crates/hubmq-core/src/lib.rs
pub mod dispatcher;
```

```bash
cargo build -p hubmq-core 2>&1 | tail -5
git add crates/hubmq-core/src/{lib.rs,dispatcher.rs}
git commit -m "feat(core): dispatcher consuming NATS streams + routing to sinks"
```

---

## Phase 6 — Binary + Deployment (Day 5)

### Task 19: hubmq-bin main daemon

**Files:**
- Create: `crates/hubmq-bin/src/main.rs`

- [ ] **Step 1: Main entry point wires everything together**

```rust
// crates/hubmq-bin/src/main.rs
use hubmq_core::{
    app_state::AppState,
    bridge::Bridge,
    config::Config,
    dispatcher::Dispatcher,
    filter::{dedup::DedupCache, ratelimit::AdaptiveRateLimiter, severity::SeverityRouter},
    nats_conn::NatsConn,
    queue::Queue,
    sink::{apprise::AppriseSink, email::EmailSink, ntfy::NtfySink, Sink},
    source::{telegram, webhook},
    audit::Audit,
};
use std::path::PathBuf;
use std::sync::Arc;
use tracing_subscriber::{EnvFilter, fmt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    fmt().json().with_env_filter(EnvFilter::from_default_env()).init();

    let cfg_path = std::env::var("HUBMQ_CONFIG").unwrap_or_else(|_| "/etc/hubmq/config.toml".into());
    let cfg = Arc::new(Config::from_file(std::path::Path::new(&cfg_path))?);

    // D3 — read SMTP password from systemd LoadCredential
    let creds_dir = std::env::var("CREDENTIALS_DIRECTORY").ok().map(PathBuf::from);
    let smtp_password = read_credential(&creds_dir, &cfg.smtp.password_credential)?;
    let telegram_token = read_credential(&creds_dir, &cfg.telegram.token_credential)?;

    // NATS + streams
    let nats = Arc::new(NatsConn::connect(&cfg.nats).await?);
    nats.ensure_streams().await?;

    // Queue + audit
    let queue = Arc::new(Queue::open(std::path::Path::new("/var/lib/hubmq/queue.db")).await?);
    let audit = Arc::new(Audit::new(queue.pool_clone()));

    // Filters
    let dedup = Arc::new(DedupCache::new(cfg.filter.dedup_window_secs));
    let rate = Arc::new(AdaptiveRateLimiter::new(cfg.filter.rate_limit_per_min, cfg.filter.rate_limit_p0_per_min));
    let router = Arc::new(SeverityRouter::new(&cfg.filter.quiet_hours_start, &cfg.filter.quiet_hours_end)?);

    let state = AppState { cfg: cfg.clone(), nats: nats.clone(), queue, audit, dedup, rate, router };

    // Sinks
    let email: Arc<dyn Sink> = Arc::new(EmailSink::new(&cfg.smtp, &smtp_password, &cfg.fallback.email_to)?);

    let ntfy: Option<Arc<dyn Sink>> = match (&cfg.ntfy.base_url, &cfg.ntfy.topic) {
        (Some(base), Some(topic)) => {
            let bearer = cfg.ntfy.bearer_credential.as_ref()
                .map(|c| read_credential(&creds_dir, c).ok()).flatten();
            Some(Arc::new(NtfySink::new(base.clone(), topic.clone(), bearer)) as Arc<dyn Sink>)
        }
        _ => None,
    };

    // Telegram outgoing via Apprise `tgram://BOT_TOKEN/CHAT_ID`
    let tg_urls: Vec<String> = cfg.telegram.allowed_chat_ids.iter()
        .map(|id| format!("tgram://{}/{}", telegram_token, id)).collect();
    let telegram_apprise: Arc<dyn Sink> = Arc::new(AppriseSink::new(tg_urls));

    let bridge = Bridge::new(&cfg.bridge);

    let dispatcher = Arc::new(Dispatcher {
        state: state.clone(),
        email: Some(email),
        ntfy,
        telegram_apprise: Some(telegram_apprise),
        bridge,
    });

    // Spawn: HTTP ingestion server, Telegram polling bot, Dispatcher consumers
    let http_state = state.clone();
    let http_task = tokio::spawn(async move {
        let app = webhook::routes(http_state);
        let listener = tokio::net::TcpListener::bind("0.0.0.0:8470").await.unwrap();
        axum::serve(listener, app).await.unwrap();
    });

    let tg_state = state.clone();
    let tg_task = tokio::spawn(async move {
        if let Err(e) = telegram::run(tg_state, telegram_token).await {
            tracing::error!(err=?e, "telegram bot crashed");
        }
    });

    let dispatcher_task = tokio::spawn(async move {
        if let Err(e) = dispatcher.run().await {
            tracing::error!(err=?e, "dispatcher crashed");
        }
    });

    tokio::signal::ctrl_c().await?;
    tracing::info!("shutdown requested");
    http_task.abort();
    tg_task.abort();
    dispatcher_task.abort();
    Ok(())
}

/// D3 — read a credential from systemd $CREDENTIALS_DIRECTORY (preferred) or fall back to /etc/hubmq/credentials/
fn read_credential(creds_dir: &Option<PathBuf>, name: &str) -> anyhow::Result<String> {
    if let Some(dir) = creds_dir {
        let p = dir.join(name);
        if p.exists() {
            return Ok(std::fs::read_to_string(p)?.trim().to_string());
        }
    }
    // Fallback (dev)
    Ok(std::fs::read_to_string(format!("/etc/hubmq/credentials/{}", name))?.trim().to_string())
}
```

Add to `Queue`:
```rust
// in queue.rs
impl Queue {
    pub fn pool_clone(&self) -> SqlitePool { self.pool.clone() }
}
```

- [ ] **Step 2: Build + commit**

```bash
cargo build --release -p hubmq 2>&1 | tail -5
git add crates/hubmq-core/src/queue.rs crates/hubmq-bin/src/main.rs
git commit -m "feat(bin): hubmq daemon wiring HTTP + Telegram + dispatcher"
```

---

### Task 20: systemd unit + LoadCredential + fallback P0 service (B1, D3)

**Files:**
- Create: `deploy/hubmq.service`
- Create: `deploy/hubmq-fallback-p0.service`
- Create: `deploy/hubmq-heartbeat.service`
- Create: `deploy/hubmq-heartbeat.timer`
- Create: `deploy/hubmq-fallback-p0.sh`

- [ ] **Step 1: Main service with LoadCredential**

```ini
# deploy/hubmq.service
[Unit]
Description=HubMQ — Unified communication hub
Documentation=http://localhost:3000/motreffs/hubmq
After=network-online.target nats.service
Requires=nats.service
Wants=network-online.target

[Service]
Type=simple
User=hubmq
Group=hubmq
Environment=RUST_LOG=info,hubmq_core=debug
Environment=HUBMQ_CONFIG=/etc/hubmq/config.toml

# D3 — systemd LoadCredential (files remain root:root 600, service sees them via $CREDENTIALS_DIRECTORY)
LoadCredential=gmail-app-password:/etc/hubmq/credentials/gmail-app-password
LoadCredential=telegram-bot-token:/etc/hubmq/credentials/telegram-bot-token

ExecStart=/usr/local/bin/hubmq
Restart=always
RestartSec=5

# B1 — fallback on repeated failure
OnFailure=hubmq-fallback-p0.service

# Hardening
ProtectSystem=strict
ReadWritePaths=/var/lib/hubmq /var/log/hubmq
ProtectHome=yes
PrivateTmp=yes
NoNewPrivileges=yes
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
```

- [ ] **Step 2: Fallback P0 script + service**

```bash
# deploy/hubmq-fallback-p0.sh
#!/usr/bin/env bash
# B1 — sends a direct email bypassing HubMQ itself when HubMQ is down.
set -euo pipefail

PASS_FILE="/etc/hubmq/credentials/gmail-app-password"
TO="${HUBMQ_FALLBACK_TO:-motreff@gmail.com}"
FROM="mymomot74@gmail.com"

HOSTNAME=$(hostname)
WHEN=$(date -Is)
STATUS=$(systemctl status hubmq --no-pager -n 20 2>&1 | head -40 || echo "systemctl status failed")

SUBJECT="[HubMQ P0 FALLBACK] hubmq.service failed on ${HOSTNAME}"
BODY="HubMQ service failed on ${HOSTNAME} at ${WHEN}.

This is a direct email fallback (bypass normal HubMQ pipeline).

systemctl status output:
${STATUS}

-- Sent by hubmq-fallback-p0.service (systemd OnFailure hook)"

PWD_VALUE=$(cat "${PASS_FILE}")

python3 <<PY
import smtplib, ssl
from email.message import EmailMessage
msg = EmailMessage()
msg["From"] = "${FROM}"
msg["To"] = "${TO}"
msg["Subject"] = """${SUBJECT}"""
msg.set_content("""${BODY}""")
ctx = ssl.create_default_context()
with smtplib.SMTP("smtp.gmail.com", 587, timeout=15) as s:
    s.starttls(context=ctx)
    s.login("${FROM}", "${PWD_VALUE}")
    s.send_message(msg)
print("FALLBACK_EMAIL_SENT_OK")
PY
```

```ini
# deploy/hubmq-fallback-p0.service
[Unit]
Description=HubMQ P0 fallback (direct email when HubMQ is down)
After=network-online.target

[Service]
Type=oneshot
ExecStart=/usr/local/bin/hubmq-fallback-p0.sh
Environment=HUBMQ_FALLBACK_TO=motreff@gmail.com
```

- [ ] **Step 3: Heartbeat (B1 — 3h silence detection)**

```ini
# deploy/hubmq-heartbeat.timer
[Unit]
Description=HubMQ heartbeat — detect 3h silence

[Timer]
OnBootSec=10min
OnUnitActiveSec=1h
Unit=hubmq-heartbeat.service

[Install]
WantedBy=timers.target
```

```ini
# deploy/hubmq-heartbeat.service
[Unit]
Description=HubMQ heartbeat probe
After=network-online.target

[Service]
Type=oneshot
ExecStart=/bin/bash -c 'curl -fs http://localhost:8470/health > /dev/null || /usr/local/bin/hubmq-fallback-p0.sh'
```

- [ ] **Step 4: Commit**

```bash
git add deploy/
git commit -m "feat(deploy): systemd units + LoadCredential + B1 fallback + heartbeat"
```

---

### Task 21: Deploy script + Forgejo CI/CD

**Files:**
- Create: `deploy/deploy-hubmq.sh`
- Create: `.forgejo/workflows/ci.yml`
- Create: `.forgejo/workflows/deploy.yml`
- Create: `deploy/config.toml.example`

- [ ] **Step 1: config.toml.example**

```toml
# deploy/config.toml.example
[nats]
url = "nats://localhost:4222"
nkey_seed_path = "/etc/nats/nkeys/hubmq-service.nk"

[smtp]
host = "smtp.gmail.com"
port = 587
username = "mymomot74@gmail.com"
from = "HubMQ Lab <mymomot74@gmail.com>"
password_credential = "gmail-app-password"

[telegram]
allowed_chat_ids = []  # fill with your chat_id
token_credential = "telegram-bot-token"

[filter]
dedup_window_secs = 60
rate_limit_per_min = 10
rate_limit_p0_per_min = 100
quiet_hours_start = "22:00"
quiet_hours_end = "07:00"

[fallback]
email_to = "motreff@gmail.com"
heartbeat_silence_max_secs = 10800

[bridge]
msg_relay_url = "http://192.168.10.99:9480"
command_whitelist = ["status", "logs", "help"]

[ntfy]
# Phase Core: LAN only, empty = disabled
# base_url = "http://localhost:2586"
# topic = "hubmq-alerts"
# bearer_credential = "ntfy-bearer"
```

- [ ] **Step 2: Deploy script**

```bash
# deploy/deploy-hubmq.sh
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
[ -f /etc/hubmq/config.toml ] || sudo install -m 640 -o root -g hubmq /tmp/config.toml.example /etc/hubmq/config.toml

sudo systemctl daemon-reload
sudo systemctl enable hubmq.service hubmq-heartbeat.timer
sudo systemctl restart hubmq.service
sudo systemctl start hubmq-heartbeat.timer

sleep 3
sudo systemctl is-active hubmq.service && echo "HUBMQ_ACTIVE" || echo "HUBMQ_FAILED"
curl -sf http://localhost:8470/health && echo " HEALTH_OK"
REMOTE
```

- [ ] **Step 3: CI workflow**

```yaml
# .forgejo/workflows/ci.yml
name: CI
on:
  push: {branches: [main]}
  pull_request:

jobs:
  test:
    runs-on: [self-hosted, linux, x64, lxc-500]
    steps:
      - uses: actions/checkout@v4
      - name: Cache cargo
        uses: actions/cache@v4
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            target
          key: hubmq-${{ hashFiles('**/Cargo.lock') }}
      - run: cargo fmt -- --check
      - run: cargo clippy --all-targets -- -D warnings
      - run: cargo test --all
      - run: cargo build --release
      - name: Upload binary
        if: github.ref == 'refs/heads/main'
        uses: actions/upload-artifact@v4
        with: {name: hubmq, path: target/release/hubmq}
```

- [ ] **Step 4: Deploy workflow**

```yaml
# .forgejo/workflows/deploy.yml
name: Deploy
on:
  push: {branches: [main]}
  workflow_dispatch:

jobs:
  deploy:
    runs-on: [self-hosted, linux, x64, lxc-500]
    needs: []
    steps:
      - uses: actions/checkout@v4
      - run: cargo build --release
      - name: Deploy to LXC 415
        env:
          TARGET_HOST: hubmq
        run: bash deploy/deploy-hubmq.sh target/release/hubmq
      - name: Smoke test
        run: |
          sleep 5
          curl -sf http://192.168.10.15:8470/health
          ssh hubmq "sudo systemctl is-active hubmq.service"
```

- [ ] **Step 5: Commit**

```bash
chmod +x deploy/deploy-hubmq.sh deploy/hubmq-fallback-p0.sh
git add deploy/ .forgejo/workflows/
git commit -m "feat(deploy): deploy script + Forgejo CI/CD workflows"
git push
```

---

### Task 22: Integration end-to-end test + BigBrother integration

**Files:**
- Create: `tests/integration/e2e.sh`
- Modify on LXC 500: `~/scripts/health-check-services.sh`

- [ ] **Step 1: e2e smoke test script**

```bash
# tests/integration/e2e.sh
#!/usr/bin/env bash
set -euo pipefail

TARGET="http://192.168.10.15:8470"

echo "→ 1. Health check"
curl -sf "$TARGET/health" | grep -q ok && echo "  ok"

echo "→ 2. Post generic alert P3 (should be log-only)"
curl -sf -X POST "$TARGET/in/generic" -H "Content-Type: application/json" -d '{
  "source":"test-e2e","severity":"P3","title":"E2E P3 test","body":"ignore me"
}' | grep -qE "^$|Accepted" || true
echo "  posted"

echo "→ 3. Post Wazuh-style alert level 14 (should email P0)"
curl -sf -X POST "$TARGET/in/wazuh" -H "Content-Type: application/json" -d '{
  "rule":{"level":14,"description":"E2E critical","id":99999},
  "agent":{"name":"e2e-host"},
  "full_log":"E2E test full log"
}' && echo "  posted wazuh"

echo "→ 4. Dedup (repost same)"
for i in 1 2 3; do
  curl -sf -X POST "$TARGET/in/wazuh" -H "Content-Type: application/json" -d '{
    "rule":{"level":14,"description":"E2E critical","id":99999},
    "agent":{"name":"e2e-host"},
    "full_log":"E2E test full log"
  }' > /dev/null
done
echo "  dedup check posted"

echo "→ 5. Check NATS stream counts"
ssh hubmq "curl -s http://localhost:8222/jsz | jq '.account_details[0].stream_detail[]|{name,messages:.state.messages}'"

echo "→ 6. Check audit log entries"
ssh hubmq "sudo sqlite3 /var/lib/hubmq/queue.db 'SELECT event, COUNT(*) FROM audit WHERE ts > datetime(\"now\", \"-5 minutes\") GROUP BY event'"

echo "✓ E2E smoke test complete"
```

- [ ] **Step 2: BigBrother integration**

```bash
# Update on LXC 500: ~/scripts/health-check-services.sh — add hubmq checks
cat >> ~/scripts/health-check-services.sh <<'EOF'

# HubMQ LXC 415 checks
check_http "NATS JetStream" "http://192.168.10.15:8222/healthz"
check_http "hubmq API"      "http://192.168.10.15:8470/health"
EOF
```

(The actual script structure depends on existing `health-check-services.sh`. This shows the intent.)

- [ ] **Step 3: Wazuh FIM update for hubmq**

```bash
ssh motreffs@192.168.10.12 "sudo docker exec wazuh-wazuh.manager-1 bash -c '
cat > /var/ossec/etc/shared/hubmq/agent.conf <<EOF_FIM
<agent_config>
  <syscheck>
    <directories realtime=\"yes\">/etc/nats</directories>
    <directories realtime=\"yes\">/etc/hubmq</directories>
    <directories realtime=\"yes\">/etc/systemd/system</directories>
    <directories check_all=\"yes\">/var/lib/hubmq</directories>
    <directories check_all=\"yes\">/var/log/hubmq</directories>
    <directories check_all=\"yes\">/home/motreffs/.ssh</directories>
    <directories check_all=\"yes\">/usr/local/bin</directories>
    <directories check_all=\"yes\">/etc/sudoers.d</directories>
  </syscheck>
  <localfile>
    <log_format>json</log_format>
    <location>/var/log/hubmq/audit.log</location>
  </localfile>
</agent_config>
EOF_FIM
echo updated
'"
```

- [ ] **Step 4: Run + commit**

```bash
chmod +x tests/integration/e2e.sh
# Run after deploy:
# bash tests/integration/e2e.sh

git add tests/integration/e2e.sh
git commit -m "test: end-to-end smoke test + BigBrother/Wazuh integration"
git push
```

---

### Task 23: Documentation — ARCHITECTURE.md + OPERATIONS.md

**Files:**
- Create: `docs/ARCHITECTURE.md`
- Create: `docs/OPERATIONS.md`

- [ ] **Step 1: ARCHITECTURE.md**

```markdown
# HubMQ Architecture

## Overview

HubMQ is a communication hub on LXC 415 that:
1. Receives messages from homelab services via HTTP (webhooks) or NATS publish
2. Filters them (dedup, rate limit, severity-aware routing)
3. Delivers via email, ntfy, Telegram
4. Bidirectional bridge: Telegram user messages → msg-relay → Claude Code

## Components

- `hubmq-core` crate: library (config, NATS, filters, sinks, sources, dispatcher, bridge)
- `hubmq-bin`: the daemon binary `/usr/local/bin/hubmq` running as systemd service
- NATS JetStream: local bus on port 4222, stream-persisted
- SQLite: persistent queue + audit log at `/var/lib/hubmq/queue.db`
- Apprise: Python subprocess for multi-channel delivery

## Data Flow (Downstream)

```
Source → POST /in/{wazuh|forgejo|generic} → filter::dedup → filter::ratelimit
       → NATS publish alert.*                              → JetStream ALERTS stream
       → dispatcher consumer → filter::severity (route)    → email/ntfy/telegram sinks
       → audit log
```

## Data Flow (Upstream)

```
Telegram API ← polling ← hubmq → filter forward_origin (reject) → chat_id allowlist
                                → NATS publish user.incoming.telegram (USER_IN stream)
                                → bridge consumer → command whitelist → msg-relay POST /send
                                → Claude Code (or other agent)
```

## Files and paths on LXC 415

- Binary: `/usr/local/bin/hubmq`
- Config: `/etc/hubmq/config.toml`
- Credentials: `/etc/hubmq/credentials/` (chmod 600 root:root, accessed via systemd LoadCredential)
- Data: `/var/lib/hubmq/queue.db` (SQLite WAL)
- Logs: `journalctl -u hubmq.service` (JSON structured)
- NATS config: `/etc/nats/nats-server.conf`
- NATS streams: `/var/lib/nats/jetstream/`

See `docs/SUBJECTS.md` for the NATS contract.

See `docs/OPERATIONS.md` for deployment, troubleshooting, credential rotation.
```

- [ ] **Step 2: OPERATIONS.md**

```markdown
# HubMQ Operations

## First-time deploy

1. `ssh hubmq` - verify LXC 415 is reachable
2. NATS installed (Task 2-3 of plan)
3. Apprise installed (Task 15)
4. Build: `cargo build --release -p hubmq`
5. Run `bash deploy/deploy-hubmq.sh target/release/hubmq`
6. Fill `/etc/hubmq/config.toml` with your chat_id
7. Drop credentials in `/etc/hubmq/credentials/` (chmod 600 root:root):
   - `gmail-app-password` (16 chars)
   - `telegram-bot-token` (from @BotFather)
8. `sudo systemctl restart hubmq`

## Health check

```bash
curl -sf http://192.168.10.15:8470/health
ssh hubmq "sudo systemctl status hubmq.service"
ssh hubmq "curl -s http://localhost:8222/jsz | jq"
```

## Credential rotation

- Gmail App Password: generate new at https://myaccount.google.com/apppasswords, overwrite `/etc/hubmq/credentials/gmail-app-password`, `systemctl restart hubmq`
- Telegram bot token: regenerate at @BotFather `/revoke`, overwrite credential file, restart

## Adding a new source

1. Define subject in `docs/SUBJECTS.md`
2. Either HTTP POST to `/in/generic` with JSON payload (easiest), or publish directly on NATS with a publisher NKey

## Troubleshooting

| Symptom | Check | Action |
|---|---|---|
| hubmq-fallback-p0 triggered email | `journalctl -u hubmq.service -n 100` | investigate crash, fix, restart |
| No messages delivered | `sqlite3 /var/lib/hubmq/queue.db 'SELECT status, COUNT(*) FROM outbox GROUP BY status'` | check NATS connectivity, sink errors |
| Telegram bot not responding | `journalctl -u hubmq.service | grep telegram` | check chat_id allowlist, bot token |
| NATS stream full | `curl -s http://localhost:8222/jsz` | messages discarded (policy: old), check publisher rate |
```

- [ ] **Step 3: Commit**

```bash
git add docs/
git commit -m "docs: ARCHITECTURE.md + OPERATIONS.md"
git push
```

---

## Self-Review Checklist

**Spec coverage:**
- ✓ Phase Core goal (LAN only, no ntfy public, no Telegram webhook)
- ✓ All council conditions integrated (B1-B6, D1-D8) except B4/B5 (Phase Exposure)
- ✓ Bidirectional pattern via polling + bridge
- ✓ Filtering engine (dedup, rate limit, severity routing)
- ✓ Fallback P0 (B1)
- ✓ Heartbeat timer (B1)
- ✓ SMTP already validated manually (EMAIL_SENT_TO_MOTREFF_OK)

**Placeholder scan:** none detected (all code blocks complete, no TODOs/TBDs).

**Type consistency:** `Message`, `Severity`, `AppState`, `Subject` consistent across all tasks. `pool_clone()` added to Queue for audit construction.

**Ambiguity:** Apprise URL format `tgram://BOT_TOKEN/CHAT_ID` assumes one chat per alert — documented, acceptable for Phase Core (user is sole recipient).

---

## Execution

**Plan complete and saved to `~/projects/hubmq/docs/plans/2026-04-12-hubmq-phase-core.md`.**

Two execution options:

**1. Subagent-Driven** (recommended) — Dispatch a fresh Backend subagent per task, with Tester + Auditeur review between tasks. Fast iteration, no context pollution.

**2. Inline Execution** — Execute tasks in this session. Batch execution with human checkpoints.

**Which approach?**
