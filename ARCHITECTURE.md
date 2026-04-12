# Architecture — hubmq

> Généré le : 2026-04-12
> Commit ref : a55abda

## Vue d'ensemble

HubMQ est un service de communication unifié LAN-first. Il ingère des événements depuis NATS JetStream (Wazuh, Forgejo, cron, heartbeats), applique filtrage/déduplication/quiet-hours, puis dispatche vers Telegram, email SMTP, et ntfy. Un pont bidirectionnel msg-relay permet les commandes distantes.

```
[CORE] hubmq-core — bibliothèque métier
  ├── [CORE] Config — chargement TOML runtime
  │   ├── [SUB] NatsConfig → utilisé par NatsConn (Task 7)
  │   ├── [SUB] SmtpConfig → utilisé par EmailSender (Task 13)
  │   ├── [SUB] TelegramConfig → utilisé par TelegramSender (Task 11)
  │   ├── [SUB] FilterConfig → utilisé par DispatchEngine (Task 10)
  │   ├── [SUB] FallbackConfig → utilisé par FallbackHandler (Task 16)
  │   ├── [SUB] BridgeConfig → utilisé par BridgeListener (Task 18, optionnel)
  │   └── [SUB] NtfyConfig → utilisé par NtfySender (Task 14, optionnel)
  ├── [CORE] Message + Severity — type canonique de message
  │   ├── [SUB] Severity enum (P0-P3) → utilisé par DispatchEngine, FilterEngine
  │   │   ├── [UTILITY] from_wazuh_level(u8) → mappé depuis WazuhIngester
  │   │   └── [UTILITY] bypasses_quiet_hours() → utilisé par QuietHoursGuard
  │   └── [SUB] Message struct → utilisé par tous les modules
  │       ├── [UTILITY] Message::new() → factory constructeur
  │       └── [UTILITY] dedup_hash() → utilisé par DedupCache
  ├── [FEATURE] NatsConn — connexion + bootstrap streams (Task 7, à implémenter)
  │   ├── [SUB] connect(NatsConfig) → dépend de Config
  │   └── [SUB] ensure_streams() → crée ALERTS/MONITOR/AGENTS/SYSTEM/CRON/USER_IN
  ├── [FEATURE] Subjects — constructeurs de sujets NATS sécurisés (Task 7, à implémenter)
  │   └── [UTILITY] is_safe_ident() → garde [a-z0-9_-], rejette dots/wildcards
  ├── [FEATURE] WazuhIngester — consommateur NATS alert.wazuh.> (Task 8, à implémenter)
  │   └── [HOOK] on_alert → déclenche Message::new() + DispatchEngine
  ├── [FEATURE] DispatchEngine — orchestration dispatch multi-canal (Task 10, à implémenter)
  │   ├── [SUB] FilterEngine — rate-limit + quiet-hours + dedup
  │   │   ├── [SUB] DedupCache → persiste dans SQLite (Task 9)
  │   │   └── [SUB] QuietHoursGuard → utilise Severity::bypasses_quiet_hours()
  │   ├── [HOOK] dispatch(Message) → déclenche TelegramSender / EmailSender / NtfySender
  │   └── [SUB] FallbackHandler → email SMTP si canal principal KO
  ├── [FEATURE] TelegramSender — envoi Telegram + polling commandes (Task 11-12, à implémenter)
  │   └── [HOOK] on_command → déclenche BridgeListener (réponse)
  ├── [FEATURE] EmailSender — envoi SMTP via lettre (Task 13, à implémenter)
  ├── [FEATURE] NtfySender — notifications ntfy.sh (Task 14, à implémenter)
  └── [FEATURE] BridgeListener — pont msg-relay bidirectionnel (Task 18, à implémenter)

[CORE] hubmq-bin — binaire exécutable
  └── [SUB] main() → orchestre Config::from_file() + NatsConn + DispatchEngine + TelegramSender
```

## Table des relations

