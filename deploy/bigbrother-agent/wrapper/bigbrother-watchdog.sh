#!/usr/bin/env bash
# bigbrother-watchdog.sh — Surveille BigBrother + garantit qu'une alerte P0 brute n'est jamais
# silencieuse. Timer systemd 30min. Si BigBrother silencieux >2h ET ALERTS non vide récemment,
# envoie une DM directe à Stéphane + publish agent.watchdog.alert pour audit.

set -euo pipefail

NATS_SEED="$HOME/.bigbrother-agent/credentials/nats-hubmq-service.seed"
NATS_SERVER="nats://192.168.10.15:4222"
LOG="$HOME/.bigbrother-agent/logs/watchdog.log"
CHAT_ID="1451527482"
SEND_TG="$HOME/.hubmq-agent/wrapper/send-telegram.sh"

# Fenêtres (secondes)
HEARTBEAT_MAX_AGE=7200       # 2h — au-delà BigBrother est considéré silencieux
ALERTS_LOOKBACK=21600        # 6h — fenêtre d'observation ALERTS
DEDUP_WINDOW=3600            # 1h — ne pas alerter plus d'1x/h pour le même état

mkdir -p "$(dirname "$LOG")"
STATE_FILE="$HOME/.bigbrother-agent/logs/.watchdog-last-alert"

log() { echo "[$(date -Is)] $*" >> "$LOG"; }

# --- 1. Quand était le dernier heartbeat BigBrother ? ---
HEARTBEAT_TS=$(nats --nkey "$NATS_SEED" --server "$NATS_SERVER" \
    stream subjects AGENTS --filter 'agent.bigbrother.heartbeat' 2>/dev/null | tail -1 || echo "")

# Fallback : derniere date du log analyze.log (plus fiable que NATS si stream vide au boot)
LAST_RUN_TS=$(grep -E '^\[.*\] DONE' "$HOME/.bigbrother-agent/logs/analyze.log" 2>/dev/null | tail -1 | sed 's/^\[\([^]]*\)\].*/\1/' || echo "")

if [[ -n "$LAST_RUN_TS" ]]; then
    LAST_RUN_EPOCH=$(date -d "$LAST_RUN_TS" +%s 2>/dev/null || echo 0)
    NOW=$(date +%s)
    AGE=$(( NOW - LAST_RUN_EPOCH ))
else
    AGE=999999  # Jamais exécuté
fi

log "check heartbeat_age=${AGE}s (threshold=${HEARTBEAT_MAX_AGE}s)"

if (( AGE < HEARTBEAT_MAX_AGE )); then
    log "OK — BigBrother vivant"
    exit 0
fi

# --- 2. BigBrother silencieux. Y a-t-il des alertes brutes dans ALERTS récemment ? ---
# On compte simplement les messages dans ALERTS (stream retention 7j, tout message y est récent par rapport à 6h)
ALERTS_COUNT=$(nats --nkey "$NATS_SEED" --server "$NATS_SERVER" \
    stream info ALERTS --json 2>/dev/null | \
    python3 -c "import sys,json; d=json.load(sys.stdin); print(d['state']['messages'])" 2>/dev/null || echo 0)

log "BigBrother silencieux (age=${AGE}s), ALERTS messages=${ALERTS_COUNT}"

if (( ALERTS_COUNT < 1 )); then
    log "RAS — ALERTS vide, pas d'urgence"
    exit 0
fi

# --- 3. Dédup : déjà alerté dans la dernière heure ? ---
if [[ -f "$STATE_FILE" ]]; then
    LAST_ALERT_EPOCH=$(cat "$STATE_FILE")
    DEDUP_AGE=$(( $(date +%s) - LAST_ALERT_EPOCH ))
    if (( DEDUP_AGE < DEDUP_WINDOW )); then
        log "SKIP — déjà alerté il y a ${DEDUP_AGE}s (< ${DEDUP_WINDOW}s)"
        exit 0
    fi
fi

# --- 4. Alerte directe ---
BODY="⚠️ *BigBrother watchdog*

BigBrother silencieux depuis ${AGE}s (>${HEARTBEAT_MAX_AGE}s seuil).
Stream ALERTS contient ${ALERTS_COUNT} messages non curés.

Checks suggérés :
\`systemctl status bigbrother-analyze.timer\`
\`journalctl -u bigbrother-analyze.service --since '4h ago'\`
\`tail ~/.bigbrother-agent/logs/analyze.log\`"

if [[ -x "$SEND_TG" ]]; then
    "$SEND_TG" "$CHAT_ID" "$BODY" >> "$LOG" 2>&1 && log "DM watchdog envoyée" || log "ERROR DM failed"
else
    log "ERROR send-telegram.sh absent — alerte perdue"
fi

# Publish audit pour trace NATS
UUID=$(cat /proc/sys/kernel/random/uuid)
TS=$(date -u +%Y-%m-%dT%H:%M:%SZ)
# severity=P3 → dispatcher log_only (DM primaire a déjà été envoyée en direct ci-dessus,
# ce publish est uniquement pour audit/trace NATS).
AUDIT=$(printf '{"id":"%s","ts":"%s","source":"bigbrother-watchdog","severity":"P3","title":"BigBrother silencieux","body":"Watchdog fallback — age=%ss alerts_pending=%s","tags":["watchdog","fallback","audit"]}' \
    "$UUID" "$TS" "$AGE" "$ALERTS_COUNT")
nats --nkey "$NATS_SEED" --server "$NATS_SERVER" pub agent.watchdog.alert "$AUDIT" >> "$LOG" 2>&1 || true

# Mémorise l'alerte pour dédup
date +%s > "$STATE_FILE"
log "DONE"
