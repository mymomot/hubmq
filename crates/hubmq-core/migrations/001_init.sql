-- HubMQ — initialisation schéma persistent (D4)

CREATE TABLE IF NOT EXISTS outbox (
    id           TEXT PRIMARY KEY,               -- message uuid
    ts           TEXT NOT NULL,                  -- ISO8601
    source       TEXT NOT NULL,
    severity     TEXT NOT NULL,
    title        TEXT NOT NULL,
    body         TEXT NOT NULL,
    dedup_hash   TEXT NOT NULL,
    status       TEXT NOT NULL DEFAULT 'pending', -- pending, delivered, failed
    retries      INTEGER NOT NULL DEFAULT 0,
    last_error   TEXT,
    created_at   TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    delivered_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_outbox_status    ON outbox(status);
CREATE INDEX IF NOT EXISTS idx_outbox_dedup_ts  ON outbox(dedup_hash, ts);

-- D4 — journal d'audit structuré (FIM-watched)
CREATE TABLE IF NOT EXISTS audit (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    ts           TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    event        TEXT NOT NULL,       -- ex. 'command_dispatched', 'message_delivered', 'auth_reject'
    actor        TEXT,                -- chat_id, source, etc.
    target       TEXT,                -- command verb, subject, etc.
    payload_json TEXT                 -- champs structurés additionnels
);

CREATE INDEX IF NOT EXISTS idx_audit_event_ts ON audit(event, ts);
