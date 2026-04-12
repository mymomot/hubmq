/// Pont bidirectionnel vers msg-relay (optionnel — désactivé si `msg_relay_url` absent).
///
/// B3 — avant tout forward vers msg-relay, le premier token du corps du message
/// doit figurer dans la whitelist de commandes configurée. Tout autre contenu
/// est rejeté silencieusement (audit log).
use crate::{app_state::AppState, config::BridgeConfig, message::Message};
use anyhow::Context;
use reqwest::Client;

/// Client de pont vers msg-relay.
///
/// Construit depuis la section `[bridge]` de la configuration.
/// Si `url` est `None`, toutes les opérations retournent `Ok(false)` sans I/O.
pub struct Bridge {
    client: Client,
    /// URL de base du service msg-relay (ex. `http://localhost:9480`).
    url: Option<String>,
    /// Liste des commandes (premier token) acceptées.
    whitelist: Vec<String>,
}

impl Bridge {
    /// Construit un `Bridge` depuis la section `BridgeConfig`.
    pub fn new(cfg: &BridgeConfig) -> Self {
        Self {
            client: Client::new(),
            url: cfg.msg_relay_url.clone(),
            whitelist: cfg.command_whitelist.clone(),
        }
    }

    /// Tente de transmettre le message `m` à msg-relay.
    ///
    /// Retourne :
    /// - `Ok(true)`  — message transmis avec succès
    /// - `Ok(false)` — bridge désactivé (pas d'URL) ou commande hors whitelist
    /// - `Err(_)`    — erreur réseau ou réponse HTTP non-2xx
    ///
    /// # Effets de bord
    /// - Effectue un appel HTTP POST vers `{url}/send` si la commande est acceptée.
    /// - Enregistre des entrées d'audit dans les deux cas (rejet et succès).
    pub async fn forward_from_telegram(
        &self,
        state: &AppState,
        m: &Message,
    ) -> anyhow::Result<bool> {
        let Some(url) = &self.url else {
            tracing::debug!("bridge disabled (no msg_relay_url)");
            return Ok(false);
        };

        // B3 — extraire le premier token (verbe de commande)
        let verb = m
            .body
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_lowercase();

        if !self
            .whitelist
            .iter()
            .any(|w| w.eq_ignore_ascii_case(&verb))
        {
            state
                .audit
                .log(
                    "bridge_command_rejected",
                    m.meta.get("chat_id").map(|s| s.as_str()),
                    Some(&verb),
                    &serde_json::json!({"id": m.id.to_string()}),
                )
                .await
                .ok();
            return Ok(false);
        }

        // Forward vers msg-relay
        let body = serde_json::json!({
            "to":   "claude-code",
            "from": "hubmq-telegram",
            "body": m.body,
            "meta": m.meta,
        });

        let resp = self
            .client
            .post(format!("{}/send", url.trim_end_matches('/')))
            .json(&body)
            .send()
            .await
            .context("msg-relay HTTP POST")?;

        if !resp.status().is_success() {
            anyhow::bail!("msg-relay a retourné {}", resp.status());
        }

        state
            .audit
            .log(
                "bridge_command_dispatched",
                m.meta.get("chat_id").map(|s| s.as_str()),
                Some(&verb),
                &serde_json::json!({"id": m.id.to_string()}),
            )
            .await
            .ok();

        Ok(true)
    }
}
