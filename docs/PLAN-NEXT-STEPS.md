# HubMQ — Plan détaillé des phases restantes

Document opérationnel de référence pour les prochaines phases post-LIVE.
Vue d'ensemble & statut dans `CLAUDE-HUBMQ.md` à la racine.

---

## ✅ DONE

### Phase Core (2026-04-12, LIVE)
- 23 tasks implémentation (commits `9bafca6` → `75aafcc`)
- NATS JetStream v2.10.24 + 6 streams (limits D2)
- daemon `hubmq.service` Rust Axum (26 MB binaire, 31 tests PASS)
- Apprise Telegram outgoing (fix `47e70ad`)
- SMTP Gmail validé + fallback P0 local (B1)
- systemd LoadCredential (D3), OnFailure (B1), heartbeat 1h
- CI/CD Forgejo + mirror GitHub

### claude-hubmq jumeau (2026-04-13, LIVE)
- Workspace sandboxed `~/.hubmq-agent/workspace/`
- `permissions.deny` natif Claude Code
- Listener systemd NATS → spawn headless
- Partage vault-mem `section=conversations tag=thread`
- E2E validé (~60s message → DM)
- Commit `e40d63f` + doc `deploy/agent/README.md`

---

## Phase 2 — Solidification claude-hubmq (~3h total)

Objectif : rendre le jumeau fiable en production continue.

### P2.1 — Lock anti-double-réponse (30 min)

**Problème** : si Claude principal est en session terminal active avec `/loop 1m msg-relay-cli check`, il répond aussi au message Telegram → double DM à Stéphane.

**Solution** : lock file `/tmp/hubmq-claude.owner` avec TTL.

**Livrables** :

1. **Hook Claude principal** — settings.json (user global) :
```json
{
  "hooks": {
    "SessionStart": [{"type": "command", "command": "touch /tmp/hubmq-claude.owner"}],
    "SessionEnd": [{"type": "command", "command": "rm -f /tmp/hubmq-claude.owner"}]
  }
}
```

2. **Check dans `hubmq-agent-spawn.sh`** (ligne 1 du script, avant tout) :
```bash
LOCK=/tmp/hubmq-claude.owner
if [[ -f "$LOCK" ]] && [[ $(($(date +%s) - $(stat -c %Y "$LOCK"))) -lt 3600 ]]; then
    echo "[$(date -Is)] SKIP — Claude principal détient le lock (<1h)" >> ~/.hubmq-agent/spawn.log
    exit 0
fi
```

3. **Heartbeat refresh** — depuis Claude principal (je fais `touch` toutes les 10 min via TaskUpdate metadata) OU via hook `UserPromptSubmit`.

4. **Tests** : lancer Claude principal, envoyer msg Telegram → spawn jumeau skippé ; fermer Claude → jumeau reprend.

**Effort** : 30 min.

### P2.2 — Rotation session JSONL (30 min)

**Problème** : si on laisse `--continue` sans fin, la session JSONL grossit indéfiniment (compactions successives → perte détails lointains).

**Solution** : rotation par inactivité.

**Livrables** :

1. **Header dans `hubmq-agent-spawn.sh`** :
```bash
# Encoded CWD
ENCODED=$(echo "$WORKSPACE" | sed 's|/|-|g' | sed 's|^-||')
JSONL_DIR="$HOME/.claude/projects/$ENCODED"

if [[ -d "$JSONL_DIR" ]]; then
    LAST=$(ls -t "$JSONL_DIR"/*.jsonl 2>/dev/null | head -1)
    if [[ -n "$LAST" ]]; then
        LAST_MTIME=$(stat -c %Y "$LAST")
        AGE=$(( $(date +%s) - LAST_MTIME ))
        if [[ $AGE -gt 86400 ]]; then
            mkdir -p ~/tmp/hubmq-claude-archive
            ts=$(date +%Y%m%d-%H%M%S)
            mv "$JSONL_DIR"/*.jsonl ~/tmp/hubmq-claude-archive/ 2>/dev/null
            echo "[$(date -Is)] ROTATE — archive session dormante >24h" >> ~/.hubmq-agent/spawn.log
        fi
    fi
fi
```

2. **Cron nettoyage archives >30j** :
```bash
0 3 * * * find ~/tmp/hubmq-claude-archive/ -name "*.jsonl" -mtime +30 -delete
```

