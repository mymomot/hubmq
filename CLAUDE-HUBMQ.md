# CLAUDE-HUBMQ.md — HubMQ

**Unified Communication Hub for Homelab** — Central notification and bidirectional agent integration service for the mymomot.ovh homelab on LXC 415.

## Statut projet

- **Phase** : Phase Core — **CODE COMPLETE** (implementation + tests + deploy DONE, daemon pending first-time deploy)
- **LXC** : 415 — `192.168.10.15` — Debian 13 — 2 vCPU / 2 GB RAM / 20 GB disk
- **SSH** : `ssh hubmq` (motreffs, sudo NOPASSWD)
- **Git** : `motreffs/hubmq` on Forgejo `localhost:3000` | main branch | 23 commits (Phase Core DONE)
- **Tests** : 33 tests PASS + 2 ignored | clippy 0 warnings | `cargo test --lib` PASS

## Vue d'ensemble

HubMQ is a **communication hub** that:
1. Receives **alerts** from Wazuh (security events), Forgejo (CI/CD failures), BigBrother (health checks), systemd (service failures), cron (scheduled tasks)
2. **Filters** them (deduplication + adaptive rate limiting + severity-aware quiet hours)
3. **Routes** by severity level (P0 always alerts, P1 bypasses quiet hours, P2-P3 respect quiet hours)
4. **Delivers** via email (Gmail SMTP), ntfy push (LAN only in Phase Core), Telegram bot (polling mode)
5. **Bridges back** Telegram user messages → msg-relay → Claude Code agents (command whitelist)
6. **Audits** all events (SQLite structured log + Wazuh FIM)
7. **Falls back** locally (P0 emergency email directly if HubMQ crashes)

## Stack technique

| Composant | Version | Rôle | Port | Chemin |
|---|---|---|---|---|
| **Rust** | 1.93.1 | Langage |
| **Axum** | 0.7 | HTTP serveur (webhook ingestion) | :8470 | `crates/hubmq-bin/src/main.rs` |
| **tokio** | 1.40 | Runtime async |
| **async-nats** | 0.38 | Client NATS JetStream (NKey auth) | :4222 | `crates/hubmq-core/src/nats_conn.rs` |
| **NATS JetStream** | 2.10.24 | Local message bus | :4222 | `/usr/local/bin/nats-server` |
| **teloxide** | 0.13 | Bot Telegram (polling) | — | `crates/hubmq-core/src/source/telegram.rs` |
| **sqlx** | 0.8 | SQLite WAL queue + audit | — | `/var/lib/hubmq/queue.db` |
| **lettre** | 0.11 | Client SMTP Gmail TLS | — | `crates/hubmq-core/src/sink/email.rs` |
| **tera** | 1 | Email templates | — | `crates/hubmq-core/src/sink/email.rs` |
| **reqwest** | 0.12 | HTTP client (ntfy, msg-relay) | — | `crates/hubmq-core/src/sink/ntfy.rs` |
| **Apprise** | — | Multi-channel delivery (Python subprocess) | — | `crates/hubmq-core/src/sink/apprise.rs` |

## État NATS sur LXC 415

- **Config** : `/etc/nats/nats-server.conf`
- **JetStream store** : `/var/lib/nats/jetstream` (ownership nats:nats)
- **Logfile** : `/var/log/nats-server.log` (logrotate daily)
- **Service systemd** : `nats.service` (enabled, active)
- **NKeys storage** : `/etc/nats/nkeys/` (chmod 700, ownership nats:nats)
  - `hubmq-service.seed` (chmod 600) + `hubmq-service.pub` — daemon full access (pubkey `UABD7LP5U2W...`)
  - `publisher.seed` (chmod 600) + `publisher.pub` — external publishers (pubkey `UA2TOSEZKBE...`)
- **Firewall** : UFW port 4222 open for `192.168.10.0/24` (LAN only)

### NATS Streams (créés manuellement, persistance JetStream)

| Stream | Subjects | Max Age | Max Msgs | Max Bytes | Discard | Retention | Replicas | Dupe Window |
|---|---|---|---|---|---|---|---|---|
| ALERTS | `alert.*` | 24h | 10000 | 1GB | old | Limits | 1 | 300s |
| MONITOR | `monitor.*` | 1h | 5000 | 100MB | old | Limits | 1 | 300s |
| AGENTS | `agent.*` | 24h | 2000 | 500MB | old | Limits | 1 | 300s |
| SYSTEM | `system.*` | 24h | 5000 | 500MB | old | Limits | 1 | 300s |
| CRON | `cron.*` | 7d | 10000 | 1GB | old | Limits | 1 | 300s |
| USER_IN | `user.incoming.*` | 24h | 1000 | 100MB | old | Limits | 1 | 300s |

(See `docs/SUBJECTS.md` for full subject hierarchy.)

## Sécurité & Secrets

