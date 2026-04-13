# BigBrother agent — observabilité transversale homelab

Agent Claude Code autonome qui consomme les streams NATS bruts (ALERTS, MONITOR, SYSTEM) de HubMQ, corrèle, juge la pertinence, et publie des digests curés sur `agent.bigbrother.summary` (stream AGENTS). HubMQ dispatcher consomme AGENTS → forwarde DM à Stéphane.

## Architecture

```
Wazuh/Services → ALERTS (7j)   ─┐
Health checks  → MONITOR (24h) ─┼─▶ BigBrother (timer 1h)
Systemd/kernel → SYSTEM (3j)   ─┘        │
                                         ▼
                              agent.bigbrother.summary (AGENTS)
                                         │
                                         ▼
                              HubMQ dispatcher → Telegram DM
```

**CRON bypass BigBrother** (prévisible, direct dispatcher).

## Fichiers

| Path | Rôle |
|---|---|
| `workspace/CLAUDE.md` | Charter (périmètre, mission, format publish) |
| `workspace/.claude/settings.json` | Permissions (allow/deny strict, defaultMode acceptEdits) |
| `wrapper/bigbrother-analyze.sh` | Spawn Claude CLI + heartbeat post-run |
| `wrapper/bigbrother-watchdog.sh` | Détecte BB silencieux + fallback DM direct si ALERTS non vide |
| `bigbrother-analyze.{service,timer}` | systemd unit + timer 1h (Persistent=true) |
| `bigbrother-watchdog.{service,timer}` | systemd unit + timer 30min |

## Déploiement (LXC 500)

```bash
# Répertoires
mkdir -p ~/.bigbrother-agent/{workspace/.claude,workspace/memory,wrapper,credentials,logs}

# Symlinks agents/skills (évite duplication)
ln -sf ~/.claude/agents ~/.bigbrother-agent/workspace/.claude/agents
ln -sf ~/.claude/skills ~/.bigbrother-agent/workspace/.claude/skills

# Copie seed NATS (même nkey que hubmq-agent — lecture streams + publish agent.>)
cp ~/.hubmq-agent/credentials/nats-hubmq-service.seed ~/.bigbrother-agent/credentials/
chmod 600 ~/.bigbrother-agent/credentials/*

# Charter + settings
cp deploy/bigbrother-agent/workspace/CLAUDE.md ~/.bigbrother-agent/workspace/
cp deploy/bigbrother-agent/workspace/.claude/settings.json ~/.bigbrother-agent/workspace/.claude/

# Wrappers
cp deploy/bigbrother-agent/wrapper/*.sh ~/.bigbrother-agent/wrapper/
chmod +x ~/.bigbrother-agent/wrapper/*.sh

# Systemd
sudo install -m 644 deploy/bigbrother-agent/bigbrother-*.{service,timer} /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now bigbrother-analyze.timer bigbrother-watchdog.timer
```

## Fenêtres de surveillance

| Paramètre | Valeur | Fichier |
|---|---|---|
| BB run period | 1h | `bigbrother-analyze.timer` (`OnUnitActiveSec`) |
| BB lookback stream | 6h | `workspace/CLAUDE.md` (mission étape 1) |
| Watchdog period | 30min | `bigbrother-watchdog.timer` |
| Heartbeat max age | 2h | `bigbrother-watchdog.sh` (`HEARTBEAT_MAX_AGE`) |
| Watchdog dédup | 1h | `bigbrother-watchdog.sh` (`DEDUP_WINDOW`) |

## Règle "silence > bruit"

BigBrother publie **uniquement** si pertinent (P0/P1/P2 non déjà résolu). RAS = note vault-mem courte, pas de DM.

Watchdog publie **uniquement** si BB silencieux >2h ET ALERTS non vide — évite fenêtre aveugle côté Stéphane.

## Backlog

- **P3.7** vault-mem MCP pour jumeau sandboxé (notes RAS actuellement non persistées)
- **Phase 5** : correction de course `nats stream subjects` vs `analyze.log` parse (actuellement fallback FS, acceptable)
