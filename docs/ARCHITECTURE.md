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
