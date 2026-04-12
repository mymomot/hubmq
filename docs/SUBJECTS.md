# HubMQ — NATS Subjects Map

Contract between publishers (Wazuh, Forgejo, BigBrother, etc.) and HubMQ consumer.

## Naming convention

`<domain>.<producer>.<event_type>[.<severity>]`

- Lowercase, dot-separated
- Fixed identifiers, **never user-provided text** (council condition D8)
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
