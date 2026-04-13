#!/usr/bin/env bash
# send-telegram.sh — Envoie un DM Telegram via @hubmqbot
# Usage : send-telegram.sh <chat_id> <message_texte>

set -euo pipefail

CHAT_ID="${1:-}"
MESSAGE="${2:-}"

if [[ -z "$CHAT_ID" || -z "$MESSAGE" ]]; then
    echo "Usage: send-telegram.sh <chat_id> <message>" >&2
    exit 1
fi

TOKEN_FILE="$HOME/.hubmq-agent/credentials/telegram-bot-token"
[[ ! -r "$TOKEN_FILE" ]] && { echo "Token illisible: $TOKEN_FILE" >&2; exit 2; }
TOKEN=$(cat "$TOKEN_FILE")

# Telegram accepte max 4096 chars par message → tronquer si nécessaire
if [[ ${#MESSAGE} -gt 4000 ]]; then
    MESSAGE="${MESSAGE:0:3900}

[... message tronqué à 3900 chars, voir vault-mem pour l'intégrale]"
fi

RESP=$(curl -sS -m 10 "https://api.telegram.org/bot${TOKEN}/sendMessage" \
    -d chat_id="$CHAT_ID" \
    --data-urlencode "text=$MESSAGE")

if echo "$RESP" | grep -q '"ok":true'; then
    echo "OK"
    exit 0
else
    echo "Telegram API error: $RESP" >&2
    exit 3
fi
