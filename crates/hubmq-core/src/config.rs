use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Configuration principale chargée depuis un fichier TOML.
///
/// La section `[telegram]` est conservée pour la backward compatibilité —
/// elle est convertie en `BotEntry` "default" par `Config::into_registry()`.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub nats: NatsConfig,
    pub smtp: SmtpConfig,
    /// Section `[telegram]` monolithique — backward compat uniquement.
    /// Remplacée par conf.d/bots.toml en mode multi-bot.
    pub telegram: TelegramConfig,
    pub filter: FilterConfig,
    pub fallback: FallbackConfig,
    /// Section optionnelle — pont msg-relay pour commandes distantes.
    #[serde(default)]
    pub bridge: BridgeConfig,
    /// Section optionnelle — notifications ntfy.sh.
    #[serde(default)]
    pub ntfy: NtfyConfig,
    /// Section optionnelle — source email IMAP (polling Gmail).
    ///
    /// Si absente, le polling IMAP n'est pas démarré (backward compat).
    #[serde(default)]
    pub email_source: Option<EmailSourceConfig>,
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

/// Paramètres Telegram monolithiques — backward compat uniquement.
///
/// En mode multi-bot, le daemon utilise les `BotEntry` chargées depuis conf.d.
/// Cette section est convertie en `BotEntry` name="default" via `into_bot_entry()`.
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

impl TelegramConfig {
    /// Convertit la config monolithique en `BotEntry` "default" pour la backward compat.
    pub fn into_bot_entry(self) -> BotEntry {
        BotEntry {
            name: "default".into(),
            token_credential: self.token_credential,
            target_agent: "claude-hubmq".into(),
            allowed_chat_ids: self.allowed_chat_ids,
        }
    }
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
    /// Nom du credential secret pour l'authentification bearer vers msg-relay.
    #[serde(default)]
    pub bearer_credential: Option<String>,
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

/// Configuration de la source email IMAP (optionnelle).
///
/// Si absente ou `enabled = false`, le polling IMAP n'est pas démarré (backward compat).
///
/// En pratique, le credential IMAP réutilise le même `password_credential` que `[smtp]`
/// (même compte Gmail `mymomot74@gmail.com`).
///
/// Exemple dans config.toml :
/// ```toml
/// [email_source]
/// enabled = true
/// imap_host = "imap.gmail.com"
/// imap_port = 993
/// allowed_from = ["motreff@gmail.com"]
/// poll_interval_secs = 30
/// ```
#[derive(Debug, Clone, Deserialize)]
pub struct EmailSourceConfig {
    /// Active le polling IMAP (défaut : false).
    #[serde(default)]
    pub enabled: bool,
    /// Hôte IMAP TLS (défaut : `"imap.gmail.com"`).
    #[serde(default = "default_imap_host")]
    pub imap_host: String,
    /// Port IMAP TLS (défaut : 993).
    #[serde(default = "default_imap_port")]
    pub imap_port: u16,
    /// Adresses email autorisées à envoyer des commandes.
    ///
    /// Les emails dont le `From` ne contient aucune de ces adresses sont ignorés.
    #[serde(default)]
    pub allowed_from: Vec<String>,
    /// Intervalle entre deux polls IMAP en secondes (défaut : 30).
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,
}

fn default_imap_host() -> String {
    "imap.gmail.com".into()
}

fn default_imap_port() -> u16 {
    993
}

fn default_poll_interval() -> u64 {
    30
}

// ─── Multi-bot registry ───────────────────────────────────────────────────────

/// Entrée bot Telegram dans le registry multi-bot.
///
/// Correspond à une section `[[bot]]` dans conf.d/bots.toml.
#[derive(Debug, Clone, Deserialize, serde::Serialize, PartialEq)]
pub struct BotEntry {
    /// Identifiant unique du bot (`^[a-z0-9_-]+$`, max 64 chars).
    pub name: String,
    /// Nom du credential secret du token bot (résolu depuis /etc/hubmq/credentials/).
    pub token_credential: String,
    /// Nom de l'agent cible (doit exister dans `Registry.agents`).
    pub target_agent: String,
    /// Chat IDs autorisés pour ce bot.
    #[serde(default)]
    pub allowed_chat_ids: Vec<i64>,
}

/// Entrée agent dans le registry multi-bot.
///
/// Correspond à une section `[[agent]]` dans conf.d/agents.toml.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct AgentEntry {
    /// Identifiant unique de l'agent (`^[a-z0-9_-]+$`, max 64 chars).
    pub name: String,
    /// Type d'agent : `"claude-code-spawn"` | `"llm-openai-compat"`.
    pub kind: String,
    /// Subject NATS sur lequel le listener souscrit (ex : `"user.inbox.llmcore"`).
    #[serde(default)]
    pub listener_subject: Option<String>,
    /// URL de l'endpoint LLM (pour kind=`llm-openai-compat`).
    #[serde(default)]
    pub endpoint: Option<String>,
    /// Modèle LLM à utiliser (ex : `"qwen3.5-122b"`).
    #[serde(default)]
    pub model: Option<String>,
    /// Prompt système injecté dans chaque requête LLM.
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// Timeout en secondes pour les appels LLM (défaut : 60).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

/// Fichier TOML intermédiaire pour la désérialisation des tableaux conf.d.
///
/// Chaque fichier conf.d peut contenir `[[bot]]` et/ou `[[agent]]`.
#[derive(Debug, Default, Deserialize)]
struct RegistryFile {
    #[serde(default, rename = "bot")]
    bots: Vec<BotEntry>,
    #[serde(default, rename = "agent")]
    agents: Vec<AgentEntry>,
}

/// Registry multi-bot : ensemble des bots et agents chargés depuis conf.d.
#[derive(Debug, Clone, Default)]
pub struct Registry {
    pub bots: Vec<BotEntry>,
    pub agents: Vec<AgentEntry>,
}

impl Registry {
    /// Charge le registry depuis un répertoire conf.d.
    ///
    /// Glob `*.toml` dans `dir`, parse chaque fichier, merge les tableaux.
    /// Les doublons de `name` sont conservés en l'état — c'est une erreur de config.
    ///
    /// # Erreurs
    /// Retourne une erreur si `dir` n'est pas listable ou si un fichier TOML est invalide.
    pub fn load_conf_d(dir: &Path) -> anyhow::Result<Self> {
        let pattern = dir.join("*.toml");
        let pattern_str = pattern
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("chemin conf.d non-UTF8"))?;

