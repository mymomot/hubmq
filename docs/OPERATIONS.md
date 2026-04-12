# HubMQ Operations

Complete guide to operate, monitor, troubleshoot, and maintain HubMQ in production on LXC 415.

## Accessing HubMQ

### SSH access

```bash
ssh hubmq  # Defined in ~/.ssh/config
# or: ssh -i ~/.ssh/id_rsa motreffs@192.168.10.15
```

### NATS Monitoring Endpoint

Open browser to: `http://192.168.10.15:8222/`

Useful endpoints:
- `/varz` — server variables (uptime, connections, etc.)
- `/jsz` — JetStream info (streams, consumers, storage usage)
- `/connz` — active connections
- `/subsz` — active subscriptions
- `/accountz` — account-level stats
- `/healthz` — HTTP 200 if healthy

Example:
```bash
curl -s http://192.168.10.15:8222/jsz | jq '.streams | map({name, state: .state})'
```

### HubMQ Health Endpoint

```bash
curl -sf http://192.168.10.15:8470/health
# Returns HTTP 200 + JSON {"status": "ok"} if daemon running
```

## Daily Operations

### View logs (structured JSON)

```bash
# HubMQ daemon
ssh hubmq "sudo journalctl -u hubmq.service -n 50 -f"

# NATS server
ssh hubmq "sudo journalctl -u nats.service -n 50 -f"

# Filter by level
ssh hubmq "sudo journalctl -u hubmq.service --grep=error -n 20"

# Specific time range
ssh hubmq "sudo journalctl -u hubmq.service --since 2026-04-12T10:00:00"
```

### Check queue status

```bash
# Connected via SSH tunnel
ssh hubmq <<'EOF'
sqlite3 /var/lib/hubmq/queue.db <<SQL
SELECT status, COUNT(*) as count FROM outbox GROUP BY status;
SELECT subject, COUNT(*) as count FROM audit_log WHERE timestamp > datetime('now', '-1 hour') GROUP BY subject;
SQL
EOF
```

### Verify NATS streams

```bash
ssh hubmq <<'EOF'
export NATS_NKEY_SEED_PATH=/etc/nats/nkeys/hubmq-service.seed
nats stream list
nats stream info ALERTS
nats stream info USER_IN
EOF
```

### Test message flow (manual publish)

```bash
ssh hubmq <<'EOF'
export NATS_NKEY_SEED_PATH=/etc/nats/nkeys/publisher.seed

# Publish test alert
nats pub alert.wazuh.critical '{"title":"Test alert","body":"This is a test"}'

# Check it arrived in stream
nats stream view ALERTS -n 1
EOF
```

## Credential Management

### Gmail App Password Rotation

1. Generate new password at https://myaccount.google.com/apppasswords
2. Update on LXC 415:
   ```bash
   printf "NEW_16_CHAR_PASSWORD" | \
     ssh hubmq "sudo tee /etc/hubmq/credentials/gmail-app-password" > /dev/null
   ssh hubmq "sudo chmod 600 /etc/hubmq/credentials/gmail-app-password"
   ```
3. Restart service:
   ```bash
   ssh hubmq "sudo systemctl restart hubmq.service"
   ```
4. Verify logs:
   ```bash
   ssh hubmq "sudo journalctl -u hubmq.service --grep SMTP -n 5"
   ```

### Telegram Bot Token Rotation

1. Revoke old token via Telegram BotFather:
   - Message @BotFather: `/revoke`
   - Select bot
   - Confirm revocation
2. Create new bot:
   - Message @BotFather: `/newbot`
   - Follow prompts, copy new token
3. Update on LXC 415:
   ```bash
   printf "NEW_BOT_TOKEN" | \
     ssh hubmq "sudo tee /etc/hubmq/credentials/telegram-bot-token" > /dev/null
   ssh hubmq "sudo chmod 600 /etc/hubmq/credentials/telegram-bot-token"
   ```
4. Restart:
   ```bash
   ssh hubmq "sudo systemctl restart hubmq.service"
   ```
5. Verify new chat_id allowlist in `/etc/hubmq/config.toml`:
   - Get your Telegram user ID: send `/start` to bot (check logs for chat_id)
   - Update config:
     ```bash
     ssh hubmq "sudo nano /etc/hubmq/config.toml"
     # Edit [telegram] allowed_chat_ids = [YOUR_CHAT_ID]
     sudo systemctl restart hubmq.service
     ```

### NKey Seed Rotation (Advanced)

See `deploy/README-nkeys.md` for full NKey management guide.

