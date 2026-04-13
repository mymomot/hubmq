# Architecture — HubMQ v1.0.0

> Mis à jour : 2026-04-13 · Tag : `v1.0.0`

## Vue d'ensemble

HubMQ est un **communication hub multi-source** homelab. Deux canaux USER_IN symétriques (Telegram multi-bot + email IMAP), routage déclaratif vers N agents (Claude Code jumeau, LLM OpenAI-compat, etc.), admin auto-provisionné via Gmail, observabilité curée via BigBrother + watchdog.

Stack : Rust Axum daemon, NATS JetStream (6 streams), SQLite audit, systemd LoadCredential, SIGHUP hot-reload.

## Diagramme haut-niveau

```
                          ┌──────────────────────────────────────────────┐
                          │ /etc/hubmq/conf.d/                           │
                          │   agents.toml  (N agents : claude-hubmq,     │
                          │                  llmcore, gemini, …)         │
                          │   bots.toml    (N bots : claude, hubmqlocal) │
                          └──────────────────────────────────────────────┘
                                           ▲
                                           │ SIGHUP atomic write
                                           │
DOWNSTREAM (alerts)                        │
  Wazuh  ──┐                               │
  Forgejo ─┤                               ▼
  systemd ─┼─► NATS JetStream ──► BigBrother ──► agent.bigbrother.summary
  cron   ──┤   [ALERTS,MONITOR,                       │
  ...    ──┘    SYSTEM,CRON,                          ▼
               AGENTS,USER_IN]             hubmq dispatcher ──► telegram_sinks[bot_name]
                    ▲                                │         ──► email_reply (thread)
                    │                                │         ──► ntfy push
UPSTREAM (user → agent)                              │         ──► email SMTP
  Telegram bots (N polling) ──► user.incoming.*    OR
    @hubmqbot     → claude-hubmq           user.inbox.<agent>
    @hubmq_llm_bot → llmcore                          │
                                                      ▼
  Gmail IMAP source ──────────► user.inbox.<agent>  listener llm-openai-compat
    From: motreff@gmail.com     OR                     POST agent.endpoint
    Subject: [<agent>] msg      admin.bot.add          │
    Subject: HUBMQ_BOT_TOKEN       │                   ▼
                                   ▼              publish agent.<agent>.response
                             admin consumer            (P1, source-aware routing)
                               atomic write
                               conf.d + SIGHUP
```

## Modules Rust

### hubmq-core (lib)

- **`config`** : `BotEntry`, `AgentEntry`, `Registry`, `load_conf_d(glob)`, backward compat `[telegram]` monolithique → `BotEntry` "default"
- **`source::telegram`** : `run_bot(state, bot_entry, token)` — 1 polling task par bot, allowlist per-bot, inject `meta.bot_name` + `meta.target_agent`, routing subject par `bot.target_agent`
- **`source::email`** : polling IMAP Gmail 30s, allowlist `From:` strict (angle-bracket extract), subject routing (`HUBMQ_BOT_TOKEN` → admin pipe | `[<agent>]`/`<agent>:` → `user.inbox.<agent>`), mark-as-read
- **`sink::apprise`** : multi-URL Apprise subprocess, Telegram via `tgram://TOKEN/CHAT_ID`
- **`sink::email`** : SMTP Gmail STARTTLS via `lettre`
- **`sink::email_reply`** : SMTP reply thread (`In-Reply-To` + `References` + `Subject: Re: ...`)
- **`sink::ntfy`** : HTTP POST ntfy (LAN, optionnel)
- **`listener::llm_openai`** : subscribe NATS `user.inbox.<agent>`, POST OpenAI-compat, publish `agent.<name>.response` severity P1
- **`admin`** : consumer `admin.bot.add`/`admin.bot.remove`, validation regex + cross-refs, atomic write credentials + conf.d, SIGHUP self via `nix::sys::signal::kill`, audit SQLite (token hash sha256[0:12])
- **`bridge`** : forward msg-relay + bearer auth + bypass whitelist verbe pour chat_id allowlisté
- **`dispatcher`** : consume AGENTS + CRON streams, routing source-aware (`meta.source = email` → email_reply, sinon `telegram_sinks[bot_name]`), severity router (quiet hours + rate limit + dedup)
- **`nats_conn`** : 6 streams (`ALERTS`, `MONITOR`, `AGENTS`, `SYSTEM`, `CRON`, `USER_IN`), `update_stream` idempotent avec fallback `get_or_create`
- **`audit`**, **`queue`** : SQLite pool partagé pour audit log + dedup cache
- **`message`** : canonical `Message` struct + `Severity` enum (P0-P3) + `bypasses_quiet_hours()`

### hubmq-bin

