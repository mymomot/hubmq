# Changelog

All notable changes to HubMQ are documented here. Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), [SemVer](https://semver.org/spec/v2.0.0.html).

## [1.0.0] — 2026-04-13

**First production release.** HubMQ passe de "Phase Core LIVE" à un hub de communication homelab pleinement opérationnel : multi-source (Telegram + email), multi-agent déclaratif, administration automatisée par email, observabilité curée via BigBrother + watchdog.

### Added — Phase 5 + 5.2 (multi-bot + email source + admin pipeline)

- **Multi-bot Telegram déclaratif** : registry `conf.d/bots.toml` + `conf.d/agents.toml` hot-rechargeable via SIGHUP. N bots en parallèle, chacun routé vers son agent cible (`target_agent`) sans rebuild Rust.
- **Listener générique `llm-openai-compat`** : appelle n'importe quel endpoint compatible OpenAI (llmcore, gemini, gateway, etc.), publie la réponse sur `agent.<name>.response`, ack at-least-once.
- **Admin consumer NATS** (`admin.bot.add`, `admin.bot.remove`) : ajout/retrait dynamique de bots sans intervention shell, atomic write credentials + conf.d, SIGHUP self pour respawn.
- **Source email IMAP** : polling Gmail 30s, allowlist `From:` strict (extract angle-bracket + comparaison exacte, anti-substring spoofing). Deux formats subject reconnus :
  - `HUBMQ_BOT_TOKEN <bot_name>` + body (`agent: <target>`, `token: <value>`) → pipe admin
  - `[<agent>] <message>` ou `<agent>: <message>` → `user.inbox.<agent>`
- **Sink `email_reply`** : reply dans le thread Gmail via SMTP existant, headers `In-Reply-To` + `References` + `Subject: Re: ...`.
- **Routing source-aware dispatcher** : selon `meta.source`, response routée vers `telegram_sinks[bot_name]` ou `email_reply` (thread).
- **Bootstrap conf.d** : `agents.toml` + `bots.toml` livrés avec deploy, installés si absents (pattern `sudo test -f`).

### Added — Phase 4 (watchdog BigBrother)

- `bigbrother-analyze.sh` publie heartbeat `agent.bigbrother.heartbeat` à chaque run (severity P3, log_only).
- `bigbrother-watchdog.sh` + timer systemd 30min : détecte BB silencieux >2h ET stream ALERTS non vide → DM Telegram directe + audit NATS. Dédup 1h via state file.
- Scripts versionnés dans `deploy/bigbrother-agent/`.

### Added — Phase 3 (BigBrother agent autonome)

- Workspace Claude Code sandboxé `~/.bigbrother-agent/workspace/` (charter + permissions.deny stricts).
- Wrapper `bigbrother-analyze.sh` + systemd timer horaire.
- Dispatcher `streams=[AGENTS, CRON]` : ALERTS/MONITOR/SYSTEM restent archive-only (lus par BigBrother qui publie digests curés sur `agent.bigbrother.summary`).

### Added — Phase 2 (claude-hubmq jumeau hardening)

- Lock anti-double-réponse `/tmp/hubmq-claude.owner` (TTL 1h, hooks SessionStart/UserPromptSubmit/SessionEnd + stale cleanup).
- Rotation session JSONL >24h vers `~/tmp/hubmq-claude-archive/` + cron purge >30j.
- Wazuh FIM groupe `hubmq-agent` : workspace CLAUDE.md + settings + wrapper + credentials + listener unit.

### Added — P2.4 (bridge auth + routing)

- Bridge `forward_from_telegram` : bypass whitelist verbe pour chat_id allowlisté (messages libres Stéphane).
- Bridge bearer auth : `BridgeConfig.bearer_credential` pour `Authorization: Bearer` vers msg-relay (pattern NtfyConfig).
- Bridge source-aware : meta.source=telegram → sink telegram, email → email_reply.

### Changed

- Stream NATS `USER_IN` : subjects étendus `["user.incoming.>", "user.inbox.>"]` pour couvrir tous les canaux USER_IN.
- Stream `AGENTS` : subjects étendus `["agent.>", "admin.>"]` pour capturer les messages admin consumer.
- Source `telegram.rs` : routing par `bot.target_agent` → `user.incoming.telegram` (legacy claude-hubmq) ou `user.inbox.<agent>` (autres).
- Listener `llm_openai` : severity P1 pour réponses (bypass quiet hours), title vide (UX sans préfixe verbeux).
- Dispatcher : `telegram_sinks: HashMap<bot_name, sink>` + fallback `"default"` si meta absent.
- Endpoint llmcore : via `llm-free-gateway :8430` (LXC 500) au lieu de direct `:8080` — routage centralisé + failover.

### Fixed

- `deploy/hubmq.service` : `OnFailure` déplacé de `[Service]` vers `[Unit]` (directive silencieusement ignorée avant ; B1 fallback enfin actif).
- `deploy/deploy-hubmq.sh` : `sudo test -f` au lieu de `[ -f ]` (motreffs n'avait pas accès à `/etc/hubmq/`, config prod écrasé à chaque deploy).
- `deploy/config.toml.example` : typo `hubmq-service.nk` → `.seed` (crashloop au démarrage).
- `email.rs` allowlist : extract angle-bracket + comparaison exacte (bypass substring `attacker@motreff@gmail.com.evil.com`).
- `source/email.rs` parser admin : validation lowercase alignée sur `admin.rs::is_valid_name`.
- SIGHUP `main.rs` : drain hors write lock (évite blocage 15s du dispatcher) + abort all active handles avant respawn (évite `409 TerminatedByOtherGetUpdates` Telegram).

### Security

- Token Telegram : jamais loggé en clair. Audit SQLite stocke `sha256[0:12]` du token uniquement.
- Atomic writes (tmp + rename + backup `.bak`) sur toute modification filesystem sensible (credentials, conf.d).
- Validation stricte admin : regex `^[a-z0-9_-]+$`, cross-refs `target_agent` doit exister dans registry, rejet des noms avec injection path.
- Chain of trust email : `mymomot74@gmail.com` compte dédié agents (MFA actif), `motreff@gmail.com` compte perso Stéphane (MFA actif), communication via tokens Gmail App Password.

### Performance

- E2E Qwen3.5-122B via `@hubmq_llmcore_bot` : **3.02s** mesurés en production (polling → gateway → llmcore Vulkan → publish → Telegram API).

### Tests

- 110 tests PASS + 3 ignored (Gmail/ntfy live E2E) dans `cargo test --workspace`.
- 0 clippy warnings `-D warnings` (warning `imap-proto v0.10.2` future-incompat accepté, dette tracée).

### Known limitations

- `imap` crate v2 (v3 stable pas disponible, seul alpha) — migration prévue.
- Parser RFC822 maison — migration `mailparse` crate prévue en V1.1.
- `admin.bot.add` token transite en clair NATS (localhost trusted, à documenter si évolution).
- Kind `claude-code-spawn` non paramétrable (workspace fixe) — V2 pour spawn N jumeaux avec charter différent.

---

## [0.2.0] — Phases 2 → 4 (2026-04-12 → 2026-04-13)

Documenté rétroactivement dans [1.0.0] (consolidé).

## [0.1.0] — Phase Core (2026-04-12)

- Ingestion HTTP POST multi-source (`/in/wazuh`, `/in/forgejo`, `/in/generic`).
- NATS JetStream bus (ALERTS, MONITOR, SYSTEM, CRON, AGENTS, USER_IN).
- Dispatcher avec dedup + rate limit + severity routing.
- Sinks email SMTP, ntfy push, Telegram via Apprise.
- Bot `@hubmqbot` polling + bridge msg-relay.
- Systemd service hardened + LoadCredential.

[1.0.0]: http://localhost:3000/motreffs/hubmq/commits/v1.0.0
[0.2.0]: http://localhost:3000/motreffs/hubmq/commits/v0.2.0
[0.1.0]: http://localhost:3000/motreffs/hubmq/commits/v0.1.0