        let mut registry = Registry::default();

        for entry in glob::glob(pattern_str)
            .map_err(|e| anyhow::anyhow!("glob conf.d invalide: {}", e))?
        {
            let path = entry.map_err(|e| anyhow::anyhow!("glob entrée conf.d: {}", e))?;
            let content = std::fs::read_to_string(&path)
                .map_err(|e| anyhow::anyhow!("lecture {:?}: {}", path, e))?;
            let file: RegistryFile = toml::from_str(&content)
                .map_err(|e| anyhow::anyhow!("TOML invalide {:?}: {}", path, e))?;
            registry.bots.extend(file.bots);
            registry.agents.extend(file.agents);
        }

        tracing::debug!(
            bots = registry.bots.len(),
            agents = registry.agents.len(),
            "conf.d chargé"
        );

        Ok(registry)
    }

    /// Valide les cross-références : chaque `bot.target_agent` doit exister dans `agents`.
    ///
    /// Retourne la liste des bots dont le target_agent est introuvable.
    pub fn validate_cross_refs(&self) -> Vec<String> {
        self.bots
            .iter()
            .filter(|b| !self.agents.iter().any(|a| a.name == b.target_agent))
            .map(|b| format!("bot '{}' → agent '{}' introuvable", b.name, b.target_agent))
            .collect()
    }

    /// Fusionne un registry supplémentaire dans celui-ci (extend).
    pub fn merge(&mut self, other: Registry) {
        self.bots.extend(other.bots);
        self.agents.extend(other.agents);
    }

    /// Recherche un agent par nom.
    pub fn find_agent(&self, name: &str) -> Option<&AgentEntry> {
        self.agents.iter().find(|a| a.name == name)
    }

    /// Recherche un bot par nom.
    pub fn find_bot(&self, name: &str) -> Option<&BotEntry> {
        self.bots.iter().find(|b| b.name == name)
    }
}