**Effort** : 30 min.

### P2.3 — Wazuh FIM groupe hubmq-agent (30 min)

**Problème** : si le charter CLAUDE.md ou les scripts du jumeau sont modifiés par un attaquant, pas d'alerte.

**Solution** : ajouter groupe Wazuh `hubmq-agent` + FIM paths.

**Livrables** :

1. **Agent LXC 500 (Wazuh ID 002 forge-lxc500) rejoint le nouveau groupe** :
```bash
ssh motreffs@192.168.10.12 "sudo docker exec wazuh-wazuh.manager-1 /var/ossec/bin/agent_groups -a -i 002 -g hubmq-agent -q"
```

2. **Config FIM groupe** sur manager :
```bash
ssh motreffs@192.168.10.12 "sudo docker exec wazuh-wazuh.manager-1 bash -c '
mkdir -p /var/ossec/etc/shared/hubmq-agent
cat > /var/ossec/etc/shared/hubmq-agent/agent.conf <<EOF
<agent_config>
  <syscheck>
    <directories realtime=\"yes\">/home/motreffs/.hubmq-agent/workspace/CLAUDE.md</directories>
    <directories realtime=\"yes\">/home/motreffs/.hubmq-agent/workspace/.claude/settings.json</directories>
    <directories realtime=\"yes\">/home/motreffs/.hubmq-agent/wrapper</directories>
    <directories check_all=\"yes\">/home/motreffs/.hubmq-agent/credentials</directories>
    <directories realtime=\"yes\">/etc/systemd/system/hubmq-agent-listener.service</directories>
  </syscheck>
</agent_config>
EOF
'"
```

3. **Test modification** — `touch ~/.hubmq-agent/workspace/CLAUDE.md` → alerte Wazuh dans les <30s.

**Effort** : 30 min.

### P2.4 — Bridge whitelist bypass chat_id allowlisté (1h)

**Problème** : actuellement, seuls les verbes `["status","logs","help"]` forwardés vers msg-relay. Tous les autres messages de Stéphane → audit "rejected" → perdus pour moi en session.

**Solution** : bypass whitelist verbe pour chat_id authentifié (allowlisté).

**Livrables** (patch Rust `crates/hubmq-core/src/bridge.rs`) :

```diff
 pub async fn forward_from_telegram(&self, state: &AppState, m: &Message) -> anyhow::Result<bool> {
     let Some(url) = &self.url else {
         return Ok(false);
     };

     let verb = m.body.split_whitespace().next().unwrap_or("").to_lowercase();
-    if !self.whitelist.iter().any(|w| w.eq_ignore_ascii_case(&verb)) {
+    let chat_id_allowlisted = m.meta.get("chat_id")
+        .and_then(|s| s.parse::<i64>().ok())
+        .map(|id| state.cfg.telegram.allowed_chat_ids.contains(&id))
+        .unwrap_or(false);
+
+    if !chat_id_allowlisted && !self.whitelist.iter().any(|w| w.eq_ignore_ascii_case(&verb)) {
         state.audit.log(...).await.ok();
         return Ok(false);
     }
+
+    // chat_id allowlisté OU verbe whitelist → forward
     ...
```

**Tests** :
- `cargo test -p hubmq-core bridge` (ajouter test cas bypass chat_id)
- E2E : envoyer "salut claude" au bot → doit forwarder (pas uniquement "status/logs/help")

**Build + deploy** :
- `cargo build --release -p hubmq`
- `bash deploy/deploy-hubmq.sh target/release/hubmq` (push main déclenche auto via CI)

**Effort** : 1h (inclus tests + deploy).

### Phase 2 — Total : ~3h. Ordre recommandé : P2.4 (le plus utile) → P2.1 → P2.2 → P2.3.

---

## Phase 3 — BigBrother agent autonome (~4h)

Objectif : avoir un observateur intelligent qui lit les streams bruts ALERTS/MONITOR/SYSTEM, corrèle, juge pertinence, publie des résumés curés sur stream AGENTS (qui sera le seul consumé par hubmq pour Telegram).

### P3.1 — Workspace BigBrother dédié (30 min)

