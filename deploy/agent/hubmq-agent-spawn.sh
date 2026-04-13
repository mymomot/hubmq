#!/usr/bin/env bash
# hubmq-agent-spawn.sh — Spawn du jumeau claude-hubmq pour répondre à un message Telegram
# Args : $1 = message, $2 = chat_id

set -euo pipefail

MESSAGE="${1:-}"
CHAT_ID="${2:-1451527482}"

if [[ -z "$MESSAGE" ]]; then
    echo "Error: empty message" >&2
    exit 1
fi

WORKSPACE="$HOME/.hubmq-agent/workspace"
LOG="$HOME/.hubmq-agent/spawn.log"
CLAUDE_BIN="$(command -v claude)"
SEND_TG="$HOME/.hubmq-agent/wrapper/send-telegram.sh"

# P2.1 — Lock anti-double-réponse : si Claude principal est en session active
# (lock touché par hooks SessionStart/UserPromptSubmit), skip le spawn pour éviter
# que les deux instances répondent au même message.
LOCK=/tmp/hubmq-claude.owner
LOCK_TTL=3600  # 1h
if [[ -f "$LOCK" ]]; then
    AGE=$(( $(date +%s) - $(stat -c %Y "$LOCK") ))
    if (( AGE < LOCK_TTL )); then
        echo "[$(date -Is)] SKIP chat_id=$CHAT_ID — Claude principal détient le lock (age=${AGE}s < ${LOCK_TTL}s)" >> "$LOG"
        exit 0
    fi
    # Lock stale — le process principal a probablement crashé sans cleanup
    echo "[$(date -Is)] STALE_LOCK age=${AGE}s → nettoyage" >> "$LOG"
    rm -f "$LOCK"
fi

# P2.2 — Rotation session JSONL >24h : archive les sessions dormantes pour
# éviter compactions successives qui perdent du contexte lointain.
ENCODED_CWD=$(echo "$WORKSPACE" | sed 's|/|-|g' | sed 's|^-||')
JSONL_DIR="$HOME/.claude/projects/$ENCODED_CWD"
if [[ -d "$JSONL_DIR" ]]; then
    LAST_JSONL=$(ls -t "$JSONL_DIR"/*.jsonl 2>/dev/null | head -1 || echo "")
    if [[ -n "$LAST_JSONL" ]]; then
        JSONL_AGE=$(( $(date +%s) - $(stat -c %Y "$LAST_JSONL") ))
        if (( JSONL_AGE > 86400 )); then
            ARCHIVE="$HOME/tmp/hubmq-claude-archive"
            mkdir -p "$ARCHIVE"
            TS=$(date +%Y%m%d-%H%M%S)
            for f in "$JSONL_DIR"/*.jsonl; do
                [[ -f "$f" ]] || continue
                mv "$f" "$ARCHIVE/${TS}_$(basename "$f")"
            done
            echo "[$(date -Is)] ROTATE session dormante >24h archivée dans $ARCHIVE/" >> "$LOG"
        fi
    fi
fi

echo "[$(date -Is)] SPAWN chat_id=$CHAT_ID msg=${MESSAGE:0:80}" >> "$LOG"

cd "$WORKSPACE"

# Injection contexte thread vault-mem (si dispo)
THREAD=""
if command -v vault-mem >/dev/null 2>&1; then
    THREAD=$(vault-mem search \
        --caller claude-hubmq \
        --section conversations \
        --tag thread \
        --limit 15 2>/dev/null || echo "(no thread)")
fi

# Export pour le jumeau
export CHAT_ID
export HUBMQ_AGENT_ROLE="claude-hubmq"

# Prompt enrichi — le jumeau lit son CLAUDE.md local + suivra l'injection
PROMPT=$(cat <<PROMPT_EOF
=== THREAD vault-mem (15 dernières notes conversations) ===
$THREAD

=== CONTEXT RUNTIME ===
- CHAT_ID=$CHAT_ID (exporté en env var)
- Date : $(date -Is)

=== MESSAGE DE STÉPHANE (via @hubmqbot) ===
$MESSAGE

=== TA TÂCHE ===
1. Lis ce message et comprends la demande
2. Utilise tes MCP (vault-mem, nexus, bash lecture, etc.) si nécessaire
3. Formule une réponse claire, courte, en français
4. Persiste un résumé Q/R dans vault-mem section=conversations tag=thread author=claude-hubmq
5. Envoie la réponse finale à Stéphane via :
   bash ~/.hubmq-agent/wrapper/send-telegram.sh "$CHAT_ID" "<ton message final>"

Respecte strictement le charter ~/.hubmq-agent/workspace/CLAUDE.md (périmètre + refus gouvernance centrale).
PROMPT_EOF
)

# Spawn Claude Code CLI headless
if ! "$CLAUDE_BIN" \
    --print \
    --continue \
    --setting-sources "user" \
    --max-turns 20 \
    --permission-mode acceptEdits \
    "$PROMPT" >> "$LOG" 2>&1; then
    echo "[$(date -Is)] ERROR claude exit $?" >> "$LOG"
    # Fallback direct — message d'erreur à Stéphane
    "$SEND_TG" "$CHAT_ID" "⚠️ claude-hubmq rencontre un souci. Consulte ~/.hubmq-agent/spawn.log." || true
    exit 1
fi

echo "[$(date -Is)] DONE chat_id=$CHAT_ID" >> "$LOG"
