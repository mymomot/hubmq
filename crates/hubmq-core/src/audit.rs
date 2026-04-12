/// Journal d'audit structuré (D4) — toutes les actions significatives sont tracées.
///
/// Les entrées sont immuables et horodatées par SQLite (`DEFAULT CURRENT_TIMESTAMP`).
/// Ce module est intentionnellement simple — pas de pagination ni de query complexe,
/// la lecture de l'audit est réservée au tableau de bord ou à l'export.
use anyhow::Context;
use serde::Serialize;
use sqlx::SqlitePool;

pub struct Audit {
    pool: SqlitePool,
}

impl Audit {
    /// Construit un `Audit` à partir d'un pool existant (partagé avec `Queue`).
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Enregistre un événement dans le journal d'audit.
    ///
    /// - `event`  : identifiant de l'événement (`"command_dispatched"`, `"auth_reject"`, …)
    /// - `actor`  : entité à l'origine de l'action (chat_id, source, …) — optionnel
    /// - `target` : cible de l'action (commande, sujet NATS, …) — optionnel
    /// - `payload`: données structurées sérialisées en JSON
    pub async fn log<P: Serialize>(
        &self,
        event: &str,
        actor: Option<&str>,
        target: Option<&str>,
        payload: &P,
    ) -> anyhow::Result<()> {
        let json = serde_json::to_string(payload).context("audit payload serialize")?;

        sqlx::query(
            "INSERT INTO audit(event, actor, target, payload_json) VALUES (?,?,?,?)",
        )
        .bind(event)
        .bind(actor)
        .bind(target)
        .bind(&json)
        .execute(&self.pool)
        .await
        .context("audit insert")?;

        tracing::info!(event, ?actor, ?target, payload = %json, "audit");

        Ok(())
    }
}