- **`main.rs`** : charge `Config` + `Registry::load_conf_d`, `spawn_all` (N polling bots + M listeners + dispatcher + admin consumer + email source), `tokio::select!` entre `ctrl_c()` et `SIGHUP` → reload diff + drain + respawn (abort all active before spawn_all pour éviter 409 Telegram)

## Streams NATS (stockage + rétention)

| Stream | Subjects | Retention | Max msgs | Max bytes | Consommé par |
|---|---|---|---|---|---|
| `ALERTS` | `alert.>` | 7 j | 50k | 500 MB | BigBrother (archive) |
| `MONITOR` | `monitor.>` | 1 j | 10k | 100 MB | BigBrother (archive) |
| `SYSTEM` | `system.>` | 3 j | 5k | 100 MB | BigBrother (archive) |
| `CRON` | `cron.>` | 1 j | 5k | 50 MB | dispatcher direct |
| `AGENTS` | `agent.>`, `admin.>` | 7 j | 10k | 200 MB | dispatcher (agent.*.response) + admin consumer (admin.bot.>) |
| `USER_IN` | `user.incoming.>`, `user.inbox.>` | 30 j | 10k | 500 MB | bridge (forward msg-relay) + listeners `llm-openai-compat` + hubmq-agent-listener externe |

## Agents

Deux kinds supportés en V1 (extensibles V2) :

### `claude-code-spawn` — ex: claude-hubmq

Agent jumeau Claude Code sur LXC 500 (hors daemon hubmq). Listener externe `hubmq-agent-listener.service` subscribe `user.incoming.telegram`, spawn `claude --print --continue --setting-sources user` avec prompt injecté (thread vault-mem + contexte runtime). Réponse via `send-telegram.sh` direct API.

Workspace sandboxé `~/.hubmq-agent/workspace/` (permissions.deny sur CLAUDE.md/CONSTITUTION.md/agents/skills). Lock anti-double `/tmp/hubmq-claude.owner`. Rotation JSONL >24h. Wazuh FIM sur workspace + wrapper + credentials + unit.

### `llm-openai-compat` — ex: llmcore

Listener Rust interne au daemon hubmq. Subscribe `user.inbox.<name>`, POST `endpoint` OpenAI-compat (`{model, messages: [system, user]}`), parse `.choices[0].message.content`, publish `agent.<name>.response` severity P1 (bypass quiet hours). Ack NATS après publish (at-least-once).

Config : `endpoint`, `model`, `system_prompt`, `timeout_secs`. Réutilise meta (`bot_name`, `source`, `chat_id`, `email_*`) pour routage retour.

## Admin pipeline

Flow sécurisé pour ajouter un bot depuis Gmail (aucun shell requis) :

```
1. Stéphane → @BotFather → bot créé → token
2. Stéphane → email motreff@gmail.com → mymomot74@gmail.com
   Subject: HUBMQ_BOT_TOKEN <bot_name_interne>
   Body:
     agent: <target_agent>
     token: <telegram_bot_token>

3. source::email (polling 30s) détecte :
   - Allowlist From: strict (angle-bracket extract + comparaison exacte)
   - Parse subject → bot_name
   - Parse body → agent + token (via extract_admin_field)
   - Validation lowercase (aligné admin::is_valid_name)
   - Publish admin.bot.add {name, target_agent, token, requester_email, ...}
   - Mark email read

4. admin consumer :
   - Validation regex + cross-refs (target_agent existe)
   - Atomic write /etc/hubmq/credentials/telegram-bot-token-<name> (tmp+rename, 0640 root:hubmq)
   - Atomic write conf.d/bots.toml (backup .bak + rename)
   - SIGHUP self → main.rs reload + spawn polling task
   - Audit SQLite (token hash sha256[0:12])
   - Publish ack sur agent.hubmq.admin.response (severity P3)

5. Dispatcher route ack selon source :
   - source=email → email_reply dans le thread Gmail
   - source=telegram → telegram_sinks[bot_name]
```

## Fichiers critiques