**Quick steps** (for hubmq-service NKey):
```bash
# Generate new NKey pair (offline or using nsc)
# Copy new seed to /etc/nats/nkeys/hubmq-service.seed (chmod 600)
# Update /etc/nats/nats-server.conf with new public key
# Verify syntax: sudo -u nats nats-server -c /etc/nats/nats-server.conf -t
# Restart NATS: sudo systemctl restart nats.service
# Restart HubMQ: sudo systemctl restart hubmq.service
```

## Adding New Alert Sources

### HTTP webhook integration (easiest)

1. Define new subject in `docs/SUBJECTS.md`:
   ```
   | `alert.my-service.event-type` | My Service webhook | medium | JSON payload |
   ```

2. Configure source system to POST to:
   ```
   http://192.168.10.15:8470/in/generic
   ```
   with JSON body:
   ```json
   {
     "source": "my-service",
     "severity": "high",
     "title": "Something happened",
     "body": "Details here",
     "tags": ["tag1", "tag2"]
   }
   ```

3. Verify in HubMQ logs:
   ```bash
   ssh hubmq "sudo journalctl -u hubmq.service | grep 'my-service'"
   ```

### NATS direct publish (requires publisher NKey)

1. Get publisher NKey seed from `/etc/nats/nkeys/publisher.seed`
2. From any LAN system with nats CLI:
   ```bash
   export NATS_NKEY_SEED_PATH=./publisher.seed
   nats pub alert.my-service.event '{"source":"...", "severity":"..."}'
   ```

3. Verify arrival:
   ```bash
   nats stream view ALERTS -n 5
   ```

## Troubleshooting

### HubMQ daemon won't start

**Symptom**: `systemctl status hubmq.service` shows failed/inactive

**Check**:
```bash
ssh hubmq "sudo journalctl -u hubmq.service -n 100"
```

**Common causes**:
- Config file syntax error:
  ```bash
  ssh hubmq "cat /etc/hubmq/config.toml | head -20"
  ```
- Missing credentials:
  ```bash
  ssh hubmq "ls -la /etc/hubmq/credentials/"
  ```
- NATS connection failed:
  ```bash
  ssh hubmq "curl -s http://localhost:4222"
  ```
- Fix and restart:
  ```bash
  ssh hubmq "sudo systemctl restart hubmq.service"
  ```

### No messages being delivered

**Symptom**: Alerts sent but no emails/Telegram received

**Check**:
```bash
# Check queue.db for stuck messages
ssh hubmq "sqlite3 /var/lib/hubmq/queue.db 'SELECT status, COUNT(*) FROM outbox GROUP BY status;'"

# Check logs for sink errors
ssh hubmq "sudo journalctl -u hubmq.service | grep -i 'email\|telegram\|ntfy'"

# Verify NATS is receiving messages
ssh hubmq <<'EOF'
export NATS_NKEY_SEED_PATH=/etc/nats/nkeys/hubmq-service.seed
nats stream info ALERTS
EOF
```

**Fix**:
1. Verify credentials:
   ```bash
   ssh hubmq "sudo test -r /etc/hubmq/credentials/* && echo OK || echo MISSING"
   ```
2. Check NATS connectivity:
   ```bash
   ssh hubmq "netstat -tuln | grep 4222"
   ```
3. Check filter configuration (rate limits, quiet hours):
   ```bash
   ssh hubmq "grep -A 10 '\[filter\]' /etc/hubmq/config.toml"
   ```

### Telegram bot not responding

**Symptom**: Message sent to bot, no response in chat

**Check**:
```bash
# Verify chat_id in allowlist
ssh hubmq "grep -A 3 '\[telegram\]' /etc/hubmq/config.toml"

# Check for incoming messages in USER_IN stream
ssh hubmq <<'EOF'
export NATS_NKEY_SEED_PATH=/etc/nats/nkeys/hubmq-service.seed
nats stream view USER_IN -n 5
EOF

# Look for bot polling errors
ssh hubmq "sudo journalctl -u hubmq.service | grep -i 'telegram\|polling'"
```

**Common causes**:
- Chat ID not in allowlist: update config, restart
- Bot token revoked: regenerate token, update credentials
- Bot doesn't have send permissions: retry message, check Telegram settings

### NATS stream full (discarding old messages)

**Symptom**: Messages appear in stream but disappear quickly

**Check**:
```bash
ssh hubmq <<'EOF'
export NATS_NKEY_SEED_PATH=/etc/nats/nkeys/hubmq-service.seed
nats stream info ALERTS | grep -i 'state\|bytes'
EOF
```

**View config**:
```bash
grep -A 20 'ALERTS' /etc/nats/nats-server.conf
```

**If stream is full**:
1. Check publisher rate (too many alerts?)
2. Adjust limits in `/etc/nats/nats-server.conf`:
   ```
   max_age: 72h      # increase retention
   max_bytes: 2GB    # increase storage
   max_msgs: 20000   # increase message count
   ```
