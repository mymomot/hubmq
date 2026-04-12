/// File d'attente persistante SQLite pour les messages HubMQ.
///
/// Utilise le mode WAL pour améliorer la concurrence lecteurs/écriture.
/// La déduplication est assurée par `dedup_hash` + fenêtre temporelle.
use crate::message::Message;
use anyhow::Context;
use sqlx::SqlitePool;
use std::path::Path;

pub struct Queue {
    pool: SqlitePool,
}

impl Queue {
    /// Ouvre (ou crée) la base SQLite à `path`, active WAL et applique les migrations.
    pub async fn open(path: &Path) -> anyhow::Result<Self> {
        let url = format!("sqlite://{}?mode=rwc", path.display());
        let pool = SqlitePool::connect(&url)
            .await
            .context("sqlite connect")?;
        sqlx::query("PRAGMA journal_mode=WAL;")
            .execute(&pool)
            .await
            .context("pragma WAL")?;
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .context("migrations")?;
        Ok(Self { pool })
    }

    /// Insère un message dans la file (`INSERT OR IGNORE` — idempotent sur l'id).
    pub async fn enqueue(&self, m: &Message) -> anyhow::Result<()> {
        let severity = serde_json::to_string(&m.severity)
            .context("severity serialize")?;
        // serde produit "\"P1\"" — on retire les guillemets pour stocker "P1"
        let severity = severity.trim_matches('"').to_owned();

        sqlx::query(
            "INSERT OR IGNORE INTO outbox(id, ts, source, severity, title, body, dedup_hash, status) \
             VALUES (?,?,?,?,?,?,?,'pending')",
        )
        .bind(m.id.to_string())
        .bind(m.ts.to_rfc3339())
        .bind(&m.source)
        .bind(&severity)
        .bind(&m.title)
        .bind(&m.body)
        .bind(m.dedup_hash())
        .execute(&self.pool)
        .await
        .context("enqueue insert")?;

        Ok(())
    }

    /// Marque un message comme livré et enregistre l'horodatage.
    pub async fn mark_delivered(&self, id: &str) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE outbox SET status='delivered', delivered_at=CURRENT_TIMESTAMP WHERE id=?",
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .context("mark_delivered")?;

        Ok(())
    }

    /// Retourne `true` si un message avec le même `dedup_hash` a été enqueué
    /// dans les `window_secs` dernières secondes.
    pub async fn is_duplicate_within(
        &self,
        dedup_hash: &str,
        window_secs: u64,
    ) -> anyhow::Result<bool> {
        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM outbox \
             WHERE dedup_hash = ? AND datetime(ts) > datetime('now', ?)",
        )
        .bind(dedup_hash)
        .bind(format!("-{} seconds", window_secs))
        .fetch_one(&self.pool)
        .await
        .context("is_duplicate_within")?;

        Ok(row.0 > 0)
    }

    /// Retourne un clone du pool — nécessaire pour partager la connexion avec `Audit`.
    pub fn pool_clone(&self) -> SqlitePool {
        self.pool.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{Message, Severity};

    /// Crée une base temporaire et retourne le couple (Queue, NamedTempFile).
    /// Le `NamedTempFile` DOIT rester en vie pendant toute la durée du test
    /// — le drop supprime le fichier et fermerait la connexion SQLite.
    async fn temp_queue() -> (Queue, tempfile::NamedTempFile) {
        let f = tempfile::NamedTempFile::new().expect("tempfile");
        let q = Queue::open(f.path()).await.expect("Queue::open");
        (q, f)
    }

    #[tokio::test]
    async fn enqueue_and_dedup_detect() {
        let (q, _f) = temp_queue().await;
        let m = Message::new("test", Severity::P1, "title", "body");
        q.enqueue(&m).await.expect("enqueue");
        assert!(
            q.is_duplicate_within(&m.dedup_hash(), 60)
                .await
                .expect("is_duplicate_within"),
            "le message vient d'être inséré — doit être détecté comme doublon dans 60s"
        );
    }

    #[tokio::test]
    async fn dedup_window_expires() {
        let (q, _f) = temp_queue().await;
        // Insère un enregistrement avec un timestamp vieux de 120 secondes
        sqlx::query(
            "INSERT INTO outbox(id, ts, source, severity, title, body, dedup_hash, status) \
             VALUES ('old', datetime('now','-120 seconds'), 'test', 'P2', 't', 'b', 'hashA', 'pending')",
        )
        .execute(&q.pool)
        .await
        .expect("insert old record");

        // Fenêtre 60s — l'enregistrement est en dehors → pas de doublon
        assert!(
            !q.is_duplicate_within("hashA", 60)
                .await
                .expect("is_duplicate_within 60"),
            "enregistrement vieux de 120s hors fenêtre 60s — ne doit pas être doublon"
        );

        // Fenêtre 180s — l'enregistrement est dedans → doublon détecté
        assert!(
            q.is_duplicate_within("hashA", 180)
                .await
                .expect("is_duplicate_within 180"),
            "enregistrement vieux de 120s dans fenêtre 180s — doit être doublon"
        );
    }
}