| Fonctionnalité | Fichier | État |
|---|---|---|
| Registry multi-bot + loader conf.d | `crates/hubmq-core/src/config.rs` | LIVE v1.0.0 |
| Source Telegram N polling | `crates/hubmq-core/src/source/telegram.rs` | LIVE v1.0.0 |
| Source Email IMAP | `crates/hubmq-core/src/source/email.rs` | LIVE v1.0.0 (P5.2) |
| Sink Telegram Apprise | `crates/hubmq-core/src/sink/apprise.rs` | LIVE v0.1.0 |
| Sink Email SMTP | `crates/hubmq-core/src/sink/email.rs` | LIVE v0.1.0 |
| Sink Email Reply | `crates/hubmq-core/src/sink/email_reply.rs` | LIVE v1.0.0 (P5.2) |
| Sink ntfy (LAN) | `crates/hubmq-core/src/sink/ntfy.rs` | LIVE v0.1.0 |
| Listener llm-openai-compat | `crates/hubmq-core/src/listener/llm_openai.rs` | LIVE v1.0.0 |
| Admin consumer NATS | `crates/hubmq-core/src/admin.rs` | LIVE v1.0.0 |
| Bridge msg-relay | `crates/hubmq-core/src/bridge.rs` | LIVE v0.1.0 + bearer auth v0.2.0 |
| Dispatcher source-aware | `crates/hubmq-core/src/dispatcher.rs` | LIVE v1.0.0 |
| NATS conn + streams | `crates/hubmq-core/src/nats_conn.rs` | LIVE v0.1.0 + USER_IN étendu + AGENTS admin.> v1.0.0 |
| Main + SIGHUP handler | `crates/hubmq-bin/src/main.rs` | LIVE v1.0.0 |
| Systemd unit + LoadCredential | `deploy/hubmq.service` | LIVE v1.0.0 + OnFailure fix + ReadWritePaths conf.d |
| Deploy script | `deploy/deploy-hubmq.sh` | LIVE v1.0.0 + sudo test -f |
| Bootstrap conf.d | `deploy/conf.d/{agents,bots}.toml` | LIVE v1.0.0 |

## Services externes

### Consomme
| Service | URL/Port | Variable/Config | Usage |
|---|---|---|---|
| NATS JetStream | `nats://localhost:4222` | `nats.url` + `nats.nkey_seed_path` | 6 streams |
| Telegram Bot API | `api.telegram.org` (HTTPS) | `/etc/hubmq/credentials/telegram-bot-token-<name>` | Multi-bot polling |
| Gmail SMTP | `smtp.gmail.com:587` | `smtp.*` + `credentials/gmail-app-password` | Email outbound + reply |
| Gmail IMAP | `imap.gmail.com:993` | `email_source.*` + même credential | Ingestion email |
| `llm-free-gateway` | `http://192.168.10.99:8430` | `agents.toml[[agent]].endpoint` | Routing LLM multi-modèle |
| msg-relay | `http://192.168.10.99:9480` | `bridge.msg_relay_url` + bearer | Pont bidirectionnel Claude Code |

### Fournit
| Endpoint | Port | Consommé par | Usage |
|---|---|---|---|
| HTTP ingestion | `0.0.0.0:8470` (`/in/wazuh`, `/in/forgejo`, `/in/generic`) | Wazuh LXC 412, scripts cron, agents | Ingestion alertes |
| NATS streams | `:4222` (bind local) | BigBrother, claude-hubmq listener, admin consumer | Bus interne |

## BigBrother (observabilité curée)

Agent autonome Claude Code sur LXC 500 (hors daemon hubmq). Timer systemd 1h lit ALERTS/MONITOR/SYSTEM sur 6h, corrèle, compare vault-mem, juge pertinence, publie digests sur `agent.bigbrother.summary` (P0-P2) ou silence (RAS).

Watchdog `bigbrother-watchdog.sh` timer 30min : si BB silencieux >2h ET ALERTS non vide → DM direct fallback + audit NATS P3.

Résultat : Stéphane reçoit **uniquement** les digests curés, pas le bruit brut Wazuh/Monitoring.

## Sécurité

- Tokens : jamais en clair dans logs (audit SQLite = hash sha256[0:12])
- Atomic writes avec backup `.bak` sur modifications sensibles
- Validation stricte admin (regex `^[a-z0-9_-]+$`, cross-refs)
- Email allowlist : `motreff@gmail.com` exact (anti-substring + angle-bracket extract)
- Gmail MFA activé 2 côtés
- systemd LoadCredential (fallback `/etc/hubmq/credentials/<name>` 0640 root:hubmq)
- Hardening : `ProtectSystem=strict`, `ProtectHome=yes`, `PrivateTmp=yes`, `NoNewPrivileges=yes`
- Wazuh FIM sur workspace jumeau + credentials + systemd units

## Roadmap V2 (backlog)

- `admin.agent.add` — ajout dynamique d'agents depuis email
- Kind `custom-http` (zeroclaw, gemini, endpoints non-OpenAI)
- Kind `claude-code-spawn` paramétrable (spawn N jumeaux avec charter différent)
- `admin.bot.edit` — modifier allowlist/config d'un bot existant
- UI dashboard HubMQ (status bots/agents/streams)
- Phase Exposure : ntfy public + Telegram webhook Internet
- Migration `imap v3` (quand stable) + parser `mailparse`
- NATS TLS pour `admin.bot.add` (documenter contrainte localhost)
- Test CI qui charge `config.toml.example` via loader réel
- Prometheus metrics export + Grafana dashboard
