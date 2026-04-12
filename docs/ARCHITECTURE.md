# HubMQ Architecture

Technical architecture, data flows, components, and design decisions for HubMQ Phase Core.

## Overview

HubMQ is a unified communication hub that acts as a message broker and delivery system for the mymomot.ovh homelab. It:
- **Ingests** alerts from multiple sources (webhooks or NATS publish)
- **Filters** messages (deduplication, rate limiting, severity-aware quiet hours)
- **Routes** by severity (P0-P3 levels)
- **Delivers** via email, ntfy push, Telegram, Apprise
- **Bridges** Telegram responses back to agents via msg-relay
- **Audits** all events in SQLite + Wazuh FIM

## System Architecture

```
┌─ Sources ─────────────────────────────────────────────────────────────────┐
│                                                                             │
│  ┌─────────────────┐  ┌──────────────────┐  ┌──────────────────┐         │
│  │  Wazuh          │  │  Forgejo CI/CD   │  │  BigBrother      │         │
│  │  (security)     │  │  (pipeline)      │  │  (health-check)  │         │
│  └────────┬────────┘  └────────┬─────────┘  └────────┬─────────┘         │
│           │                    │                      │                   │
│  ┌────────▼────────┐  ┌────────▼─────────┐  ┌────────▼─────────┐         │
│  │ HTTP POST       │  │ HTTP POST        │  │ NATS publish    │         │
│  │ /in/wazuh       │  │ /in/forgejo      │  │ system.>        │         │
│  └────────┬────────┘  └────────┬─────────┘  └────────┬─────────┘         │
│           │                    │                      │                   │
│  ┌────────▼────────┬───────────┴─────────┬───────────▼─────────┐         │
│  │   systemd       │   cron tasks        │  User cron jobs    │         │
│  │   service       │   (scheduled tasks) │                     │         │
│  │   failures      │                     │                     │         │
│  └────────┬────────┴───────────┬─────────┴───────────┬─────────┘         │
│           │                    │                      │                   │
└───────────┼────────────────────┼──────────────────────┼───────────────────┘
            │                    │                      │
            │         ╔══════════╩══════════════════════╩═══════════════╗
            │         ║                                                 ║
            │         ║  NATS JetStream (local bus, port 4222)        ║
            │         ║                                                 ║
            │         ║  Streams: ALERTS | MONITOR | AGENTS |         ║
            │         ║           SYSTEM | CRON | USER_IN              ║
            │         ║                                                 ║
            │         ╚══════════╤══════════════════════════════════════╝
            │                    │
            ▼                    ▼
      ┌──────────────────────────────────────────────────────────┐
      │  HubMQ Daemon (Rust/Axum, port 8470)                     │
      │  ┌────────────────────────────────────────────────────┐  │
      │  │ HTTP Server (ingestion)                            │  │
      │  │ - POST /in/{wazuh,forgejo,generic}                │  │
      │  │ - GET /health                                      │  │
      │  │ - GET /metrics (future)                            │  │
      │  └────────────────────────────────────────────────────┘  │
      │  ┌────────────────────────────────────────────────────┐  │
      │  │ Telegram Bot (polling, teloxide)                   │  │
      │  │ - Poll API for new messages                        │  │
      │  │ - Send bot replies                                 │  │
      │  └────────────────────────────────────────────────────┘  │
      │  ┌────────────────────────────────────────────────────┐  │
      │  │ NATS Dispatcher Consumer (async-nats)              │  │
      │  │ - Subscribe to NATS streams                        │  │
      │  │ - Process messages through filters                 │  │
      │  │ - Route to sinks                                   │  │
      │  └────────────────────────────────────────────────────┘  │
      │  ┌────────────────────────────────────────────────────┐  │
      │  │ Filter Pipeline                                    │  │
      │  │ 1. Dedup (60s TTL cache)                           │  │
      │  │ 2. Rate limit (adaptive token bucket)              │  │
      │  │ 3. Severity router (quiet hours aware)             │  │
      │  └────────────────────────────────────────────────────┘  │
      │  ┌────────────────────────────────────────────────────┐  │
      │  │ Queue & Audit Log (SQLite WAL)                     │  │
      │  │ - Persistent outbox (delivery status tracking)     │  │
      │  │ - Audit log (all events + timestamps)              │  │
      │  └────────────────────────────────────────────────────┘  │
      └──────────────────────────────────────────────────────────┘
            │                    │                      │
      ┌─────▼──────┐       ┌────▼──────┐       ┌────────▼──────┐
      │ Email Sink │       │ ntfy Sink │       │ Apprise Sink  │
      │ (SMTP)     │       │ (HTTP)    │       │ (subprocess)  │
      │ lettre     │       │ reqwest   │       │ JSON stdin    │
      │ port 587   │       │ Bearer    │       │ (multi-ch)    │
      └─────┬──────┘       └────┬──────┘       └────────┬──────┘
            │                    │                      │
            ▼                    ▼                      ▼
      ┌─────────────┐       ┌──────────┐       ┌────────────────┐
      │ Gmail SMTP  │       │ ntfy.sh  │       │ Telegram API   │
      │ port 587    │       │ LAN only │       │ (polling)      │
      │ TLS 1.3     │       │ (Phase C)│       │ (bidirectional)│
      └─────────────┘       └──────────┘       └────────┬───────┘
                                                         │
                                                    User chat
                                                         │
                                        ┌────────────────▼──────────────┐
                                        │ msg-relay (LXC 500, :9480)   │
                                        │ Bridge command whitelist      │
                                        │ - /status, /logs, /help      │
                                        └────────────────┬──────────────┘
                                                         │
                                                    Claude Code
                                                     (agents)
```