```
~/.bigbrother-agent/
├── workspace/                   # CWD dédié
│   ├── CLAUDE.md                # charter BigBrother (rôle = analyste monitoring)
│   ├── .claude/
│   │   ├── settings.json        # permissions.deny (pas modifier rien, lecture seule + publish NATS)
│   │   ├── agents -> ~/.claude/agents
│   │   └── skills -> ~/.claude/skills
│   └── memory/
├── wrapper/
│   ├── bigbrother-analyze.sh    # script invocation claude --mode audit
│   └── bigbrother-publish.sh    # helper publish NATS agent.bigbrother.summary
└── credentials/
    └── nats-hubmq-service.seed  # chmod 600 (copy existing)
```

### P3.2 — Charter BigBrother CLAUDE.md (30 min)

Spécifie :
- **Rôle** : analyste sécurité + observabilité transversal
- **Périmètre** : lecture streams NATS, vault-mem R/W, nexus search, bash lecture (systemctl, journalctl, curl health)
- **Interdits** : tout Write/Edit filesystem sauf son propre workspace, pas de git, pas de deploy
- **Mission à chaque invocation** :
  1. Lire `nats stream view ALERTS --last 6h`
  2. Lire `nats stream view MONITOR --last 6h`
  3. Lire `nats stream view SYSTEM --last 6h`
  4. Identifier les corrélations (même timestamp ±5min, même host, patterns répétés)
  5. Comparer avec vault-mem (notes resolution précédentes → éviter alerte sur problème déjà traité)
  6. Juger la pertinence (est-ce que Stéphane doit être dérangé ?)
  7. Si pertinent → publish NATS `agent.bigbrother.summary` avec severity + titre + body (markdown)
  8. Persister analyse dans vault-mem `section=monitoring tag=bigbrother-analysis`
- **Format output NATS** :
```json
{
  "id": "uuid",
  "ts": "2026-04-13T...",
  "source": "bigbrother",
  "severity": "P0|P1|P2|P3",
  "title": "synthèse courte (1 ligne)",
  "body": "analyse détaillée markdown",
  "tags": ["monitoring","correlation"],
  "meta": {"events_analyzed": "42", "alerts_in": "...", "duration_sec": "..."}
}
```

### P3.3 — Wrapper `bigbrother-analyze.sh` (30 min)

```bash
#!/usr/bin/env bash
set -euo pipefail
WORKSPACE="$HOME/.bigbrother-agent/workspace"
LOG="$HOME/.bigbrother-agent/analyze.log"
cd "$WORKSPACE"

echo "[$(date -Is)] START analyze" >> "$LOG"

PROMPT="Tu es BigBrother. Aujourd'hui $(date -Is).
Utilise nats CLI (seed $HOME/.bigbrother-agent/credentials/nats-hubmq-service.seed, server nats://192.168.10.15:4222)
pour lire les streams ALERTS, MONITOR, SYSTEM des 6 dernières heures.
Corrèle, compare avec vault-mem (tag=bigbrother-analysis, limit=10 notes récentes).
Si rien de nouveau pertinent → silence total (pas de publish, note vault courte 'RAS').
Si pertinent → publish NATS agent.bigbrother.summary + vault_write analyse complète.
Respecte le charter ~/.bigbrother-agent/workspace/CLAUDE.md.
"

claude --print --continue --setting-sources user --max-turns 20 "$PROMPT" >> "$LOG" 2>&1 || {
    echo "[$(date -Is)] ERROR exit $?" >> "$LOG"
    exit 1
}
echo "[$(date -Is)] DONE" >> "$LOG"
```

### P3.4 — Systemd timer (15 min)

```ini
# /etc/systemd/system/bigbrother-analyze.timer
[Unit]
Description=BigBrother analysis every 1h

[Timer]
OnBootSec=15min
OnUnitActiveSec=1h
Unit=bigbrother-analyze.service

[Install]
WantedBy=timers.target
```

```ini
# /etc/systemd/system/bigbrother-analyze.service
[Unit]
Description=BigBrother monitoring analysis (Claude CLI)
After=network-online.target

[Service]
Type=oneshot
User=motreffs
ExecStart=/home/motreffs/.bigbrother-agent/wrapper/bigbrother-analyze.sh
```

### P3.5 — Patch hubmq dispatcher (1h30)

