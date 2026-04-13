# HubMQ

> **Unified communication hub for the mymomot.ovh homelab** — N-bot Telegram + email IMAP ingestion, declarative routing per-agent, admin automation via Gmail, BigBrother curated observability, hot-reload registry. NATS JetStream at core, Rust Axum daemon, no webhook exposure in Phase Core.

[![License](https://img.shields.io/badge/license-Apache%202.0-blue)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.93.1-orange)](https://www.rust-lang.org)
[![NATS](https://img.shields.io/badge/NATS-2.10.24-green)](https://nats.io)
[![Version](https://img.shields.io/badge/version-1.0.0-brightgreen)](./CHANGELOG.md)
[![Phase](https://img.shields.io/badge/phase-Core%20%2B%20P2%20%2B%20P3%20%2B%20P4%20%2B%20P5%20✓-brightgreen)](./CHANGELOG.md)

## Status — v1.0.0 (2026-04-13) — first operational release

**5 phases completed, 110 tests PASS, E2E validated 3.02s** (Telegram → NATS → llmcore Qwen3.5-122B → reply).

- ✅ **Phase Core** (2026-04-12) : ingestion multi-source + 6 NATS streams + dispatcher + Apprise Telegram + SMTP Gmail + fallback B1
- ✅ **P2.4 + bridge bearer auth** (2026-04-13) : bypass whitelist verbe chat_id allowlisté + bearer credential msg-relay
- ✅ **Phase 2 jumeau hardening** (2026-04-13) : lock anti-double, rotation session JSONL, Wazuh FIM groupe `hubmq-agent`
- ✅ **Phase 3 BigBrother** (2026-04-13) : agent autonome horaire, dispatcher streams `[AGENTS, CRON]`, ALERTS/MONITOR/SYSTEM archive-only
- ✅ **Phase 4 watchdog** (2026-04-13) : heartbeat BB + timer 30min + fallback DM direct si BB silencieux
- ✅ **Phase 5 + 5.2 multi-bot + email source** (2026-04-13) : N bots déclaratifs conf.d/*.toml, SIGHUP hot-reload, listener `llm-openai-compat` générique, admin consumer NATS, source email IMAP + sink `email_reply` threaded, routing source-aware
- ✅ **End-to-end** : `@hubmq_llmcore_bot` → Qwen3.5-122B via `llm-free-gateway :8430` → DM reply **3.02s**
- 📋 **V2 backlog** : `admin.agent.add`, kind `custom-http` (zeroclaw/gemini), UI dashboard, Phase Exposure (ntfy public + Telegram webhook Internet)

## Architecture — multi-source, declarative, self-admin

## Telegram Bots (N bots, declarative)

Registry `conf.d/bots.toml` : chaque entrée `[[bot]]` = 1 polling task indépendant + token dédié + allowlist chat_ids + agent cible.

Bots actifs (v1.0.0) :
- [`@hubmqbot`](https://t.me/hubmqbot) → agent `claude-hubmq` (jumeau Claude Code, subject `user.incoming.telegram` legacy)
- `@hubmq_llmcore_bot` → agent `llmcore` (Qwen3.5-122B via gateway, subject `user.inbox.llmcore`)

Caractéristiques communes :
- **Mode** : polling (no webhook exposure Phase Core)
- **Allowlist per-bot** : `allowed_chat_ids = [...]` in `conf.d/bots.toml`
- **Security** : forwarded messages rejected (B2), unknown chat_id silently dropped + audit
- **Routing** : `target_agent` détermine le subject publish (NATS USER_IN)
- **Tokens** : `/etc/hubmq/credentials/telegram-bot-token-<name>` (0640 root:hubmq) — créés automatiquement par admin consumer, jamais dans git

### Ajout d'un bot — flow automatisé via Gmail

1. Créer bot via `@BotFather`, récupérer token.
2. Email depuis `motreff@gmail.com` vers `mymomot74@gmail.com` :
   - **Subject** : `HUBMQ_BOT_TOKEN <bot_name_interne>`
   - **Body** :
     ```
     agent: <target_agent>
     token: <telegram_bot_token>
     ```
3. Source IMAP hubmq détecte, extrait, publish `admin.bot.add` NATS.
4. Admin consumer valide (regex + cross-ref agent existe), atomic write credential + `conf.d/bots.toml`, SIGHUP self, polling task spawnée.
5. Reply email auto dans le thread : bot actif.

Aucune intervention shell requise. Le token ne transite **jamais** via Telegram chat (log serveur, historique client) — seul canal : emails end-to-end Gmail-to-Gmail (MFA activé des 2 côtés).

## Email source (symmetric USER_IN channel)

Polling IMAP Gmail toutes les 30s (`imap.gmail.com:993` + app password `gmail-app-password` réutilisé de SMTP) :
- Allowlist `From:` stricte avec extraction angle-bracket (anti-substring bypass)
- Subject routing : `HUBMQ_BOT_TOKEN <name>` → admin pipe | `[<agent>] <msg>` ou `<agent>: <msg>` → `user.inbox.<agent>`
- Mark as read post-traitement (éviter re-delivery)
- Reply auto dans le thread via `EmailReplySink` (headers `In-Reply-To` + `References`)

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
