#!/usr/bin/env bash
# hubmq-agent-listener.sh — Daemon qui écoute NATS user.incoming.telegram et spawn le jumeau
set -euo pipefail
LOG="$HOME/.hubmq-agent/listener.log"
NATS_SEED="$HOME/.hubmq-agent/credentials/nats-hubmq-service.seed"
NATS_SERVER="nats://192.168.10.15:4222"

echo "[$(date -Is)] listener starting" >> "$LOG"

/usr/local/bin/nats subscribe "user.incoming.telegram" \
    --nkey "$NATS_SEED" \
    --server "$NATS_SERVER" \
    --raw \
    2>> "$LOG" | \
while IFS= read -r line; do
    [ -z "$line" ] && continue
    MSG=$(echo "$line" | jq -r '.body // empty' 2>/dev/null || true)
    CHAT=$(echo "$line" | jq -r '.meta.chat_id // "1451527482"' 2>/dev/null || echo "1451527482")
    if [ -n "$MSG" ]; then
        echo "[$(date -Is)] trigger chat=$CHAT msg=${MSG:0:60}" >> "$LOG"
        "$HOME/.hubmq-agent/wrapper/hubmq-agent-spawn.sh" "$MSG" "$CHAT" 2>> "$LOG" &
    fi
done