**Problème actuel** : `dispatcher.rs` consume directement ALERTS/MONITOR/SYSTEM/CRON et envoie tout vers Telegram → bruit max.

**Correction conforme à la vision** :
- **Retirer** consumers ALERTS/MONITOR/SYSTEM du dispatcher direct downstream
- **Ajouter** consumer AGENTS (seul stream "curé" par BigBrother)
- **Garder** consumer CRON (prévisible, peu de bruit, bypass BigBrother)

Diff `crates/hubmq-core/src/dispatcher.rs` :
```diff
-let streams = [("ALERTS", "hubmq-alerts"), ("MONITOR", "hubmq-monitor"),
-                ("SYSTEM", "hubmq-system"), ("CRON", "hubmq-cron")];
+let streams = [("AGENTS", "hubmq-agents"), ("CRON", "hubmq-cron")];
```

Bonus : les streams ALERTS/MONITOR/SYSTEM restent en archive NATS (rétention 7j/24h/3j) et sont lus par BigBrother seulement.

**Tests** :
- `cargo test` (vérifier que retirer ne casse rien)
- E2E : publier fake sur `alert.wazuh.critical` → **pas de DM Telegram** (filtré par BigBrother) ; publier `cron.backup.failed` → DM direct ; publier `agent.bigbrother.summary` → DM via dispatcher

**Build + deploy** via CI.

### P3.6 — Dry-run BigBrother (30 min)

- Lancer manuellement `~/.bigbrother-agent/wrapper/bigbrother-analyze.sh` 3× (matin/midi/soir)
- Vérifier logs
- Ajuster charter si faux positifs ou manques

### Phase 3 — Total : ~4h. Gain : Stéphane ne reçoit plus du bruit Wazuh brut, seulement des digests curés par Claude (BigBrother) intelligents.

---

## Phase Exposure — Internet-facing (~2 jours)

Objectif : permettre push mobile hors LAN + webhook Telegram performant.

### PX.1 — ntfy public (1 jour)

**Installation ntfy sur LXC 415** :

```bash
ssh hubmq "curl -sL https://github.com/binwiederhier/ntfy/releases/download/v2.11.0/ntfy_2.11.0_linux_amd64.tar.gz | sudo tar xz -C /tmp/ && sudo mv /tmp/ntfy_2.11.0_linux_amd64/ntfy /usr/local/bin/ && sudo chmod +x /usr/local/bin/ntfy"
```

**Config** `/etc/ntfy/server.yml` :
```yaml
base-url: https://ntfy.mymomot.ovh
listen-http: 0.0.0.0:2586
cache-file: /var/lib/ntfy/cache.db
auth-default-access: deny-all
auth-file: /var/lib/ntfy/user.db
behind-proxy: true
visitor-request-limit-burst: 50
visitor-request-limit-replenish: 10s
visitor-message-daily-limit: 5000
```

**Users + topics** :
```bash
ntfy user add stephane --role user
ntfy access stephane "hubmq-alerts-$(uuidgen)" rw
```

**DNS AdGuard** : rewrite `ntfy.mymomot.ovh` → `192.168.10.10` (déjà en LAN... ou DNS OVH public direct IP publique → Traefik).

**Traefik vhost** :
- Host rule : `ntfy.mymomot.ovh`
- Forward : `http://192.168.10.15:2586`
- Middlewares : `ratelimit` (20 req/s, burst 50), CrowdSec optionnel
- **Pas de SSO Authentik** (mobile app doit pouvoir atteindre)
- TLS : certResolver ovh (wildcard existant)

**Update config hubmq** `/etc/hubmq/config.toml` :
```toml
[ntfy]
base_url = "https://ntfy.mymomot.ovh"
topic = "<uuid-non-devinable>"
bearer_credential = "ntfy-bearer"
```

Déposer le Bearer token dans `/etc/hubmq/credentials/ntfy-bearer` + LoadCredential dans systemd.

**App mobile** : Samsung S25+ → Play Store ntfy → add server `ntfy.mymomot.ovh` + username/password → subscribe topic UUID.

**Test E2E** : `curl POST /in/generic severity=P0` → push instantané sur tel même en 4G.

### PX.2 — Telegram webhook (0.5 jour)

Remplacer le polling teloxide par webhook.