impl Config {
    /// Charge la configuration depuis un fichier TOML sur disque.
    ///
    /// # Erreurs
    /// Retourne une erreur si le fichier est illisible ou si le TOML est invalide.
    pub fn from_file(path: &Path) -> anyhow::Result<Self> {
        let s = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&s)?)
    }

    /// Convertit la config monolithique `[telegram]` en `BotEntry` "default"
    /// et retourne un `Registry` minimal pour la backward compat.
    pub fn into_registry(self) -> Registry {
        let bot = self.telegram.into_bot_entry();
        // Agent "claude-code-spawn" implicite pour backward compat.
        let agent = AgentEntry {
            name: "claude-hubmq".into(),
            kind: "claude-code-spawn".into(),
            listener_subject: Some("user.incoming.telegram".into()),
            endpoint: None,
            model: None,
            system_prompt: None,
            timeout_secs: None,
        };
        Registry {
            bots: vec![bot],
            agents: vec![agent],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    // ──── Tests backward compat (Config monolithique) ──────────────────────────

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

    // ──── Test backward compat : into_registry ─────────────────────────────────

    #[test]
    fn backward_compat_into_registry() {
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
allowed_chat_ids = [1451527482]
token_credential = "telegram-bot-token"

[filter]
dedup_window_secs = 60
rate_limit_per_min = 10
rate_limit_p0_per_min = 100
quiet_hours_start = "22:00"
quiet_hours_end = "07:00"

[fallback]
email_to = "a@b.c"
heartbeat_silence_max_secs = 3600
"#;
        let cfg: Config = toml::from_str(toml).unwrap();
        let reg = cfg.into_registry();
        assert_eq!(reg.bots.len(), 1);
        assert_eq!(reg.bots[0].name, "default");
        assert_eq!(reg.bots[0].token_credential, "telegram-bot-token");
        assert_eq!(reg.bots[0].target_agent, "claude-hubmq");
        assert_eq!(reg.bots[0].allowed_chat_ids, vec![1451527482]);
        assert_eq!(reg.agents.len(), 1);
        assert_eq!(reg.agents[0].kind, "claude-code-spawn");
    }

    // ──── Tests load_conf_d ────────────────────────────────────────────────────

    fn write_file(dir: &TempDir, name: &str, content: &str) {
        let path = dir.path().join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
    }

    #[test]
    fn load_conf_d_basic() {
        let dir = TempDir::new().unwrap();

        write_file(
            &dir,
            "agents.toml",
            r#"
[[agent]]
name = "llmcore"
kind = "llm-openai-compat"
endpoint = "http://192.168.10.118:8080/v1/chat/completions"
model = "qwen3.5-122b"
listener_subject = "user.inbox.llmcore"
timeout_secs = 120
"#,
        );

        write_file(
            &dir,
            "bots.toml",
            r#"
[[bot]]
name = "claude"
token_credential = "telegram-bot-token"
target_agent = "claude-hubmq"
allowed_chat_ids = [1451527482]
"#,
        );

        let reg = Registry::load_conf_d(dir.path()).unwrap();
        assert_eq!(reg.agents.len(), 1);
        assert_eq!(reg.agents[0].name, "llmcore");
        assert_eq!(reg.agents[0].kind, "llm-openai-compat");
        assert_eq!(reg.agents[0].timeout_secs, Some(120));
        assert_eq!(reg.bots.len(), 1);
        assert_eq!(reg.bots[0].name, "claude");
        assert_eq!(reg.bots[0].allowed_chat_ids, vec![1451527482]);
    }

    #[test]
    fn load_conf_d_merge_multiple_files() {
        let dir = TempDir::new().unwrap();

        write_file(
            &dir,
            "agents.toml",
            r#"
[[agent]]
name = "claude-hubmq"
kind = "claude-code-spawn"
listener_subject = "user.incoming.telegram"
"#,
        );

        write_file(
            &dir,
            "bots.toml",
            r#"
[[bot]]
name = "claude"
token_credential = "telegram-bot-token"
target_agent = "claude-hubmq"
allowed_chat_ids = [111]

[[bot]]
name = "llmcore"
token_credential = "telegram-bot-token-llmcore"
target_agent = "claude-hubmq"
allowed_chat_ids = [222]
"#,
        );

        let reg = Registry::load_conf_d(dir.path()).unwrap();
        assert_eq!(reg.bots.len(), 2);
        assert_eq!(reg.agents.len(), 1);
        // Les deux bots sont présents
        assert!(reg.find_bot("claude").is_some());
        assert!(reg.find_bot("llmcore").is_some());
    }

    #[test]
    fn load_conf_d_cross_ref_validation() {
        let dir = TempDir::new().unwrap();

        write_file(
            &dir,
            "agents.toml",
            r#"
[[agent]]
name = "claude-hubmq"
kind = "claude-code-spawn"
"#,
        );

        write_file(
            &dir,
            "bots.toml",
            r#"
[[bot]]
name = "good-bot"
token_credential = "tok1"
target_agent = "claude-hubmq"

[[bot]]
name = "bad-bot"
token_credential = "tok2"
target_agent = "agent-inexistant"
"#,
        );

        let reg = Registry::load_conf_d(dir.path()).unwrap();
        let errors = reg.validate_cross_refs();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("bad-bot"));
        assert!(errors[0].contains("agent-inexistant"));
    }

    #[test]
    fn load_conf_d_empty_directory() {
        let dir = TempDir::new().unwrap();
        let reg = Registry::load_conf_d(dir.path()).unwrap();
        assert!(reg.bots.is_empty());
        assert!(reg.agents.is_empty());
    }

    #[test]
    fn load_conf_d_ignores_non_toml() {
        let dir = TempDir::new().unwrap();
        // Fichier non-toml dans le répertoire — doit être ignoré par le glob
        write_file(&dir, "readme.txt", "ceci n'est pas un TOML");
        write_file(
            &dir,
            "agents.toml",
            r#"
[[agent]]
name = "test-agent"
kind = "claude-code-spawn"
"#,
        );
        let reg = Registry::load_conf_d(dir.path()).unwrap();
        assert_eq!(reg.agents.len(), 1);
    }

    #[test]
    fn registry_find_helpers() {
        let dir = TempDir::new().unwrap();
        write_file(
            &dir,
            "all.toml",
            r#"
[[agent]]
name = "my-agent"
kind = "llm-openai-compat"
endpoint = "http://example.com/v1/chat/completions"

[[bot]]
name = "my-bot"
token_credential = "cred"
target_agent = "my-agent"
"#,
        );
        let reg = Registry::load_conf_d(dir.path()).unwrap();
        assert!(reg.find_agent("my-agent").is_some());
        assert!(reg.find_agent("missing").is_none());
        assert!(reg.find_bot("my-bot").is_some());
        assert!(reg.find_bot("missing").is_none());
    }
}
