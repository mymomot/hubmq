# CLAUDE-HUBMQ.md — HubMQ

## Statut projet

- **Phase** : Phase Core (implémentation, 5j estimés)
- **LXC** : 415 — `192.168.10.15` — Debian 13 — 2 vCPU / 2 GB RAM / 20 GB disk
- **SSH** : `ssh hubmq` (motreffs, sudo NOPASSWD)
- **Plan** : `docs/plans/2026-04-12-hubmq-phase-core.md` (23 tasks, TDD)

## Stack

| Composant | Détail | Port |
|---|---|---|
| NATS JetStream | v2.10.24, binary `/usr/local/bin/nats-server` | :4222 |
| hubmq (Rust Axum) | à implémenter — Phase Core | TBD |
| ntfy | notifications push | TBD |
| Apprise | multi-channel (email + Telegram) | — |
| Telegram bot | bidirectionnel (polling Phase Core, webhook Phase Exposure) | — |

## NATS — État Task 3 DONE (2026-04-12)

- Config : `/etc/nats/nats-server.conf`
- JetStream store : `/var/lib/nats/jetstream`
- Logfile : `/var/log/nats-server.log` (owner nats:nats)
- Service systemd : `nats.service` (enabled, active)
- NKeys : `/etc/nats/nkeys/` (chmod 700, owner nats:nats)
  - `hubmq-service.seed` (chmod 600) + `hubmq-service.pub` — daemon full access
  - `publisher.seed` (chmod 600) + `publisher.pub` — publishers LAN (Wazuh, Forgejo, cron)
- UFW : port 4222 ouvert pour `192.168.10.0/24`

### Comptes NATS

| NKey (pubkey prefix) | Rôle | Publish | Subscribe |
|---|---|---|---|
| UA2TOSEZKBE... (publisher) | Sources externes | alert.>, monitor.>, agent.>, system.>, cron.>, user.incoming.> | _INBOX.> |
| UABD7LP5U2W... (hubmq-service) | Daemon interne | > | > |

## Sécurité

- Wazuh agent 010 Active, groupe FIM `hubmq`
- SMTP Gmail : `/etc/hubmq/credentials/gmail-app-password` (chmod 600, owner hubmq ou root)
- SSH : port standard (LAN), motreffs only
- UFW : SSH LAN + NATS LAN uniquement

## Fichiers deploy/

| Fichier | Description |
|---|---|
| `nats-server.conf.template` | Template config avec `${PUB_NKEY}` / `${HUBMQ_NKEY}` |
| `nats.service` | Unit systemd NATS |
| `README-nkeys.md` | Doc NKeys : storage, permissions, rotation |

## Phasage

| Phase | Durée | Scope | État |
|---|---|---|---|
| Phase Core | 5j | LAN only, Telegram polling, fallback email direct | EN COURS (Task 6/23 DONE) |
| Phase Exposure | 2j | ntfy public, Telegram webhook Internet | À VENIR |

## Commits

| Hash | Message |
|---|---|
| `9bafca6` | init: hubmq workspace skeleton + Phase Core plan |
| `1a2d41a` | feat(nats): JetStream config template + systemd unit + NKeys doc |
| `19a6bf9` | feat(core): config loader TOML avec defaults + 2 tests TDD |
| `a55abda` | feat(core): canonical Message + Severity enum with Wazuh mapping |

## Tasks Phase Core — Suivi

- [x] Task 1 : LXC 415 provisionné (Proxmox)
- [x] Task 2 : NATS + nsc installés
- [x] Task 3 : NKeys générées, config finale, NATS LIVE
- [ ] Task 4 : Cargo workspace skeleton (hubmq-core + hubmq-bin stubs)
- [x] Task 5 : `config.rs` TOML loader — sections nats/smtp/telegram/filter/fallback/bridge/ntfy, defaults credentials par nom, `Config::from_file()`, 2 tests TDD PASS, clippy 0 warning (commit `19a6bf9`)
- [x] Task 6 : `message.rs` — `Severity` enum (P0-P3) + `from_wazuh_level()` (≥12=P0, 8-11=P1, 5-7=P2, <5=P3) + `bypasses_quiet_hours()` (P0+P1=true) + `Message` struct (id/ts/source/severity/title/body/tags/dedup_key/meta) + `Message::new()` + `dedup_hash()` SHA256. 3 tests TDD PASS, clippy 0 warning (commit `a55abda`)
- [ ] Task 7→23 : implémentation Rust hubmq (voir plan)
