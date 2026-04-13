#!/usr/bin/env bash
# bigbrother-analyze.sh — Invocation Claude CLI pour analyse monitoring transversale.
# Déclenché par systemd timer (bigbrother-analyze.timer, 1h).

set -euo pipefail

WORKSPACE="$HOME/.bigbrother-agent/workspace"
LOG="$HOME/.bigbrother-agent/logs/analyze.log"
CLAUDE_BIN="$(command -v claude)"

mkdir -p "$(dirname "$LOG")"
echo "[$(date -Is)] START analyze" >> "$LOG"

cd "$WORKSPACE"

PROMPT=$(cat <<'PROMPT_EOF'
=== CONTEXTE RUN ===
Date : $(date -Is)
Mission : analyse transversale homelab via NATS streams + vault-mem (voir charter CLAUDE.md).

=== ÉTAPES À EXÉCUTER ===
1. Lis ton charter ~/.bigbrother-agent/workspace/CLAUDE.md (si pas déjà injecté).
2. Collecte les 3 streams ALERTS/MONITOR/SYSTEM sur 6h (nats CLI, seed dans credentials/).
3. Corrèle (timestamps ±5min, host récurrent, patterns).
4. vault_search section=monitoring tag=bigbrother-analysis limit=10 pour comparer avec analyses récentes.
5. Juge pertinence P0/P1/P2/RAS.
6. Si pertinent (P0-P2 non déjà résolu dans les 2h) : publish sur NATS agent.bigbrother.summary.
7. vault_write analyse détaillée dans section=monitoring tag=bigbrother-analysis.
8. Log final dans ~/.bigbrother-agent/logs/analyze.log (timestamp, severity, publish oui/non, events count).

Silence = meilleur que bruit. Si rien ne mérite d'être remonté → juste note vault courte "RAS".
PROMPT_EOF
)

if ! "$CLAUDE_BIN" \
    --print \
    --continue \
    --setting-sources "user" \
    --max-turns 30 \
    --permission-mode acceptEdits \
    "$PROMPT" >> "$LOG" 2>&1; then
    echo "[$(date -Is)] ERROR claude exit $?" >> "$LOG"
    exit 1
fi

echo "[$(date -Is)] DONE" >> "$LOG"

# P4.1 — Heartbeat : signale que BigBrother vient de s'exécuter (consommé par watchdog).
# Severity P3 → dispatcher log_only (pas de DM à Stéphane).
NATS_SEED="$HOME/.bigbrother-agent/credentials/nats-hubmq-service.seed"
NATS_SERVER="nats://192.168.10.15:4222"
if command -v nats >/dev/null 2>&1; then
    UUID=$(cat /proc/sys/kernel/random/uuid)
    TS=$(date -u +%Y-%m-%dT%H:%M:%SZ)
    HEARTBEAT=$(printf '{"id":"%s","ts":"%s","source":"bigbrother","severity":"P3","title":"BigBrother heartbeat","body":"Run OK","tags":["heartbeat"]}' "$UUID" "$TS")
    nats --nkey "$NATS_SEED" --server "$NATS_SERVER" pub agent.bigbrother.heartbeat "$HEARTBEAT" >> "$LOG" 2>&1 || \
        echo "[$(date -Is)] WARN heartbeat publish failed" >> "$LOG"
fi