## Data Flow — Downstream (Source → Delivery)

### Phase 1: Ingestion

**HTTP Webhook** (Wazuh, Forgejo, generic):
```
POST /in/wazuh
Content-Type: application/json

{
  "rule.level": 12,
  "full_log": "...",
  "timestamp": "2026-04-12T10:30:00Z"
}
```

Parsed → `Message` struct → NATS publish on `alert.wazuh.critical`

**NATS Direct** (BigBrother, custom scripts):
```
publish("system.service.failed", JSON)
```

Received → Stream persisted (JetStream)

### Phase 2: Queuing & Dedup

```
Message
    ↓
SHA256(source + title + body) = dedup_key
    ↓
Cache lookup (60s TTL)
    ↓
[HIT] → SKIP
[MISS] → INSERT cache, CONTINUE
    ↓
SQLite INSERT outbox (status='pending')
```

### Phase 3: Filtering

**Rate Limiting**:
- Normal sources: 10 msgs/min (token bucket)
- P0 (critical): 100 msgs/min (separate bucket)
- Exceeded → SKIP with audit log entry

**Severity Routing** (quiet hours aware):
```
P0 (critical)     → ALWAYS deliver (bypasses quiet hours)
P1 (high)         → ALWAYS deliver (bypasses quiet hours)
P2 (medium)       → Deliver, OR skip if quiet_hours active
P3 (low)          → Deliver, OR skip if quiet_hours active

quiet_hours: 22:00 → 07:00 (9-hour window)
```

### Phase 4: Delivery (Multi-sink)

For each sink:
1. **Email** (Gmail SMTP):
   - Template: `email.html.tera`
   - Subject: `[P0/P1/P2/P3] {title}`
   - CC/BCC: configured in config.toml
   - Delivery: synchronous (blocking)

2. **ntfy** (LAN push):
   - URL: `http://192.168.10.15:2586/hubmq-alerts`
   - Priority: P0→max, P1→high, P2→default, P3→low
   - Auth: Bearer token (configured)
   - Delivery: HTTP POST async (queued)

3. **Apprise** (multi-channel):
   - Subprocess: `/usr/local/bin/apprise` with JSON stdin
   - Channels: email, telegram, slack, etc.
   - Anti-injection: JSON stdin (never shell=True)
   - Delivery: subprocess async

### Phase 5: Persistence

SQLite WAL (Write-Ahead Logging):
```sql
CREATE TABLE outbox (
  id INTEGER PRIMARY KEY,
  message_id TEXT UNIQUE,
  subject TEXT,
  status TEXT,       -- pending | sent | failed | skipped
  timestamp TEXT,
  retry_count INT
);

CREATE TABLE audit_log (
  id INTEGER PRIMARY KEY,
  timestamp TEXT,
  event_type TEXT,   -- ingested | filtered | delivered | failed
  source TEXT,
  severity TEXT,
  message TEXT
);
```

## Data Flow — Upstream (User → Agent)

### Phase 1: Telegram Input

User sends message to HubMQ Telegram bot → polls API → Message received

### Phase 2: Validation

