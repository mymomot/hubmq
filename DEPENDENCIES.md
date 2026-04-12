# Dépendances — HubMQ

> Généré le : 2026-04-12
> Commit ref : 19a6bf9

Workspace Cargo : `hubmq-bin` (daemon) + `hubmq-core` (library).
Rust 1.93.1 — resolver v2.

---

## Dépendances directes workspace

| Crate | Version | Scope | Rôle |
|---|---|---|---|
| **async-nats** | 0.38.0 | core | Client NATS JetStream (NKey auth) |
| **axum** | 0.7.9 | core | Serveur HTTP webhook ingestion |
| **tokio** | 1.51.1 | core + bin | Runtime async (full features) |
| **teloxide** | 0.13.0 | core | Bot Telegram polling |
| **sqlx** | 0.8.6 | core | SQLite WAL — queue persistante + audit |
| **lettre** | 0.11.21 | core | Client SMTP Gmail TLS |
| **reqwest** | 0.12.28 | core | HTTP client ntfy + external APIs |
| **serde** | 1.0.228 | core | Sérialisation/désérialisation |
| **serde_json** | 1.0.149 | core | JSON payloads NATS + API |
| **toml** | 0.8.23 | core | Parsing config.toml |
| **tera** | 1.20.1 | core | Templates email |
| **tracing** | 0.1.44 | core + bin | Logs structurés |
| **tracing-subscriber** | 0.3.23 | bin | Formatage logs (JSON + env-filter) |
| **thiserror** | 2.0.18 | core | Typage erreurs (pas de Box<dyn Error>) |
| **anyhow** | 1.0.102 | core + bin | Propagation erreurs contextuelles |
| **chrono** | 0.4.44 | core | Timestamps RFC3339 |
| **uuid** | 1.23.0 | core | ID messages v4 |
| **sha2** | 0.10.9 | core | Hash déduplication (SHA-256) |
| **async-trait** | 0.1.89 | core | Trait Sink async |
| **futures-util** | 0.3.32 | core | Streams async |
| **hubmq-core** | 0.1.0 (local) | bin | Dépendance interne |

## Dev-dépendances (hubmq-core)

| Crate | Version | Usage |
|---|---|---|
| **tempfile** | 3.27.0 | Tests intégration (fichiers config temporaires) |
| **tokio** | 1.51.1 | Runtime tests async |

---

## Arbre profondeur 2 (cargo tree --depth 2)

```
hubmq v0.1.0 (hubmq-bin)
├── anyhow v1.0.102
├── hubmq-core v0.1.0
│   ├── anyhow v1.0.102
│   ├── async-nats v0.38.0
│   ├── async-trait v0.1.89 (proc-macro)
│   ├── axum v0.7.9
│   ├── chrono v0.4.44
│   ├── futures-util v0.3.32
│   ├── lettre v0.11.21
│   ├── reqwest v0.12.28
│   ├── serde v1.0.228
│   ├── serde_json v1.0.149
│   ├── sha2 v0.10.9
│   ├── sqlx v0.8.6
│   ├── teloxide v0.13.0
│   ├── tera v1.20.1
│   ├── thiserror v2.0.18
│   ├── tokio v1.51.1
│   ├── toml v0.8.23
│   ├── tracing v0.1.44
│   └── uuid v1.23.0
│   [dev-dependencies]
│   ├── tempfile v3.27.0
│   └── tokio v1.51.1 (*)
├── tokio v1.51.1 (*)
├── tracing v0.1.44 (*)
└── tracing-subscriber v0.3.23
    ├── matchers v0.2.0
    ├── nu-ansi-term v0.50.3
    ├── once_cell v1.21.4
    ├── regex-automata v0.4.14
    ├── serde v1.0.228 (*)
    ├── serde_json v1.0.149 (*)
    ├── sharded-slab v0.1.7
    ├── smallvec v1.15.1
    ├── thread_local v1.1.9
    ├── tracing v0.1.44 (*)
    ├── tracing-core v0.1.36
    ├── tracing-log v0.2.0
    └── tracing-serde v0.2.0
```

---

## Notes

- Pas de dépendances hors workspace-deps (contrainte Task 5 respectée)
- `reqwest` v0.12 (nouveau) et `reqwest` v0.11 via `teloxide-core` — deux versions coexistent (duplication attendue, non critique)
- `sha2` présent deux fois dans l'arbre complet (hubmq-core direct + ed25519-dalek transitive via async-nats/nkeys) — versions identiques v0.10.9, cargo déduplique
- Arbre complet : 961 lignes (voir `cargo tree --workspace` pour le détail)