**Traefik vhost** `hubmq-webhook.mymomot.ovh` :
- Host rule
- Forward : `http://192.168.10.15:8471` (nouveau port webhook daemon hubmq)
- Middleware `ipAllowList` : CIDR Telegram officiels `91.108.4.0/22, 91.108.56.0/22, 149.154.160.0/20, 149.154.0.0/17`
- **Pas de SSO** (Telegram doit atteindre)
- TLS certResolver ovh

**Patch daemon hubmq** `crates/hubmq-core/src/source/telegram.rs` :
- Ajouter route Axum `POST /webhook/<random_secret_uuid>`
- Parser `Update` JSON
- Mêmes filtres (forward_origin rejection, chat_id allowlist) que polling mode
- `setWebhook` appelé au démarrage hubmq daemon avec URL `https://hubmq-webhook.mymomot.ovh/webhook/<secret>`

**Config** :
```toml
[telegram]
mode = "webhook"  # ou "polling"
webhook_url = "https://hubmq-webhook.mymomot.ovh/webhook/abc123..."
webhook_secret = "abc123..."  # loadCredential
```

**Gain latence** : 100ms → 50ms (marginal pour Stéphane, mais propre techniquement).

### Phase Exposure — Total : 1.5 jour. Gain réel : push mobile hors LAN > webhook Telegram.

---

## Phase Monitoring (futur, TBD)

Si volume d'alertes justifie.

### PM.1 — Prometheus exporter NATS

NATS expose déjà métriques Prometheus :
- `/varz` → memory, connections, msg rates
- `/jsz` → stream depths, consumer lags

Ajouter scrape job Prometheus. Prometheus pas encore déployé sur le homelab — décision à prendre séparément.

### PM.2 — Grafana dashboards

Templates existants Synadia (NATS Surveyor). Dashboards :
- Traffic par stream
- Latence delivery P50/P95/P99
- Consumer lag
- Messages in/out par source
- Fallback P0 triggered count

### PM.3 — Alertmanager

Alertes meta sur HubMQ lui-même (meta-monitoring) :
- hubmq.service down >1 min
- Consumer lag >10k messages
- Disk NATS >80% full
- Aucun message pendant 12h (silent broken ?)

---

## Décisions en suspens

1. **Stream AGENTS — qui consomme ?**
   - Option A : hubmq dispatcher (après patch P3.5) — fait sortir vers Telegram les digests BigBrother
   - Option B : multiple consumers (claude-hubmq aussi consume pour contexte proactif ?)
   - **Reco** : A (simple, testé), B si besoin émerge

2. **BigBrother — granularité timer**
   - 1h (par défaut proposé)
   - 6h (plus économique, moins réactif)
   - Dynamique (ex: 15min quand ALERTS pique, 4h sinon) → complexité ++
   - **Reco** : 1h, ajuster après observation

3. **Coût API Anthropic**
   - Phase 2 : négligeable (P2.1-P2.3 pas de spawn claude, P2.4 pas de spawn)
   - Phase 3 : BigBrother timer 1h + claude-hubmq par message Stéphane = ~2-5€/jour estimation haute
   - **Mitigation** : utiliser `--model haiku` pour BigBrother simple (analyse pattern stream → pas besoin Opus)

4. **Retirer ou garder CRON bypass dispatcher ?**
   - CRON = prévisible (jobs connus, peu de bruit)
   - Si BigBrother peut aussi curer CRON → cohérence
   - **Reco** : garder bypass CRON en Phase 3, ré-évaluer en Phase 4 si BigBrother prouve sa valeur

---

## Ordre d'exécution recommandé

1. **P2.4** (bridge bypass chat_id) — débloque conversations libres avec moi en session
2. **P2.1** (lock anti-double) — évite pollution double réponse
3. **P2.2** (rotation session) — hygiène baseline
4. **P2.3** (Wazuh FIM) — sécurité baseline
5. **P3** (BigBrother complet) — réduit le bruit à la source
6. **PX.1** (ntfy public) — confort mobile
7. **PX.2** (Telegram webhook) — dernier (gain marginal)
8. **PM** (monitoring) — quand le volume justifie

**Total estimé Phase 2 + Phase 3** : **~7h** d'implémentation = 1 journée Claude principal en session. Faisable en 1 ou 2 sessions.