```
Chat ID check
    ↓
[NOT in allowlist] → SKIP (B2 forward rejection)
[OK] → CONTINUE
    ↓
Command whitelist (B3)
    ↓
["status", "logs", "help", ...] → ALLOWED
[other] → REJECT
```

### Phase 3: Bridge

```
Valid user command
    ↓
NATS publish on user.incoming.telegram
    ↓
Bridge consumer subscribes
    ↓
Check command whitelist (B3)
    ↓
POST to msg-relay (LXC 500 :9480)
    ↓
Body: {"chat_id": N, "text": "/status", "source": "hubmq"}
    ↓
msg-relay forwards to Claude Code / agents
```

## File Layout on LXC 415

### Source Code

```
~/projects/hubmq/
├── crates/hubmq-core/src/
│   ├── lib.rs                      # module re-exports
│   ├── config.rs                   # TOML loader, Config struct
│   ├── message.rs                  # Message, Severity, dedup_hash
│   ├── subjects.rs                 # NATS subject builders (safe identifiers)
│   ├── nats_conn.rs               # NATS client, stream init
│   ├── queue.rs                    # SQLite connection, INSERT/query
│   ├── audit.rs                    # Audit log schema + insert
│   ├── dispatcher.rs              # Consumer loop + filter pipeline
│   ├── filter/
│   │   ├── mod.rs                  # Filter trait, dedup/ratelimit/severity chain
│   │   ├── dedup.rs               # DedupFilter (60s cache)
│   │   ├── ratelimit.rs           # RateLimitFilter (token bucket, P0 bucket)
│   │   └── severity.rs            # SeverityRouter (quiet hours)
│   ├── sink/
│   │   ├── mod.rs                  # Sink trait, SinkRegistry
│   │   ├── email.rs               # EmailSink (lettre SMTP + tera)
│   │   ├── ntfy.rs                # NtfySink (reqwest HTTP)
│   │   └── apprise.rs             # ApprisieSink (subprocess JSON)
│   ├── source/
│   │   ├── mod.rs                  # Source trait + HTTP routes
│   │   ├── webhook.rs             # HTTP ingestion (Wazuh, Forgejo, generic)
│   │   ├── telegram.rs            # Telegram polling bot (teloxide)
│   │   └── heartbeat.rs           # Inverse heartbeat consumer
│   └── bridge.rs                   # msg-relay bridge + command whitelist
└── crates/hubmq-bin/src/main.rs   # axum HTTP server + tokio main loop
```

### Runtime Data

```
/etc/hubmq/
├── config.toml                     # Runtime config (nats, smtp, telegram, filter, bridge)
├── credentials/                    # Secret files (chmod 600)
│   ├── gmail-app-password
│   └── telegram-bot-token

/var/lib/hubmq/
├── queue.db                        # SQLite WAL (outbox + audit log)
├── queue.db-wal                    # Write-ahead log
└── queue.db-shm                    # Shared memory index

/var/log/hubmq/
└── (systemd journal via journalctl)

/var/lib/nats/jetstream/
└── (NATS JetStream store, persistent streams)
```

### Binary & Systemd

```
/usr/local/bin/
├── hubmq                           # Main daemon
├── hubmq-fallback-p0.sh           # P0 emergency email script
├── nats-server                     # NATS JetStream
└── nats                            # NATS CLI

/etc/systemd/system/
├── hubmq.service                   # Main service (Type=notify, LoadCredential)
├── hubmq-fallback-p0.service      # OnFailure handler
├── hubmq-heartbeat.service        # Heartbeat pulse script
├── hubmq-heartbeat.timer          # 1h recurring timer
└── nats.service                    # NATS service
```

## NATS Subject Hierarchy

See `docs/SUBJECTS.md` for complete contract.

**Format**: `<domain>.<producer>.<event_type>[.<severity>]`

**Example subjects**:
- `alert.wazuh.critical` — Wazuh alert severity 12+
- `alert.wazuh.high` — Wazuh alert severity 8-11
- `alert.forgejo.failed` — CI pipeline failed
- `monitor.heartbeat.hubmq` — HubMQ pulse (1h interval)
- `system.service.failed` — systemd service failure
- `cron.backup.completed` — Cron job completion
- `user.incoming.telegram` — User message from Telegram

## Security Model

