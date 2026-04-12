# HubMQ

> **Unified communication hub for the mymomot.ovh homelab** — ingests alerts from any service (Wazuh, Forgejo CI, BigBrother, systemd, cron, agents) via NATS JetStream, filters them (dedup + rate limit + severity routing), and delivers via email (SMTP), ntfy push, and a bidirectional Telegram bot.

[![License](https://img.shields.io/badge/license-Apache%202.0-blue)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.93.1-orange)](https://www.rust-lang.org)
[![NATS](https://img.shields.io/badge/NATS-2.10.24-green)](https://nats.io)
[![Phase](https://img.shields.io/badge/phase-Core%20%E2%9C%93%20Code%20Complete-brightgreen)](./docs/plans/2026-04-12-hubmq-phase-core.md)

## Status

**Phase Core — LIVE (2026-04-12)** — end-to-end validated.

- ✅ 23/23 tasks implemented (33 tests PASS + 2 ignored, 0 clippy warnings)
- ✅ LXC 415 HubMQ LIVE (192.168.10.15, Debian 13, 2 vCPU / 2 GB / 20 GB)
- ✅ NATS JetStream v2.10.24 LIVE (6 streams with explicit limits)
- ✅ NATS-NUI web interface at [nui.lab.mymomot.ovh](https://nui.lab.mymomot.ovh)
- ✅ SMTP Gmail validated (`mymomot74@gmail.com` App Password)
- ✅ Wazuh agent 010 Active (FIM + audit log)
- ✅ **Telegram bot [`@hubmqbot`](https://t.me/hubmqbot) LIVE** — polling mode, chat_id allowlist, forward rejection
- ✅ **hubmq daemon LIVE** on LXC 415 — `systemctl status hubmq.service` → active
- ✅ **End-to-end validated** : `POST /in/generic severity=P1` → Telegram DM received
- 📋 Phase Exposure planned (2 days) — ntfy public + Telegram webhook Internet

## Architecture — 2-phase approach

## Telegram Bot

- **Username** : [`@hubmqbot`](https://t.me/hubmqbot) (bot ID `8730722475`)
- **Mode** : Polling (no Internet-facing webhook in Phase Core)
- **Allowlist** : single `chat_id` declared in `/etc/hubmq/config.toml` → `[telegram] allowed_chat_ids = [...]`
- **Security** : forwarded messages rejected (B2), unknown chat_id silently dropped with audit entry
- **Upstream commands** : whitelist `["status", "logs", "help"]` in `[bridge] command_whitelist` → forwarded to `msg-relay` at `192.168.10.99:9480` → delivered to Claude Code
- **Downstream alerts** : delivered via Apprise `tgram://TOKEN/CHAT_ID` when severity routing picks Telegram channel (P0 always, P1 always, P2 outside quiet hours)
- **Token storage** : `/etc/hubmq/credentials/telegram-bot-token` (chmod 600 root:root, via systemd `LoadCredential`) — never in git

**Phase Core (LAN only)** — this repo :
- NATS JetStream bus internal to LXC 415
- Rust Axum daemon `hubmq` (~80 MB RAM)
- HTTP ingestion on port 8470 (`/in/wazuh`, `/in/forgejo`, `/in/generic`)
- Telegram bot in **polling mode** (no webhook exposed)
- Email delivery via SMTP Gmail (lettre)
- Apprise subprocess for multi-channel (100+ integrations)
- ntfy push **LAN only**
- Local P0 fallback (systemd OnFailure → direct email bypass)
- Filtering: dedup 60s, adaptive rate limit (100/min for P0, 10/min normal), quiet hours 22h-7h (P0/P1 bypass)

**Phase Exposure (later)** :
- ntfy published on `ntfy.mymomot.ovh` (Internet, Bearer + UUID topics)
- Telegram webhook instead of polling (IP allowlist Telegram CIDR + HMAC)
- Dashboard exposed at `hubmq.lab.mymomot.ovh` (SSO Authentik)

```
DOWNSTREAM (alerts → user)
  Wazuh ──┐
  Forgejo ┤                              ┌──► ntfy (push, LAN)
  BigBro  ┼──► NATS JetStream ──► hubmq ─┼──► Apprise email (SMTP)
  systemd ┤      6 streams       daemon  └──► Telegram bot
  cron ───┘

UPSTREAM (user → agents)
  User on Telegram ──► bot polling ──► NATS user.incoming ──► bridge
                      (forward reject, chat_id allowlist,          │
                       command whitelist)                          ▼
                                                            msg-relay :9480
                                                                   │
                                                                   ▼
                                                         Claude Code / ZeroClaw
```

Full details: [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md)

## Quick Start

### Prerequisites

- LXC 415 provisioned (Debian 13, SSH `hubmq` alias, sudo NOPASSWD)
- NATS JetStream installed + 2 NKeys generated
- SMTP credentials + Telegram bot token in `/etc/hubmq/credentials/` (chmod 600 root:root)
- See [docs/SETUP.md](./docs/SETUP.md) for complete from-zero provisioning

### Build + Deploy

```bash
# Build release binary
cargo build --release -p hubmq

# Deploy to LXC 415
bash deploy/deploy-hubmq.sh target/release/hubmq
```

### Verify

```bash
# Health check
curl -sf http://192.168.10.15:8470/health
# Expected: ok

# NATS streams
ssh hubmq "/usr/local/bin/nats stream ls \
  --nkey /etc/nats/nkeys/hubmq-service.seed \
  --server nats://localhost:4222"

# Web UI (SSO Authentik required)
open https://nui.lab.mymomot.ovh
```

### Ingest a test alert

```bash
curl -X POST http://192.168.10.15:8470/in/generic \
  -H "Content-Type: application/json" \
  -d '{
    "source":"test",
    "severity":"P2",
    "title":"Hello HubMQ",
    "body":"First test message",
    "tags":["test"]
  }'
```

Full E2E smoke test: `bash tests/integration/e2e.sh`

## Repository layout

```
hubmq/
├── Cargo.toml                       # workspace (2 crates)
├── crates/
│   ├── hubmq-core/                  # library
│   │   ├── src/
│   │   │   ├── config.rs            # TOML config loader
│   │   │   ├── message.rs           # Message + Severity (P0-P3)
│   │   │   ├── subjects.rs          # NATS subject builders (D8 sanitization)
│   │   │   ├── nats_conn.rs         # NATS client + JetStream streams (D2 limits)
│   │   │   ├── queue.rs             # SQLite WAL persistent queue
│   │   │   ├── audit.rs             # structured audit log (D4)
│   │   │   ├── filter/
│   │   │   │   ├── dedup.rs         # in-memory TTL dedup cache
│   │   │   │   ├── ratelimit.rs     # adaptive token bucket (P0 separate)
│   │   │   │   └── severity.rs      # quiet hours routing (D5)
│   │   │   ├── sink/
│   │   │   │   ├── email.rs         # lettre SMTP + Tera templates
│   │   │   │   ├── ntfy.rs          # HTTP POST + priority mapping
│   │   │   │   └── apprise.rs       # Python subprocess JSON stdin (V7 anti-injection)
│   │   │   ├── source/
│   │   │   │   ├── webhook.rs       # Axum routes /in/*
│   │   │   │   └── telegram.rs      # teloxide polling + forward rejection (B2)
│   │   │   ├── bridge.rs            # msg-relay bridge + command whitelist (B3)
│   │   │   ├── dispatcher.rs        # NATS consumers + fan-out to sinks
│   │   │   └── app_state.rs         # shared Arc<AppState>
│   │   └── migrations/
│   │       └── 001_init.sql         # outbox + audit tables
│   └── hubmq-bin/                   # daemon binary
│       └── src/main.rs              # wire-up + systemd LoadCredential (D3)
├── deploy/
│   ├── hubmq.service                # main systemd unit (LoadCredential + OnFailure)
│   ├── hubmq-fallback-p0.service    # OnFailure fallback (B1)
│   ├── hubmq-fallback-p0.sh         # direct SMTP bypass script
│   ├── hubmq-heartbeat.service      # health probe
│   ├── hubmq-heartbeat.timer        # 1h silence detector
│   ├── nats.service                 # NATS systemd unit
│   ├── nats-server.conf.template    # NATS config template (NKeys placeholders)
│   ├── config.toml.example          # example config.toml
│   ├── deploy-hubmq.sh              # scp + install + restart
│   └── README-nkeys.md              # NKey management guide
├── docs/
│   ├── plans/
│   │   └── 2026-04-12-hubmq-phase-core.md   # implementation plan (23 tasks)
│   ├── ARCHITECTURE.md              # flows, paths, security model, failure modes
│   ├── SETUP.md                     # from-zero reproducible setup (6 phases)
│   ├── OPERATIONS.md                # daily ops, rotation, troubleshooting
│   └── SUBJECTS.md                  # NATS subjects contract (D1)
├── tests/
│   └── integration/
│       └── e2e.sh                   # end-to-end smoke test
└── .forgejo/
    └── workflows/
        ├── ci.yml                   # fmt + clippy + test + build
        └── deploy.yml               # build + deploy to LXC 415
```

## Documentation

| Document | Audience | Content |
|---|---|---|
| [CLAUDE-HUBMQ.md](./CLAUDE-HUBMQ.md) | Tech Lead | Project overview, state, stack, council conditions, timeline |
| [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md) | Architects | Flows (down/upstream), security model, concurrency, observability, failure modes |
| [docs/SETUP.md](./docs/SETUP.md) | Ops | From-zero reproducible setup (LXC → NATS → NUI → deploy) |
| [docs/OPERATIONS.md](./docs/OPERATIONS.md) | Ops / Support | Monitoring, credential rotation, troubleshooting (5 cases), backup/restore |
| [docs/SUBJECTS.md](./docs/SUBJECTS.md) | Integrators | NATS subjects contract + stream limits |
| [docs/plans/2026-04-12-hubmq-phase-core.md](./docs/plans/2026-04-12-hubmq-phase-core.md) | Reference | Original 23-tasks implementation plan |

## Council conditions (14)

Architecture validated by 4 experts (Auditeur + Knox + SecurityAuditor + BigBrother) via `/homelab-gouvernance` council (2026-04-12). All 12 Phase Core conditions integrated. 2 remaining (B4 + B5) scheduled for Phase Exposure.

| ID | Condition | Phase | Status |
|---|---|---|---|
| B1 | Local P0 fallback (no Internet required) | Core | ✅ `hubmq-fallback-p0.service` |
| B2 | Reject Telegram forwarded messages | Core | ✅ `source/telegram.rs` |
| B3 | msg-relay bridge command whitelist | Core | ✅ `bridge.rs` |
| B4 | Traefik rate limit on ntfy public | Exposure | 📋 later |
| B5 | IP allowlist Telegram webhook (CIDR) | Exposure | 📋 later |
| B6 | ntfy native auth (Bearer + deny-all) | Core | ✅ config template ready |
| D1 | NATS subjects contract documented | Core | ✅ `docs/SUBJECTS.md` |
| D2 | JetStream limits explicit per stream | Core | ✅ `nats_conn::ensure_streams` |
| D3 | systemd LoadCredential (no env vars) | Core | ✅ `main::read_credential` |
| D4 | Audit log structured + Wazuh FIM | Core | ✅ `audit.rs` + FIM group |
| D5 | P0/P1 bypass quiet hours | Core | ✅ `Severity::bypasses_quiet_hours` |
| D6 | Adaptive rate limit (P0 bucket separate) | Core | ✅ `filter/ratelimit.rs` |
| D7 | Boot order + `After=nats.service` | Core | ✅ `deploy/hubmq.service` |
| D8 | NATS subject fixed identifiers | Core | ✅ `subjects::is_safe_ident` |

## Tech Stack

**Core** :
- [Rust](https://www.rust-lang.org) 1.93.1 (edition 2021)
- [tokio](https://tokio.rs) 1.40 — async runtime
- [axum](https://github.com/tokio-rs/axum) 0.7 — HTTP server
- [async-nats](https://github.com/nats-io/nats.rs) 0.38 — NATS client
- [teloxide](https://github.com/teloxide/teloxide) 0.13 — Telegram bot
- [sqlx](https://github.com/launchbadge/sqlx) 0.8 — SQLite WAL
- [lettre](https://github.com/lettre/lettre) 0.11 — SMTP
- [reqwest](https://github.com/seanmonstar/reqwest) 0.12 — HTTP client

**Infra** :
- [NATS Server](https://nats.io) 2.10.24 (JetStream)
- [NATS-NUI](https://github.com/nats-nui/nui) (web UI on LXC 412)
- [Apprise](https://github.com/caronc/apprise) (100+ notification services, Python)

## CI/CD

- **Primary** : Forgejo at `http://localhost:3000/motreffs/hubmq` on LXC 500
- **Mirror** : GitHub at `https://github.com/mymomot/hubmq` (Forgejo push mirror, sync on commit + 15 min fallback cron)
- **Runner** : `forge-actions` on LXC 500 (self-hosted, host mode, 6 cores, labels `self-hosted,linux,x64,lxc-500,ubuntu-latest`)
- **Workflows** :
  - `.forgejo/workflows/ci.yml` — fmt check + clippy (-D warnings) + test + build release + artifact upload
  - `.forgejo/workflows/deploy.yml` — build + scp + systemctl restart + smoke test on LXC 415

## License

Apache 2.0 — see [LICENSE](./LICENSE).

## Credits

Built for the [mymomot.ovh homelab](https://mymomot.ovh) by Stéphane motreffs with Claude Code (Anthropic) as pair-programmer.

Council validation by 4 specialized agents: Auditeur (spec/quality), Knox (infra), SecurityAuditor (threat modeling), BigBrother (monitoring).
