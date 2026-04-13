# HubMQ Agent — `claude-hubmq` (jumeau Claude via Telegram)

Configuration pour déployer un **clone comportemental de Claude Code** accessible via le bot Telegram `@hubmqbot`. Le jumeau répond aux messages de Stéphane 24/7 sans nécessiter de session terminal active.

## Principe

- Le **Claude principal** (session interactive sur LXC 500) garde l'autorité centrale (CLAUDE.md, CONSTITUTION.md, push code)
- Le **jumeau claude-hubmq** est spawné à chaque message Telegram via cron/event, partage le contexte via vault-mem tag=thread, a un périmètre sandboxed (lecture seule sur gouvernance)
- Partage de contexte via `vault-mem` MCP (`section=conversations tag=thread`)

## Flow opérationnel

```
Stéphane → @hubmqbot (Telegram)
             ↓ (teloxide polling)
         hubmq daemon (LXC 415)
             ↓ NATS publish
         user.incoming.telegram → stream USER_IN
             ↓ (consumer)
         hubmq-agent-listener.service (LXC 500, bash daemon)
             ↓ bash spawn
         hubmq-agent-spawn.sh
             ↓ injecte vault-mem thread
         claude --continue --print --setting-sources user
             ↓ (le jumeau raisonne, utilise MCP)
         send-telegram.sh → Telegram API
             ↓
         DM reçue par Stéphane
```

## Déploiement (étapes réalisées)

### 1. Workspace jumeau (LXC 500)
```bash
mkdir -p ~/.hubmq-agent/workspace/.claude ~/.hubmq-agent/wrapper ~/.hubmq-agent/credentials
chmod 700 ~/.hubmq-agent/credentials
ln -sfn ~/.claude/agents ~/.hubmq-agent/workspace/.claude/agents
ln -sfn ~/.claude/skills ~/.hubmq-agent/workspace/.claude/skills
ln -sf ~/CONSTITUTION.md ~/.hubmq-agent/workspace/CONSTITUTION.md
```

### 2. Charter (CLAUDE.md du jumeau)
Fichier `CLAUDE-HUBMQ-AGENT.md` → copier en `~/.hubmq-agent/workspace/CLAUDE.md`.
Définit identité, périmètre autorisé/interdit, mémoire partagée, format de réponse Telegram.

### 3. Permissions natives Claude Code
Fichier `settings.json` → copier en `~/.hubmq-agent/workspace/.claude/settings.json`.
Utilise `permissions.deny` pour bloquer au niveau tool-call :
- Write/Edit `~/CLAUDE.md`, `~/CONSTITUTION.md`, `~/.claude/agents/**`, `~/.claude/skills/**`
- `Bash(rm -rf*)`, `Bash(systemctl stop hubmq*)`, `Bash(git push*)`, etc.

### 4. Credentials locaux LXC 500
```bash
# Token Telegram (copy from LXC 415)
ssh hubmq "sudo cat /etc/hubmq/credentials/telegram-bot-token" > ~/.hubmq-agent/credentials/telegram-bot-token
chmod 600 ~/.hubmq-agent/credentials/telegram-bot-token

# NATS NKey seed (copy from LXC 415)
ssh hubmq "sudo cat /etc/nats/nkeys/hubmq-service.seed" > ~/.hubmq-agent/credentials/nats-hubmq-service.seed
chmod 600 ~/.hubmq-agent/credentials/nats-hubmq-service.seed
```

### 5. nats CLI sur LXC 500
```bash
sudo apt install -y unzip
curl -sL https://github.com/nats-io/natscli/releases/download/v0.1.5/nats-0.1.5-linux-amd64.zip -o /tmp/nats.zip
sudo unzip -oq /tmp/nats.zip -d /tmp/
sudo mv /tmp/nats-0.1.5-linux-amd64/nats /usr/local/bin/
sudo chmod +x /usr/local/bin/nats
```

### 6. Scripts wrapper
- `send-telegram.sh "$CHAT_ID" "$MSG"` → `sendMessage` via API Telegram (token depuis credentials local)
- `hubmq-agent-spawn.sh "$MSG" "$CHAT_ID"` → construit prompt enrichi + lance `claude --continue --print`
- `hubmq-agent-listener.sh` → daemon bash qui `nats sub user.incoming.telegram` et spawne le wrapper

### 7. Unité systemd
```bash
sudo cp hubmq-agent-listener.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now hubmq-agent-listener.service
sudo systemctl is-active hubmq-agent-listener.service   # active
```

## Vérification

```bash
# Envoyer un message au bot depuis Telegram, puis :
tail -f ~/.hubmq-agent/spawn.log

# Envoyer manuellement un event NATS :
echo '{"id":"test","ts":"...","source":"telegram","severity":"P2","title":"t","body":"salut","tags":[],"meta":{"chat_id":"1451527482","message_id":"1"}}' | \
  /usr/local/bin/nats publish user.incoming.telegram \
  --nkey ~/.hubmq-agent/credentials/nats-hubmq-service.seed \
  --server nats://192.168.10.15:4222 \
  "$(cat)"
```

## Sécurité

| Couche | Mécanisme | Effet |
|---|---|---|
| System prompt | CLAUDE.md charter | Le jumeau connaît son périmètre |
| Claude Code natif | `permissions.deny` | Tool call Edit/Write rejeté avant exécution |
| Fallback erreur | Catch dans spawn.sh | Message `⚠️ erreur` envoyé à Stéphane si échec |
| Logs | `~/.hubmq-agent/{listener,spawn}.log` | Audit + debug |
| Credentials | chmod 600 motreffs:motreffs | Pas accessibles autres users |
| Wazuh FIM | groupe hubmq LXC 500 (à ajouter) | Détection modification suspecte |

## Limites connues

- **Double réponse potentielle** : si Claude principal est en session active avec `/loop msg-relay-cli check`, il peut aussi répondre au même message → TODO : lock file `/tmp/hubmq-claude.owner` ou ack dans msg-relay
- **Latence réponse** : spawn Claude ~10-30s (chargement + inférence). Stéphane voit "…" en attendant.
- **Coût** : chaque spawn consomme API Anthropic (~20k tokens contexte + 2-5k réponse). À monitorer.

## Rotation session

Pattern recommandé dans le spawn script (TODO Phase 2) :
```bash
# Archive session dormante > 24h
LAST=$(stat -c %Y ~/.claude/projects/<encoded-cwd>/*.jsonl 2>/dev/null | sort -n | tail -1)
if [[ -n "$LAST" ]] && [[ $(($(date +%s) - LAST)) -gt 86400 ]]; then
    mkdir -p ~/tmp/hubmq-claude-archive
    mv ~/.claude/projects/<encoded-cwd>/*.jsonl ~/tmp/hubmq-claude-archive/
fi
```