| De | Type | Vers | Note |
|---|---|---|---|
| Config | dépend de | fichier TOML `/etc/hubmq/config.toml` | chargé via `from_file()` |
| NatsConfig | utilisé par | NatsConn | url + nkey_seed_path |
| SmtpConfig | utilisé par | EmailSender | host/port/username/password_credential |
| TelegramConfig | utilisé par | TelegramSender | allowed_chat_ids + token_credential |
| FilterConfig | utilisé par | DispatchEngine.FilterEngine | dedup_window + rate_limit + quiet_hours |
| FallbackConfig | utilisé par | FallbackHandler | email_to + heartbeat_silence_max_secs |
| NtfyConfig | utilisé par | NtfySender | base_url + topic + bearer_credential |
| BridgeConfig | utilisé par | BridgeListener | msg_relay_url + command_whitelist |
| Severity | utilisé par | DispatchEngine, FilterEngine, WazuhIngester | routing + quiet-hours |
| Message | utilisé par | WazuhIngester, DispatchEngine, TelegramSender, EmailSender, NtfySender | type canonique |
| Message::dedup_hash() | utilise | DedupCache | clé SHA256 pour déduplication |
| Severity::from_wazuh_level() | utilisé par | WazuhIngester | mapping Wazuh level→Severity |
| Severity::bypasses_quiet_hours() | utilisé par | QuietHoursGuard | P0+P1 passent en quiet hours |
| DedupCache | persiste dans | SQLite | fenêtre configurable |
| hubmq-bin | dépend de | hubmq-core | lib métier |

## Fichiers critiques par fonctionnalité

| Fonctionnalité | Fichier(s) | État |
|---|---|---|
| Config TOML | `crates/hubmq-core/src/config.rs` | DONE (Task 5) |
| Message + Severity | `crates/hubmq-core/src/message.rs` | DONE (Task 6) |
| NatsConn + Subjects | `crates/hubmq-core/src/nats_conn.rs`, `subjects.rs` | À implémenter (Task 7) |
| WazuhIngester | `crates/hubmq-core/src/wazuh_ingester.rs` | À implémenter (Task 8) |
| DedupCache (SQLite) | `crates/hubmq-core/src/dedup_cache.rs` | À implémenter (Task 9) |
| DispatchEngine | `crates/hubmq-core/src/dispatch_engine.rs` | À implémenter (Task 10) |
| TelegramSender | `crates/hubmq-core/src/telegram_sender.rs` | À implémenter (Task 11-12) |
| EmailSender | `crates/hubmq-core/src/email_sender.rs` | À implémenter (Task 13) |
| NtfySender | `crates/hubmq-core/src/ntfy_sender.rs` | À implémenter (Task 14) |
| BridgeListener | `crates/hubmq-core/src/bridge_listener.rs` | À implémenter (Task 18) |
| Binaire principal | `crates/hubmq-bin/src/main.rs` | Stub (Task 4) |

## Services externes

### Consomme
| Service | URL/Port | Variable / Config | Usage |
|---|---|---|---|
| NATS JetStream | nats://localhost:4222 | `nats.url` + `nats.nkey_seed_path` | Ingestion événements (Wazuh, Forgejo, cron, heartbeats) |
| Telegram Bot API | api.telegram.org (HTTPS) | `telegram.token_credential` | Envoi notifications + polling commandes |
| SMTP Gmail | smtp.gmail.com:587 | `smtp.*` + `smtp.password_credential` | Emails fallback + alertes P2/P3 |
| ntfy.sh ou instance locale | configurable | `ntfy.base_url` + `ntfy.topic` | Notifications push mobile (Phase Exposure) |
| msg-relay | http://localhost:9480 (optionnel) | `bridge.msg_relay_url` | Pont bidirectionnel commandes distantes |

### Fournit
| Endpoint | Port | Consommé par | Usage |
|---|---|---|---|
| API Axum REST | TBD (Phase Core Task 19) | Claude Code, monitoring | Statut + gestion alertes |
| Wazuh webhook (optionnel) | TBD (Phase Exposure) | Wazuh manager LXC 412 | Ingestion directe HTTP |