3. Reload NATS:
   ```bash
   sudo systemctl reload nats.service
   ```

### P0 fallback email triggered (HubMQ crashed)

**Symptom**: Emergency email received from fallback system

**Action**:
1. Investigate crash:
   ```bash
   ssh hubmq "sudo journalctl -u hubmq.service -n 200 -p err"
   ```
2. Fix root cause (corrupted config, database, etc.)
3. Restart:
   ```bash
   ssh hubmq "sudo systemctl restart hubmq.service"
   ```
4. Verify recovery:
   ```bash
   curl -sf http://192.168.10.15:8470/health
   ```

## Monitoring & Alerting

### Heartbeat pulse (B1 — Local P0 fallback)

HubMQ publishes heartbeat every 1 hour. If no heartbeat for >3 hours, emergency email sent.

**Check heartbeat**:
```bash
ssh hubmq <<'EOF'
export NATS_NKEY_SEED_PATH=/etc/nats/nkeys/hubmq-service.seed
nats stream view MONITOR -n 5 | grep heartbeat
EOF
```

**View timer status**:
```bash
ssh hubmq "sudo systemctl list-timers | grep hubmq"
```

### Integration with BigBrother (future)

HubMQ expects heartbeat on `monitor.heartbeat.hubmq`:
```json
{"timestamp": "2026-04-12T10:30:00Z", "ok": true}
```

When received, HubMQ resets the P0 silence counter, preventing false-positive fallback emails.

## Performance Tuning

### Increase rate limits

Edit `/etc/hubmq/config.toml`:
```toml
[filter]
rate_limit_per_min = 20      # 10 → 20 normal messages/min
rate_limit_p0_per_min = 200  # 100 → 200 critical/min
```

Restart:
```bash
ssh hubmq "sudo systemctl restart hubmq.service"
```

### Expand NATS stream storage

Edit `/etc/nats/nats-server.conf`:
```
stream ALERTS
  subjects alert.*
  max_age 48h       # 24h → 48h
  max_bytes 2GB     # 1GB → 2GB
  max_msgs 20000    # 10000 → 20000
```

Reload:
```bash
ssh hubmq "sudo systemctl reload nats.service"
```

### Enable SQLite WAL mode (already enabled)

Verify:
```bash
ssh hubmq "sqlite3 /var/lib/hubmq/queue.db 'PRAGMA journal_mode;'"
# Should output: wal
```

## Backup & Disaster Recovery

### Backup persistent data

```bash
# SQLite queue (audit log + outbox)
scp hubmq:/var/lib/hubmq/queue.db ~/backups/hubmq-queue-$(date +%s).db

# NATS JetStream data (optional, recreate streams if lost)
ssh hubmq "sudo tar czf /tmp/jetstream-backup.tar.gz /var/lib/nats/jetstream/" && \
  scp hubmq:/tmp/jetstream-backup.tar.gz ~/backups/jetstream-$(date +%s).tar.gz
```

### Restore from backup

```bash
# Restore SQLite
scp ~/backups/hubmq-queue-*.db hubmq:/tmp/ && \
  ssh hubmq "sudo systemctl stop hubmq && \
    sudo rm /var/lib/hubmq/queue.db && \
    sudo cp /tmp/hubmq-queue-*.db /var/lib/hubmq/queue.db && \
    sudo chown hubmq:hubmq /var/lib/hubmq/queue.db && \
    sudo systemctl start hubmq"

# Verify
curl -sf http://192.168.10.15:8470/health
```

## First-time Deploy Checklist

- [ ] LXC 415 provisioned + SSH hardened + UFW enabled
- [ ] NATS JetStream v2.10.24 installed + systemd service running
- [ ] 6 streams created (ALERTS, MONITOR, AGENTS, SYSTEM, CRON, USER_IN)
- [ ] NKeys generated + stored in `/etc/nats/nkeys/` (chmod 600)
- [ ] Apprise installed in `/opt/apprise`
- [ ] HubMQ daemon binary built + deployed to `/usr/local/bin/hubmq`
- [ ] systemd units installed (hubmq.service, hubmq-fallback-p0.service, hubmq-heartbeat.timer)
- [ ] Config file `/etc/hubmq/config.toml` populated
- [ ] Credentials files created (gmail-app-password, telegram-bot-token) with chmod 600
- [ ] Service enabled and running: `systemctl enable hubmq.service && systemctl start hubmq.service`
- [ ] Health endpoint responds: `curl -sf http://192.168.10.15:8470/health`
- [ ] NATS monitoring accessible: `curl -s http://192.168.10.15:8222/jsz | jq`
- [ ] Wazuh agent active (agent 010)
- [ ] Test alert published and delivered successfully
- [ ] Telegram bot responding to `/start` command