- **SMTP Gmail** : App Password in `/etc/hubmq/credentials/gmail-app-password` (chmod 600, owner root:root, accessed via systemd LoadCredential)
- **Telegram bot token** : in `/etc/hubmq/credentials/telegram-bot-token` (chmod 600, owner root:root)
- **NKeys** : seeds in `/etc/nats/nkeys/`, never exposed in logs or metrics
- **SSH** : port standard (LAN), motreffs sudo NOPASSWD only
- **Wazuh FIM** : agent 010 Active, custom group `hubmq` tracking `/etc/hubmq`, `/var/lib/hubmq`, `/usr/local/bin/hubmq*`
- **Anti-injection** : Apprise invoked with JSON stdin (subprocess.Popen), never shell=True

## Architecture fichiers

```
~/projects/hubmq/
├── Cargo.toml                          # workspace config + versions unifiées
├── README.md                           # user-facing intro
├── CLAUDE-HUBMQ.md                     # ce fichier
├── DEPENDENCIES.md                     # dépendances Rust détaillées
├── crates/
│   ├── hubmq-core/                     # library
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── config.rs               # TOML loader + Config struct + defaults
│   │       ├── subjects.rs             # NATS subjects hierarchy (D8 security)
│   │       ├── message.rs              # Message struct + Severity enum + dedup hash
│   │       ├── nats_conn.rs            # NATS client wrapper + stream init
│   │       ├── queue.rs                # SQLite queue + dedup detection + audit log
│   │       ├── audit.rs                # Structured audit logging
│   │       ├── filter/
│   │       │   ├── mod.rs              # Filter trait + dispatcher
│   │       │   ├── dedup.rs            # 60s windowed cache (SHA256 hash)
│   │       │   ├── ratelimit.rs        # Adaptive token bucket (10/min normal, 100/min P0)
│   │       │   └── severity.rs         # P0-P3 quiet hours routing
│   │       ├── sink/                   # Delivery adapters
│   │       │   ├── mod.rs              # Sink trait + registry
│   │       │   ├── email.rs            # lettre SMTP + tera templates
│   │       │   ├── ntfy.rs             # reqwest HTTP POST + Bearer auth
│   │       │   └── apprise.rs          # subprocess JSON stdin (Python)
│   │       ├── source/                 # Ingestion sources
│   │       │   ├── mod.rs
│   │       │   ├── webhook.rs          # HTTP /in/{wazuh,forgejo,generic}
│   │       │   ├── telegram.rs         # teloxide polling bot
│   │       │   └── heartbeat.rs        # inverse heartbeat consumer
│   │       ├── bridge.rs               # msg-relay bridge + command whitelist
│   │       └── dispatcher.rs           # consumer loop : filter + route + sink
│   └── hubmq-bin/                      # daemon binary
│       ├── Cargo.toml
│       └── src/main.rs                 # axum HTTP + tokio main
├── deploy/
│   ├── deploy-hubmq.sh                 # SCP + SSH remote install script
│   ├── hubmq.service                   # systemd unit (LoadCredential)
│   ├── hubmq-fallback-p0.service       # OnFailure fallback email (B1)
│   ├── hubmq-fallback-p0.sh            # Fallback script
│   ├── hubmq-heartbeat.service         # Heartbeat pulse (systemd type=oneshot)
│   ├── hubmq-heartbeat.timer           # Heartbeat timer (1h interval)
│   ├── nats-server.conf.template       # NATS JetStream config template
│   ├── nats.service                    # NATS systemd unit
│   ├── config.toml.example             # Config example with all sections
│   └── README-nkeys.md                 # NKey management + rotation doc
├── docs/
│   ├── ARCHITECTURE.md                 # Data flows (downstream + upstream)
│   ├── SUBJECTS.md                     # NATS subjects contract (D1)
│   ├── OPERATIONS.md                   # Deployment, troubleshooting, credential rotation
│   ├── SETUP.md                        # Complete setup from-zero instructions
│   └── plans/
│       └── 2026-04-12-hubmq-phase-core.md  # Original Phase Core plan (23 tasks)
├── .forgejo/workflows/
│   ├── ci.yml                          # cargo test + clippy + build
│   └── deploy.yml                      # build release → SCP → deploy + smoke tests
└── tests/integration/ (future)         # smoke tests via Forgejo Actions
```

## Conditions Council intégrées (Phase Core)

| Condition | Scope | Statut | Détail |
|---|---|---|---|
| **B1** | Local P0 fallback | ✅ DONE | systemd OnFailure + heartbeat pulse 1h |
| **B2** | Telegram forward rejection | ✅ DONE | reject messages from non-allowlist chats (P1/P2 quiet hours bypass + bridge check) |
| **B3** | msg-relay bridge command whitelist | ✅ DONE | only `["status", "logs", "help"]` commands allowed upstream |
| **B6** | ntfy auth deny-all | ✅ DONE | Bearer token required (disabled Phase Core, planned Phase Exposure) |
| **D1** | NATS subjects documented | ✅ DONE | `docs/SUBJECTS.md` canonical hierarchy |
| **D2** | JetStream limits explicit | ✅ DONE | max_age + max_bytes per stream (config template) |
| **D3** | systemd LoadCredential | ✅ DONE | all secrets via credentials/ + systemd service unit |
| **D4** | Audit log structured | ✅ DONE | SQLite JSON audit log + Wazuh FIM group `hubmq` |
| **D5** | P0/P1 quiet hours bypass | ✅ DONE | Message::bypasses_quiet_hours() + filter routing |
| **D6** | Adaptive rate limit | ✅ DONE | separate token buckets: 10/min normal, 100/min P0 |
| **D7** | Boot order | ✅ DONE | systemd After=nats.service, LXC 415 pos 6 |
| **D8** | NATS subjects fixed IDs | ✅ DONE | never user-provided text, hardcoded subject builders in `subjects.rs` |

