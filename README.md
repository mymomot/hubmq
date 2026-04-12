# HubMQ — Unified Communication Hub for Homelab

Central notification and bidirectional communication service for the mymomot.ovh homelab.

**Status**: Phase Core (LAN only) — see `docs/plans/2026-04-12-hubmq-phase-core.md`

## Architecture

- NATS JetStream bus (local, port 4222)
- Rust Axum service (`hubmq`)
- ntfy.sh push (LAN only in Phase Core)
- Apprise email/Telegram delivery
- SQLite persistent queue + audit log

See `docs/ARCHITECTURE.md` and `docs/SUBJECTS.md`.
