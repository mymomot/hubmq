use serde::Deserialize;
use std::path::PathBuf;

/// Configuration principale chargée depuis un fichier TOML.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub nats: NatsConfig,
    pub smtp: SmtpConfig,
    pub telegram: TelegramConfig,
    pub filter: FilterConfig,
    pub fallback: FallbackConfig,
    /// Section optionnelle — pont msg-relay pour commandes distantes.
    #[serde(default)]
    pub bridge: BridgeConfig,
    /// Section optionnelle — notifications ntfy.sh.
    #[serde(default)]
    pub ntfy: NtfyConfig,
}

/// Connexion NATS JetStream.
#[derive(Debug, Clone, Deserialize)]
pub struct NatsConfig {
    /// URL du serveur NATS (ex : `nats://localhost:4222`).
    pub url: String,
    /// Chemin vers le fichier seed NKey du service.
    pub nkey_seed_path: PathBuf,
}

/// Paramètres SMTP pour l'envoi d'emails.
#[derive(Debug, Clone, Deserialize)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    /// Adresse expéditeur (header `From`).
    pub from: String,
    /// Nom du credential secret (résolu au runtime depuis le gestionnaire de secrets).
    /// Valeur par défaut : `"gmail-app-password"`.
    #[serde(default = "default_smtp_cred")]
    pub password_credential: String,
}

fn default_smtp_cred() -> String {
    "gmail-app-password".into()
}

/// Paramètres Telegram.
#[derive(Debug, Clone, Deserialize)]
pub struct TelegramConfig {
    /// Liste des chat IDs autorisés à envoyer des commandes.
    pub allowed_chat_ids: Vec<i64>,
    /// Nom du credential secret du token bot.
    /// Valeur par défaut : `"telegram-bot-token"`.
    #[serde(default = "default_tg_cred")]
    pub token_credential: String,
}

fn default_tg_cred() -> String {
    "telegram-bot-token".into()
}

/// Règles de filtrage et déduplication des messages.
#[derive(Debug, Clone, Deserialize)]
pub struct FilterConfig {
    /// Fenêtre de déduplication en secondes.
    pub dedup_window_secs: u64,
    /// Limite de messages par minute (toutes priorités).
    pub rate_limit_per_min: u32,
    /// Limite de messages par minute pour la priorité P0 (critique).
    pub rate_limit_p0_per_min: u32,
    /// Heure de début des heures silencieuses (format `"HH:MM"`).
    pub quiet_hours_start: String,
    /// Heure de fin des heures silencieuses (format `"HH:MM"`).
    pub quiet_hours_end: String,
}

/// Comportement de repli en cas d'absence de heartbeat ou de canal indisponible.
#[derive(Debug, Clone, Deserialize)]
pub struct FallbackConfig {
    /// Adresse email de destination pour les alertes de repli.
    pub email_to: String,
    /// Durée maximale de silence heartbeat avant alerte (en secondes).
    pub heartbeat_silence_max_secs: u64,
}

/// Pont bidirectionnel via msg-relay (optionnel).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct BridgeConfig {
    /// URL du service msg-relay (ex : `http://localhost:9480`).
    #[serde(default)]
    pub msg_relay_url: Option<String>,
    /// Liste blanche des commandes acceptées depuis le pont.
    #[serde(default)]
    pub command_whitelist: Vec<String>,
}

/// Notifications ntfy.sh (optionnel).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct NtfyConfig {
    /// URL de base du serveur ntfy (ex : `https://ntfy.sh`).
    #[serde(default)]
    pub base_url: Option<String>,
    /// Topic ntfy cible.
    #[serde(default)]
    pub topic: Option<String>,
    /// Nom du credential secret pour l'authentification bearer.
    #[serde(default)]
    pub bearer_credential: Option<String>,
}

impl Config {
    /// Charge la configuration depuis un fichier TOML sur disque.
    ///
    /// # Erreurs
    /// Retourne une erreur si le fichier est illisible ou si le TOML est invalide.
    pub fn from_file(path: &std::path::Path) -> anyhow::Result<Self> {
        let s = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&s)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_config() {
        let toml = r#"
[nats]
url = "nats://localhost:4222"
nkey_seed_path = "/etc/nats/nkeys/hubmq-service.seed"

[smtp]
host = "smtp.gmail.com"
port = 587
username = "mymomot74@gmail.com"
from = "HubMQ <mymomot74@gmail.com>"

[telegram]
allowed_chat_ids = [12345]

[filter]
dedup_window_secs = 60
rate_limit_per_min = 10
rate_limit_p0_per_min = 100
quiet_hours_start = "22:00"
quiet_hours_end = "07:00"

[fallback]
email_to = "motreff@gmail.com"
heartbeat_silence_max_secs = 10800
"#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert_eq!(cfg.nats.url, "nats://localhost:4222");
        assert_eq!(cfg.filter.dedup_window_secs, 60);
        assert_eq!(cfg.filter.rate_limit_p0_per_min, 100);
        assert_eq!(cfg.telegram.allowed_chat_ids, vec![12345]);
        assert_eq!(cfg.smtp.password_credential, "gmail-app-password"); // defaut
        assert_eq!(cfg.telegram.token_credential, "telegram-bot-token"); // defaut
    }

    #[test]
    fn optional_sections_default() {
        let toml = r#"
[nats]
url = "nats://localhost:4222"
nkey_seed_path = "/tmp/seed"

[smtp]
host = "smtp.gmail.com"
port = 587
username = "u@x.y"
from = "u@x.y"

[telegram]
allowed_chat_ids = []

[filter]
dedup_window_secs = 30
rate_limit_per_min = 5
rate_limit_p0_per_min = 50
quiet_hours_start = "23:00"
quiet_hours_end = "06:00"

[fallback]
email_to = "a@b.c"
heartbeat_silence_max_secs = 3600
"#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert!(cfg.bridge.msg_relay_url.is_none());
        assert!(cfg.bridge.command_whitelist.is_empty());
        assert!(cfg.ntfy.base_url.is_none());
    }
}