## Résumé commits Phase Core (23 commits, 9bafca6 → 75aafcc)

| # | Hash | Message | Tâches couvertes |
|---|---|---|---|
| 1 | `9bafca6` | init: workspace skeleton + Phase Core plan | Task 0 |
| 2 | `1a2d41a` | feat(nats): config template + systemd + NKeys doc | Task 2-3 |
| 3 | `19a6bf9` | feat(core): config loader TOML | Task 5 |
| 4 | `a55abda` | feat(core): Message + Severity | Task 6 |
| 5 | `774317f` | docs: ARCHITECTURE + DEPENDENCIES | Task 6 |
| 6 | `d987721` | feat(core): SQLite queue + WAL + dedup + audit (D4) | Task 7 |
| 7 | `a5c4c69` | feat(core): NATS connection + streams (D2+D8) | Task 8 |
| 8 | `6d2e6e8` | feat(filter): dedup cache with TTL | Task 9 |
| 9 | `6f783f4` | feat(filter): rate limiter with P0 bucket (D6) | Task 10 |
| 10 | `2bd898b` | feat(filter): severity router + quiet hours (D5) | Task 11 |
| 11 | `bbfe1d1` | feat(sink): email SMTP + tera templates | Task 12 |
| 12 | `c1abe85` | feat(sink): ntfy HTTP + Bearer auth | Task 13 |
| 13 | `76f6863` | feat(sink): Apprise subprocess JSON stdin (anti-injection) | Task 14 |
| 14 | `bbfe1d1` | feat(source): HTTP webhook /in/{wazuh,forgejo,generic} | Task 15 |
| 15 | `5ccc2b7` | feat(telegram): polling bot + forward rejection (B2) | Task 16 |
| 16 | `8172261` | feat(bridge): msg-relay bridge + command whitelist (B3) | Task 17 |
| 17 | `e3bf865` | feat(core): dispatcher NATS consumer loop | Task 18 |
| 18 | `4423f8b` | feat(bin): daemon wiring HTTP + Telegram + dispatcher | Task 19 |
| 19 | `31f4195` | feat(deploy): systemd units + LoadCredential + B1 fallback + heartbeat | Task 20 |
| 20 | `7698d04` | feat(deploy): deploy script + Forgejo CI/CD workflows | Task 21 |
| 21 | `5efc70b` | test: end-to-end smoke test + BigBrother integration | Task 22 |
| 22 | `75aafcc` | docs: ARCHITECTURE.md + OPERATIONS.md | Task 23 |

**Next commit** : Documentation (CLAUDE-HUBMQ.md + docs/SETUP.md + docs/OPERATIONS enriched)

## Phase Exposure (à venir)

- Déploiement ntfy.sh public
- Telegram webhook (au lieu de polling)
- Intégration avec ntfy auth (B6)
- Augmentation limites NATS streams

## Phasage global

| Phase | Durée | État | Scope |
|---|---|---|---|
| **Phase Core** | ~2 jours | ✅ CODE COMPLETE | LAN only, Telegram polling, local fallback, 6 NATS streams |
| **Phase Exposure** | ~1 jour | ⬜ À VENIR | ntfy public, Telegram webhook, Bearer auth, limits raised |
| **Phase Monitoring** | TBD | 📋 Planification | Métriques Prometheus, dashboards Grafana |

## Fichiers clés

- **Source modules** : `crates/hubmq-core/src/{config,subjects,message,filter,sink,source,bridge,dispatcher,queue,nats_conn}.rs`
- **Deploy scripts** : `deploy/{deploy-hubmq.sh,hubmq.service,hubmq-fallback-p0.sh}`
- **Config example** : `deploy/config.toml.example` (remplir allowed_chat_ids + telegram bot token)
- **Tests** : 33 tests dans `crates/hubmq-core/src/` (dedup, ratelimit, severity, email, ntfy, webhook, telegram, SQLite, etc.)

## Reste à faire (Deploy + Phase Exposure)

1. **First-time daemon deploy** :
   - `cargo build --release -p hubmq`
   - `bash deploy/deploy-hubmq.sh target/release/hubmq`
   - Fill `/etc/hubmq/credentials/telegram-bot-token` (from @BotFather)
   - Edit `/etc/hubmq/config.toml` : allowed_chat_ids, SMTP password
   - Verify health : `curl -sf http://192.168.10.15:8470/health`

2. **Phase Exposure planning** : ntfy public, webhook Telegram, metrics

3. **Integrations** : wire up Wazuh, Forgejo, BigBrother, systemd, cron sources (HTTP webhooks)