| Layer | Mechanism | Details |
|---|---|---|
| **NATS Auth** | NKey pairs | hubmq-service (full access), publisher (restricted domains) |
| **Secrets Storage** | systemd LoadCredential | `/etc/hubmq/credentials/` (chmod 600, root:root) |
| **Process Isolation** | systemd service (User=hubmq) | Daemon runs as unprivileged hubmq user |
| **Firewall** | UFW rules | Port 4222 (NATS), 8470 (health), 8222 (monitoring) — LAN only |
| **Anti-injection** | JSON stdin | Apprise invoked with JSON, never shell=True |
| **Audit Trail** | SQLite log + Wazuh FIM | All events timestamped, FIM monitors config/binary/db |
| **Rate Limiting** | Token bucket | Prevents alert spam, separate P0 bucket |
| **Deduplication** | SHA256 hash + TTL cache | Prevents duplicate deliveries (60s window) |

## Condition Council Integration

| Condition | Implementation | Status |
|---|---|---|
| **B1** — Local P0 fallback | `OnFailure=hubmq-fallback-p0.service`, heartbeat 1h timer | ✅ |
| **B2** — Telegram forward rejection | `Bridge::check_forward()` rejects non-allowlist | ✅ |
| **B3** — msg-relay command whitelist | `["status", "logs", "help"]` hardcoded in config | ✅ |
| **B6** — ntfy auth deny-all | Bearer token required, empty token = disabled | ✅ |
| **D1** — NATS subjects documented | `docs/SUBJECTS.md` canonical contract | ✅ |
| **D2** — JetStream limits explicit | max_age, max_bytes per stream (nats-server.conf) | ✅ |
| **D3** — systemd LoadCredential | All secrets via credentials/ + unit file | ✅ |
| **D4** — Audit log structured | SQLite JSON + Wazuh FIM group `hubmq` | ✅ |
| **D5** — P0/P1 bypass quiet hours | `Message::bypasses_quiet_hours()` | ✅ |
| **D6** — Adaptive rate limit | Separate buckets: 10/min normal, 100/min P0 | ✅ |
| **D7** — Boot order | `After=nats.service`, position 6 on LXC 415 | ✅ |
| **D8** — NATS subjects fixed IDs | Never user-provided text, hardcoded in `subjects.rs` | ✅ |

## Concurrency Model

- **Async Rust** : tokio runtime (full features)
- **Dispatcher loop** : async consumer subscription (NATS)
- **HTTP server** : axum spawns per-request tasks
- **Telegram polling** : separate async task (polling interval 5s)
- **Sink delivery** : async per-message (concurrent channels)
- **SQLite access** : sqlx connection pool (single writer via WAL)

## Observability

### Logs (systemd JSON)

```
{
  "SYSLOG_IDENTIFIER": "hubmq",
  "PRIORITY": "6",
  "MESSAGE": "Message delivered to email sink",
  "MESSAGE_ID": "uuid",
  "FIELDS": {
    "source": "wazuh",
    "severity": "P0",
    "subject": "alert.wazuh.critical"
  }
}
```

### Metrics (future)

- Messages ingested (per source)
- Messages delivered (per sink)
- Dedup cache hits
- Rate limit rejections
- Quiet hours active (boolean)

### Audit Trail (SQLite)

```sql
SELECT timestamp, event_type, source, severity, message
FROM audit_log
WHERE timestamp > datetime('now', '-24 hours')
ORDER BY timestamp DESC;
```

## Failure Modes & Recovery

| Failure | Impact | Recovery |
|---|---|---|
| **NATS down** | Messages queued locally, lost if HubMQ crashes | Restart NATS, replay queue |
| **SMTP unavailable** | Email delivery blocked | Retry queue + fallback (future) |
| **Telegram API down** | Bot messages fail | Retry queue (with backoff) |
| **SQLite corrupted** | Delivery history lost | WAL recovery or restore backup |
| **HubMQ daemon crash** | P0 emergency email triggered (B1) | systemd OnFailure handler |
| **Disk full** | SQLite writes fail | Free disk, restart |

## Performance Notes

- **Dedup cache** : 60s TTL, in-memory (DashMap)
- **Rate limiter** : token bucket (minimal CPU)
- **Email sink** : sync (blocking per message)
- **ntfy sink** : async (queued, non-blocking)
- **Apprise sink** : async subprocess (concurrent)
- **NATS stream** : distributed WAL on disk (JetStream)
- **SQLite** : WAL mode, connection pool

Typical latency: **2-5s** per message (ingestion → NATS → filter → delivery)
