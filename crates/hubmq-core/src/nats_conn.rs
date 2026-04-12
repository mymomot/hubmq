/// Wrapper de connexion NATS JetStream avec bootstrap des streams.
///
/// D2 — chaque stream est créé avec des limites explicites (max_age, max_messages,
/// max_bytes, discard=Old, storage=File). Aucune valeur par défaut NATS n'est acceptée.
use crate::config::NatsConfig;
use anyhow::Context;
use async_nats::jetstream::{self, stream::Config as StreamConfig};
use std::time::Duration;

/// Handle vivant vers le serveur NATS avec accès JetStream.
pub struct NatsConn {
    /// Client NATS sous-jacent — utile pour publier hors JetStream.
    pub client: async_nats::Client,
    /// Contexte JetStream pour publier, consommer et gérer les streams.
    pub js: jetstream::Context,
}

impl NatsConn {
    /// Établit la connexion NATS via authentification NKey.
    ///
    /// Lit le fichier seed pointé par `cfg.nkey_seed_path`, extrait la ligne seed
    /// (commence par 'S', 58 caractères), et crée la connexion avec le nom
    /// `"hubmq-service"`.
    ///
    /// # Erreurs
    /// - Fichier seed illisible
    /// - Aucune ligne seed valide trouvée
    /// - Échec de connexion NATS
    pub async fn connect(cfg: &NatsConfig) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(&cfg.nkey_seed_path)
            .with_context(|| format!("lecture seed nkey {:?}", cfg.nkey_seed_path))?;
        let seed = extract_seed(&raw)?;

        let client = async_nats::ConnectOptions::with_nkey(seed)
            .name("hubmq-service")
            .connect(&cfg.url)
            .await
            .context("connexion NATS")?;

        let js = jetstream::new(client.clone());
        Ok(Self { client, js })
    }

    /// D2 — crée ou confirme les 6 streams JetStream avec limites explicites.
    ///
    /// Utilise `get_or_create_stream` — idempotent si le stream existe déjà avec
    /// la même configuration. Toute divergence de config déclenchera une erreur NATS.
    ///
    /// # Streams créés
    ///
    /// | Nom      | Subjects        | max_age  | max_msgs | max_bytes |
    /// |----------|-----------------|----------|----------|-----------|
    /// | ALERTS   | alert.>         | 7 jours  | 50 000   | 500 MB    |
    /// | MONITOR  | monitor.>       | 1 jour   | 10 000   | 100 MB    |
    /// | AGENTS   | agent.>         | 7 jours  | 10 000   | 200 MB    |
    /// | SYSTEM   | system.>        | 3 jours  | 5 000    | 100 MB    |
    /// | CRON     | cron.>          | 1 jour   | 5 000    | 50 MB     |
    /// | USER_IN  | user.incoming.> | 30 jours | 10 000   | 500 MB    |
    pub async fn ensure_streams(&self) -> anyhow::Result<()> {
        // (nom, subjects, max_age_secs, max_messages, max_bytes)
        let streams: &[(&str, &[&str], u64, i64, i64)] = &[
            ("ALERTS",  &["alert.>"],          7 * 86400,  50_000, 500 * 1024 * 1024),
            ("MONITOR", &["monitor.>"],             86400,  10_000, 100 * 1024 * 1024),
            ("AGENTS",  &["agent.>"],          7 * 86400,  10_000, 200 * 1024 * 1024),
            ("SYSTEM",  &["system.>"],         3 * 86400,   5_000, 100 * 1024 * 1024),
            ("CRON",    &["cron.>"],               86400,   5_000,  50 * 1024 * 1024),
            ("USER_IN", &["user.incoming.>"], 30 * 86400,  10_000, 500 * 1024 * 1024),
        ];

        for &(name, subjects, max_age_secs, max_messages, max_bytes) in streams {
            let cfg = StreamConfig {
                name: name.to_string(),
                subjects: subjects.iter().map(|s| s.to_string()).collect(),
                max_age: Duration::from_secs(max_age_secs),
                max_messages,
                max_bytes,
                discard: jetstream::stream::DiscardPolicy::Old,
                storage: jetstream::stream::StorageType::File,
                ..Default::default()
            };

            self.js
                .get_or_create_stream(cfg)
                .await
                .with_context(|| format!("bootstrap stream {}", name))?;

            tracing::info!(stream = name, "stream prêt");
        }

        Ok(())
    }
}

/// Extrait la ligne seed NKey depuis le contenu d'un fichier seed nsc.
///
/// Le format nsc produit des fichiers contenant la seed sur une ligne commençant
/// par 'S' et faisant exactement 58 caractères.
///
/// # Erreurs
/// Retourne une erreur si aucune ligne correspondante n'est trouvée.
fn extract_seed(file_content: &str) -> anyhow::Result<String> {
    for line in file_content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('S') && trimmed.len() == 58 {
            return Ok(trimmed.to_string());
        }
    }
    anyhow::bail!("aucune seed nkey trouvée dans le fichier (ligne commençant par 'S', 58 chars)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_seed_trouves_ligne_valide() {
        // Seed NKey de test — 58 chars (format nsc : commence par 'S', base32)
        let seed_line = "SUACSSL3UNAYNZY5BNRHROFHAYFM4ZFBX4PIJAROV3CAMZQXE3NL4CXCAA";
        assert_eq!(seed_line.len(), 58);

        let content = format!(
            "# fichier nsc généré\n# commentaire\n\n{}\n",
            seed_line
        );
        let result = extract_seed(&content).unwrap();
        assert_eq!(result, seed_line);
    }

    #[test]
    fn extract_seed_rejette_fichier_sans_seed() {
        let content = "# pas de seed ici\nAutre: valeur\n";
        assert!(extract_seed(content).is_err());
    }

    #[test]
    fn extract_seed_rejette_ligne_trop_courte() {
        let content = "SXXXSHORT\n";
        assert!(extract_seed(content).is_err());
    }

    #[test]
    fn extract_seed_rejette_ligne_trop_longue() {
        // 59 chars commençant par S
        let content = format!("{}\n", "S".to_string() + &"A".repeat(58));
        assert!(extract_seed(content.as_str()).is_err());
    }
}
