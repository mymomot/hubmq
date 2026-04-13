# HubMQ — Plan & avancement (post 2026-04-13 05:45)

## État actuel

### ✅ Phase Core — LIVE
- 23/23 tasks Phase Core livrées (`47e70ad`)
- hubmq daemon LIVE sur LXC 415 (`hubmq.service`, 31 tests PASS)
- NATS JetStream 6 streams (ALERTS / MONITOR / AGENTS / SYSTEM / CRON / USER_IN)
- Bot Telegram `@hubmqbot` opérationnel (polling, chat_id allowlist 1451527482)
- SMTP Gmail `mymomot74@gmail.com` validé
- ntfy push (LAN only Phase Core, pas encore déployé sur LXC 415 — optionnel)
- Apprise subprocess sink fonctionnel (fix tag bug `47e70ad`)

### ✅ claude-hubmq jumeau — LIVE (2026-04-13)
- Workspace sandboxed `~/.hubmq-agent/workspace/`
- Listener systemd `hubmq-agent-listener.service` → NATS sub → spawn
- `claude --continue --print` headless + permissions.deny sur gouvernance centrale
- Partage contexte vault-mem tag=thread avec Claude principal
- Commit `e40d63f`

## Plan restant (priorités)

### Phase 2 — Solidification (~2-3h effort cumulé)

#### P1 — Anti-double-réponse
Lock file `/tmp/hubmq-claude.owner` :
- Claude principal en session → `touch` le lock au startup + `rm` au SIGTERM
- `hubmq-agent-spawn.sh` vérifie le lock → skip si présent (laisse Claude principal répondre via `/loop msg-relay-cli check`)

#### P2 — Rotation session jumeau
Dans `hubmq-agent-spawn.sh`, avant spawn :
```bash
JSONL_DIR=~/.claude/projects/-home-motreffs-.hubmq-agent-workspace
LAST=$(stat -c %Y "$JSONL_DIR"/*.jsonl 2>/dev/null | sort -n | tail -1)
if [[ -n "$LAST" ]] && [[ $(($(date +%s) - LAST)) -gt 86400 ]]; then
    mkdir -p ~/tmp/hubmq-claude-archive
    mv "$JSONL_DIR"/*.jsonl ~/tmp/hubmq-claude-archive/
fi
```

#### P3 — Wazuh FIM jumeau
Groupe Wazuh `hubmq-agent` (LXC 500) surveille :
- `~/.hubmq-agent/workspace/CLAUDE.md` (toute modif = alerte)
- `~/.hubmq-agent/workspace/.claude/settings.json` (permissions deny)
- `~/.hubmq-agent/wrapper/*.sh`
- `~/.hubmq-agent/credentials/` (audit accès)

#### P4 — Bridge whitelist bypass chat_id (petit patch Rust)
`crates/hubmq-core/src/bridge.rs` : si `m.meta["chat_id"] ∈ config.telegram.allowed_chat_ids`, bypass whitelist verbe. Actuellement la whitelist filtre pour `status/logs/help`. Pour chat allowlisté (authentifié), tout message passe.

### Phase 3 — BigBrother agent (~3-4h)

Daemon séparé (pas dans hubmq) consumer NATS `ALERTS/MONITOR/SYSTEM` :
- Invoqué par timer systemd toutes les 1-6h : `claude-code-cli-skill.sh --mode audit --model sonnet`
- Rôle : agréger, corréler, résumer, juger pertinence
- Si pertinent → publish NATS `agent.bigbrother.summary` sur stream AGENTS
- hubmq daemon dispatcher consume AGENTS → DM Telegram via Apprise OU send-telegram.sh

Patch nécessaire `crates/hubmq-core/src/dispatcher.rs` :
- Retirer ALERTS/MONITOR/SYSTEM des consumers directs
- Ajouter AGENTS + CRON (CRON bypass justifié, peu de bruit)

### Phase Exposure (~2j, reporté sans urgence)

- ntfy.sh exposé sur `ntfy.mymomot.ovh` (public Internet) via Traefik
- Telegram webhook au lieu de polling (latence 100ms → 50ms, gain marginal)
- Rate limit Traefik + IP allowlist Telegram CIDR (B4/B5 council conditions)

## Points de vigilance

### Confusion de noms
- `hubmq` = le service Rust (daemon LXC 415)
- `@hubmqbot` = le bot Telegram (interface)
- `claude-hubmq` = le jumeau Claude Code (répondeur)
- `HubMQ` (hostname) = le LXC 415

Cohérent sémantiquement (tout est "le hub") mais peut créer ambiguïté dans les conversations. OK pour Stéphane, validé explicitement.

### Coût API Anthropic
Chaque spawn claude-hubmq consomme :
- Contexte chargé : ~15-25k tokens (workspace CLAUDE.md + thread vault + MCP tools)
- Réponse : 2-5k tokens
- Estimation : ~0.10-0.25€ par réponse

À monitorer sur 1 semaine. Si abus → ajouter filtre côté bot (ex: rejet "pattern babillage" en amont).

### Coexistence Claude principal ↔ jumeau
Actuellement **pas de coordination**. Si Stéphane écrit pendant que je suis en session :
1. Jumeau spawn et répond (via NATS USER_IN)
2. Moi dans session aussi je vois via msg-relay → je pourrais répondre aussi
→ double réponse possible. P1 à faire.

### Identité dans vault-mem
- `author=Claude Code` = Claude principal (moi, session terminal)
- `author=claude-hubmq` = le jumeau
Permet audit qui a dit quoi, valide/rejette transversalement.

## Fichiers clés créés dans cette session

```
~/projects/hubmq/deploy/agent/
├── CLAUDE-HUBMQ-AGENT.md       # charter jumeau
├── settings.json                # permissions.deny
├── hubmq-agent-spawn.sh         # wrapper spawn
├── hubmq-agent-listener.sh      # listener daemon bash
├── hubmq-agent-listener.service # unit systemd
├── send-telegram.sh             # helper API Telegram
└── README.md                    # doc déploiement complète
```

Fichiers déployés (hors git, pattern `.zeroclaw/workspace/skill-workspaces` étendu) :
```
~/.hubmq-agent/
├── workspace/            # CWD du jumeau (CLAUDE.md, .claude/settings.json, CONSTITUTION.md symlink)
├── wrapper/              # scripts (copies locales des fichiers versionnés dans repo)
└── credentials/          # telegram-bot-token + nats-hubmq-service.seed (chmod 600)
```

## Prochaine session Claude principal

1. Lire `vault_search section=conversations tag=thread limit=30` pour voir ce que claude-hubmq a dit pendant l'absence
2. Décider P1 (lock anti-double-réponse) et l'implémenter si besoin
3. Rotation session si >24h d'inactivité observée
4. BigBrother agent (Phase 3) — réfléchir design + implémentation
