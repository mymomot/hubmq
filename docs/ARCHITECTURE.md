# Architecture — HubMQ

> Généré le : 2026-04-12
> Commit ref : 19a6bf9

Service de communication unifié homelab (LXC 415). Reçoit des alertes depuis NATS JetStream,
filtre (dédup + rate-limit + severity), distribue via email/ntfy/Telegram. Bidirectionnel :
commandes Telegram → msg-relay bridge → agents.

---

## Arbre fonctionnel

```
[CORE] hubmq-core — Bibliothèque centrale
  ├── [CORE] config — Chargement configuration TOML
  │   ├── [SUB] NatsConfig → utilisé par nats_conn
  │   ├── [SUB] SmtpConfig → utilisé par sink/email
  │   ├── [SUB] TelegramConfig → utilisé par source/telegram
  │   ├── [SUB] FilterConfig → utilisé par filter/*
  │   ├── [SUB] FallbackConfig → utilisé par audit, sink/email fallback
  │   ├── [SUB] BridgeConfig (optional, default) → utilisé par bridge
  │   └── [SUB] NtfyConfig (optional, default) → utilisé par sink/ntfy
  │
  ├── [CORE] message — Enveloppe canonique Message + Severity enum
  │   └── [SUB] subjects — Constructeurs de sujets NATS (D8 : pas de texte user)
  │
  ├── [CORE] nats_conn — Client NATS NKey authentifié
  │   ├── dépend de NatsConfig (config)
  │   └── utilisé par source/*, queue
  │
  ├── [FEATURE] filter — Pipeline de filtrage
  │   ├── [SUB] dedup — Fenêtre de déduplication 60s (SHA-256 hash)
  │   │   └── persiste dans SQLite (queue)
  │   ├── [SUB] ratelimit — Token bucket adaptatif (D6 : 10/min normal, 100/min P0)
  │   │   └── dépend de FilterConfig.rate_limit_*
  │   └── [SUB] severity — Routage P0→P3, bypass quiet hours (D5)
  │       └── dépend de FilterConfig.quiet_hours_*
  │
  ├── [FEATURE] source — Ingestion des messages
  │   ├── [SUB] webhook — Routes HTTP Axum /in/* (Wazuh, Forgejo, BigBrother, systemd, cron)
  │   │   └── déclenche filter → sink
  │   ├── [SUB] telegram — Polling Teloxide bidirectionnel
  │   │   ├── déclenche bridge (commandes entrantes)
  │   │   └── dépend de TelegramConfig
  │   └── [SUB] heartbeat — Surveillance inverse : alerte si silence > 3h
  │       └── utilise FallbackConfig.heartbeat_silence_max_secs
  │
  ├── [FEATURE] sink — Adaptateurs de livraison
  │   ├── [SUB] email — lettre SMTP Gmail (TLS) + templates Tera
  │   │   └── dépend de SmtpConfig
  │   ├── [SUB] ntfy — reqwest HTTP POST vers ntfy (LAN, Phase Core)
  │   │   └── dépend de NtfyConfig
  │   └── [SUB] apprise — Subprocess Python JSON stdin (anti-injection)
  │       └── dépend de SmtpConfig + TelegramConfig
  │
  ├── [FEATURE] bridge — Pont msg-relay bidirectionnel (B3 whitelist)
  │   └── dépend de BridgeConfig.command_whitelist
  │
  ├── [FEATURE] queue — File persistante SQLite WAL
  │   ├── persiste dans SQLite (LXC 415 local)
  │   └── utilisé par filter/dedup, audit
  │
  └── [FEATURE] audit — Log structuré JSON (D4 : Wazuh FIM)
      └── persiste dans SQLite

[CORE] hubmq-bin — Daemon systemd
  ├── dépend de hubmq-core (tous les modules)
  ├── [HOOK] main — Bootstrap tokio runtime + init config + démarre workers
  └── [HOOK] deploy/hubmq.service — LoadCredential D3 + After=nats.service D7
```

---

## Table des relations

| De | Relation | Vers |
|---|---|---|
| config | utilisé par | nats_conn, filter/*, source/*, sink/*, bridge, audit |
| message | utilisé par | source/*, sink/*, queue, audit |
| subjects | utilisé par | source/webhook, source/heartbeat |
| nats_conn | utilisé par | source/webhook, source/heartbeat, queue |
| filter/dedup | persiste dans | queue (SQLite) |
| filter/ratelimit | dépend de | config.FilterConfig |
| filter/severity | dépend de | config.FilterConfig |
| source/webhook | déclenche | filter → sink |
| source/telegram | déclenche | bridge |
| source/heartbeat | utilise | config.FallbackConfig |
| sink/email | dépend de | config.SmtpConfig |
| sink/ntfy | dépend de | config.NtfyConfig |
| sink/apprise | dépend de | config.SmtpConfig + TelegramConfig |
| bridge | dépend de | config.BridgeConfig |
| queue | persiste dans | SQLite (LXC 415 /var/lib/hubmq/) |
| audit | persiste dans | SQLite |
| hubmq-bin | dépend de | hubmq-core |

---

## Fichiers critiques par fonctionnalité

| Fonctionnalité | Fichiers |
|---|---|
| config (DONE Task 5) | `crates/hubmq-core/src/config.rs` |
| message / subjects | `crates/hubmq-core/src/message.rs`, `subjects.rs` (à créer) |
| nats_conn | `crates/hubmq-core/src/nats_conn.rs` (à créer) |
| filter | `crates/hubmq-core/src/filter/{mod,dedup,ratelimit,severity}.rs` (à créer) |
| source | `crates/hubmq-core/src/source/{mod,webhook,telegram,heartbeat}.rs` (à créer) |
| sink | `crates/hubmq-core/src/sink/{mod,email,ntfy,apprise}.rs` (à créer) |
| bridge | `crates/hubmq-core/src/bridge.rs` (à créer) |
| queue | `crates/hubmq-core/src/queue.rs` (à créer) |
| audit | `crates/hubmq-core/src/audit.rs` (à créer) |
| daemon | `crates/hubmq-bin/src/main.rs` |
| deploy | `deploy/hubmq.service`, `deploy/hubmq-fallback-p0.service`, `deploy/config.toml.example` |

---

## Services externes

### Consomme

| Service | URL / Port | Variable env / Config | Usage |
|---|---|---|---|
| NATS JetStream (local) | `nats://localhost:4222` | `config.nats.url` | Bus de messages (6 streams) |
| SMTP Gmail | `smtp.gmail.com:587` | `config.smtp.*` + credential `/etc/hubmq/credentials/gmail-app-password` | Livraison email |
| ntfy (LAN) | configurable | `config.ntfy.base_url` | Push notifications LAN |
| Apprise | subprocess local | — | Multi-channel delivery (email + Telegram) |
| Telegram API | `https://api.telegram.org` | credential `telegram-bot-token` | Bot bidirectionnel polling |
| msg-relay bridge | `config.bridge.msg_relay_url` | `config.bridge.msg_relay_url` | Bridge commandes → agents LXC 500 |

### Fournit

| Endpoint | Port | Consommé par | Usage |
|---|---|---|---|
| `POST /in/wazuh` | TBD (Axum) | Wazuh webhook | Ingestion alertes Wazuh |
| `POST /in/forgejo` | TBD (Axum) | Forgejo webhook | Ingestion CI/deploy failures |
| `POST /in/*` | TBD (Axum) | BigBrother, systemd OnFailure, cron | Ingestion alertes génériques |
| Telegram polling | — | Telegram API (polling sortant) | Commandes Stéphane → bridge |
