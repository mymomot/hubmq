/// Pont bidirectionnel vers msg-relay (optionnel — désactivé si `msg_relay_url` absent).
///
/// B3 — avant tout forward vers msg-relay, le premier token du corps du message
/// doit figurer dans la whitelist de commandes configurée, OU le chat_id expéditeur
/// doit figurer dans `cfg.telegram.allowed_chat_ids` (bypass explicite).
///
/// Garantie : le filtrage par `allowed_chat_ids` est déjà appliqué en amont dans
/// `source/telegram.rs` — ce bypass rend le bridge cohérent avec cette garantie
/// et permet à Stéphane d'envoyer des messages libres au bot.
use crate::{app_state::AppState, config::Config, message::Message};
use crate::config::BridgeConfig;
use anyhow::Context;
use reqwest::Client;

/// Résultat de la décision de routage du bridge.
///
/// `forward`  : le message doit être transmis à msg-relay
/// `bypassed` : `true` si l'autorisation provient du chat_id allowlist (pas de la whitelist verbe)
struct ForwardDecision {
    forward: bool,
    bypassed: bool,
}

/// Client de pont vers msg-relay.
///
/// Construit depuis la section `[bridge]` de la configuration.
/// Si `url` est `None`, toutes les opérations retournent `Ok(false)` sans I/O.
/// `Clone` est implémenté car `reqwest::Client` est cheaply cloneable (Arc interne).
#[derive(Clone)]
pub struct Bridge {
    client: Client,
    /// URL de base du service msg-relay (ex. `http://localhost:9480`).
    url: Option<String>,
    /// Liste des commandes (premier token) acceptées.
    whitelist: Vec<String>,
    /// Token bearer pour l'authentification vers msg-relay (optionnel).
    bearer: Option<String>,
}

impl Bridge {
    /// Construit un `Bridge` depuis la section `BridgeConfig`.
    ///
    /// `bearer` : valeur résolue du credential (pas le nom du fichier) — `None` si absent.
    pub fn new(cfg: &BridgeConfig, bearer: Option<String>) -> Self {
        Self {
            client: Client::new(),
            url: cfg.msg_relay_url.clone(),
            whitelist: cfg.command_whitelist.clone(),
            bearer,
        }
    }

    /// Décide si le message doit être forwardé, sans aucun I/O.
    ///
    /// Un message est accepté si :
    /// 1. Son premier token (verbe) figure dans la whitelist — règle B3, ou
    /// 2. Le `chat_id` expéditeur figure dans `cfg.telegram.allowed_chat_ids` — bypass explicite.
    ///
    /// Retourne `ForwardDecision { forward, bypassed }` :
    /// - `forward`  : `true` si le message doit passer
    /// - `bypassed` : `true` si c'est le chat_id qui a autorisé (pas la whitelist verbe)
    fn decide(&self, cfg: &Config, m: &Message) -> ForwardDecision {
        let verb = m
            .body
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_lowercase();

        let verb_allowed = self
            .whitelist
            .iter()
            .any(|w| w.eq_ignore_ascii_case(&verb));

        if verb_allowed {
            return ForwardDecision { forward: true, bypassed: false };
        }

        // Bypass : chat_id allowlisté → messages libres autorisés
        let chat_id_allowlisted = m
            .meta
            .get("chat_id")
            .and_then(|s| s.parse::<i64>().ok())
            .map(|id| cfg.telegram.allowed_chat_ids.contains(&id))
            .unwrap_or(false);

        ForwardDecision {
            forward: chat_id_allowlisted,
            bypassed: chat_id_allowlisted,
        }
    }

