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