    /// Tente de transmettre le message `m` à msg-relay.
    ///
    /// Retourne :
    /// - `Ok(true)`  — message transmis avec succès
    /// - `Ok(false)` — bridge désactivé (pas d'URL) ou message rejeté (ni whitelist ni bypass)
    /// - `Err(_)`    — erreur réseau ou réponse HTTP non-2xx
    ///
    /// # Effets de bord
    /// - Effectue un appel HTTP POST vers `{url}/send` si le message est accepté.
    /// - Enregistre des entrées d'audit dans les deux cas (rejet et succès).
    ///   En cas de bypass chat_id, l'audit `bridge_command_dispatched` inclut
    ///   `"bypass": "chat_id_allowlisted"` pour traçabilité complète.
    pub async fn forward_from_telegram(
        &self,
        state: &AppState,
        m: &Message,
    ) -> anyhow::Result<bool> {
        let Some(url) = &self.url else {
            tracing::debug!("bridge disabled (no msg_relay_url)");
            return Ok(false);
        };

        let verb = m
            .body
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_lowercase();

        let decision = self.decide(&state.cfg, m);

        if !decision.forward {
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

        let mut req = self
            .client
            .post(format!("{}/send", url.trim_end_matches('/')))
            .json(&body);

        if let Some(token) = &self.bearer {
            req = req.bearer_auth(token);
        }

        let resp = req.send().await.context("msg-relay HTTP POST")?;

        if !resp.status().is_success() {
            anyhow::bail!("msg-relay a retourné {}", resp.status());
        }

        // Audit avec traçabilité du motif d'autorisation
        let audit_payload = if decision.bypassed {
            serde_json::json!({
                "id": m.id.to_string(),
                "bypass": "chat_id_allowlisted",
            })
        } else {
            serde_json::json!({"id": m.id.to_string()})
        };

        state
            .audit
            .log(
                "bridge_command_dispatched",
                m.meta.get("chat_id").map(|s| s.as_str()),
                Some(&verb),
                &audit_payload,
            )
            .await
            .ok();

        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        BridgeConfig, FallbackConfig, FilterConfig, NatsConfig, NtfyConfig, SmtpConfig,
        TelegramConfig,
    };
    use crate::message::Severity;
    use std::collections::BTreeMap;

    /// Construit une `Config` minimale pour les tests unitaires du bridge.
    fn make_config(allowed_chat_ids: Vec<i64>) -> Config {
        Config {
            nats: NatsConfig {
                url: "nats://localhost:4222".into(),
                nkey_seed_path: "/tmp/seed".into(),
            },
            smtp: SmtpConfig {
                host: "smtp.example.com".into(),
                port: 587,
                username: "u@x.y".into(),
                from: "u@x.y".into(),
                password_credential: "gmail-app-password".into(),
            },
            telegram: TelegramConfig {
                allowed_chat_ids,
                token_credential: "telegram-bot-token".into(),
            },
            filter: FilterConfig {
                dedup_window_secs: 60,
                rate_limit_per_min: 10,
                rate_limit_p0_per_min: 100,
                quiet_hours_start: "22:00".into(),
                quiet_hours_end: "07:00".into(),
            },
            fallback: FallbackConfig {
                email_to: "a@b.c".into(),
                heartbeat_silence_max_secs: 3600,
            },
            bridge: BridgeConfig::default(),
            ntfy: NtfyConfig::default(),
            email_source: None,
        }
    }

    /// Construit un `Bridge` avec la whitelist donnée (url factice — non utilisée dans decide()).
    fn make_bridge(whitelist: Vec<&str>) -> Bridge {
        Bridge {
            client: Client::new(),
            url: Some("http://localhost:9480".into()),
            whitelist: whitelist.into_iter().map(String::from).collect(),
            bearer: None,
        }
    }

    /// Construit un `Message` avec meta chat_id positionné.
    fn make_msg(body: &str, chat_id: Option<i64>) -> Message {
        let mut meta = BTreeMap::new();
        if let Some(id) = chat_id {
            meta.insert("chat_id".into(), id.to_string());
        }
        Message {
            id: uuid::Uuid::nil(),
            ts: chrono::Utc::now(),
            source: "telegram".into(),
            severity: Severity::P2,
            title: "test".into(),
            body: body.into(),
            tags: vec![],
            dedup_key: None,
            meta,
        }
    }

    // --- Tests B3 existants (whitelist verbe) ---

    #[test]
    fn whitelist_verb_non_allowlisted_chat_id_reject() {
        // Verbe hors whitelist + chat_id non allowlisté → rejet
        let bridge = make_bridge(vec!["status", "logs", "help"]);
        let cfg = make_config(vec![99999]);
        let m = make_msg("libre message quelconque", Some(11111));
        let d = bridge.decide(&cfg, &m);
        assert!(!d.forward, "doit être rejeté : verbe hors whitelist + chat_id non allowlisté");
    }

    #[test]
    fn whitelist_verb_match_non_allowlisted_chat_id_forward() {
        // Verbe dans whitelist + chat_id non allowlisté → accepté via whitelist
        let bridge = make_bridge(vec!["status", "logs", "help"]);
        let cfg = make_config(vec![99999]);
        let m = make_msg("status", Some(11111));
        let d = bridge.decide(&cfg, &m);
        assert!(d.forward, "doit être accepté : verbe dans whitelist");
        assert!(!d.bypassed, "pas de bypass : c'est la whitelist qui autorise");
    }

    #[test]
    fn whitelist_verb_case_insensitive() {
        // La whitelist est insensible à la casse (eq_ignore_ascii_case)
        let bridge = make_bridge(vec!["STATUS"]);
        let cfg = make_config(vec![]);
        let m = make_msg("status détail", None);
        let d = bridge.decide(&cfg, &m);
        assert!(d.forward);
        assert!(!d.bypassed);
    }

    // --- Nouveaux tests bypass chat_id ---

    #[test]
    fn chat_id_allowlisted_free_verb_forward() {
        // chat_id allowlisté + verbe hors whitelist → forward via bypass
        let bridge = make_bridge(vec!["status", "logs", "help"]);
        let cfg = make_config(vec![42]);
        let m = make_msg("bonjour Claude, peux-tu vérifier le service nexus ?", Some(42));
        let d = bridge.decide(&cfg, &m);
        assert!(d.forward, "doit être accepté : chat_id allowlisté");
        assert!(d.bypassed, "le bypass doit être signalé pour l'audit");
    }

    #[test]
    fn chat_id_not_allowlisted_free_verb_reject() {
        // chat_id NON allowlisté + verbe hors whitelist → rejet (régression)
        let bridge = make_bridge(vec!["status", "logs", "help"]);
        let cfg = make_config(vec![42]);
        let m = make_msg("bonjour Claude", Some(999));
        let d = bridge.decide(&cfg, &m);
        assert!(!d.forward, "doit être rejeté : chat_id non allowlisté");
        assert!(!d.bypassed);
    }

    #[test]
    fn no_chat_id_in_meta_reject() {
        // Pas de chat_id dans les meta + verbe hors whitelist → rejet (unwrap_or(false))
        let bridge = make_bridge(vec!["status"]);
        let cfg = make_config(vec![42]);
        let m = make_msg("message sans chat_id", None);
        let d = bridge.decide(&cfg, &m);
        assert!(!d.forward, "sans chat_id en meta, le bypass ne peut pas s'appliquer");
    }

    #[test]
    fn chat_id_allowlisted_whitelist_verb_forward_no_bypass() {
        // chat_id allowlisté + verbe dans whitelist → accepté par whitelist (bypass=false)
        // La whitelist prend la priorité — le bypass ne s'applique pas
        let bridge = make_bridge(vec!["status"]);
        let cfg = make_config(vec![42]);
        let m = make_msg("status", Some(42));
        let d = bridge.decide(&cfg, &m);
        assert!(d.forward);
        assert!(!d.bypassed, "whitelist verbe prend la priorité sur le bypass chat_id");
    }

    #[test]
    fn with_bearer_stores_token() {
        // Bridge construit avec un token bearer → self.bearer est bien renseigné
        let cfg_bridge = BridgeConfig {
            msg_relay_url: Some("http://192.168.10.99:9480".into()),
            command_whitelist: vec!["status".into()],
            bearer_credential: Some("msg-relay-token".into()),
        };
        let bridge = Bridge::new(&cfg_bridge, Some("secret-token-xyz".into()));
        assert_eq!(
            bridge.bearer.as_deref(),
            Some("secret-token-xyz"),
            "le token bearer doit être stocké dans self.bearer"
        );
    }

    #[test]
    fn without_bearer_stores_none() {
        // Bridge construit sans bearer → self.bearer est None
        let cfg_bridge = BridgeConfig {
            msg_relay_url: Some("http://192.168.10.99:9480".into()),
            command_whitelist: vec![],
            bearer_credential: None,
        };
        let bridge = Bridge::new(&cfg_bridge, None);
        assert!(bridge.bearer.is_none(), "sans credential configuré, bearer doit être None");
    }
}
