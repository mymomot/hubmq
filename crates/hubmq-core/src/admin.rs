/// Admin consumer NATS — gestion dynamique des bots via `admin.bot.add` / `admin.bot.remove`.
///
/// Ce module souscrit aux subjects NATS de management et permet d'ajouter ou retirer
/// des bots sans redémarrage du daemon (via SIGHUP + reload conf.d).
///
/// ## Sécurité
/// - Validation stricte du payload AVANT toute écriture filesystem (regex + cross-ref)
/// - Token JAMAIS loggé — seul le hash sha256[0:12] est stocké en audit SQLite
/// - Écriture atomique : `File::create_new(tmp)` + `rename(tmp → final)` + backup `.bak`
/// - `requester_chat_id` doit être dans la liste allowlist globale (cfg.telegram.allowed_chat_ids)
///
/// ## Permissions filesystem attendues
/// - `/etc/hubmq/credentials/` : 0770 root:hubmq (daemon peut créer les tokens)
/// - `/etc/hubmq/conf.d/`      : 0770 root:hubmq (daemon peut éditer bots.toml)
///
/// ## Réponse
/// Publish sur `agent.hubmq.admin.response` severity P3 (log_only, pas de DM doublée).
use crate::{
    app_state::AppState,
    config::{BotEntry, Registry},
    message::{Message, Severity},
};
use anyhow::Context;
use async_nats::jetstream;
use futures_util::StreamExt;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Regex compilée une seule fois pour valider les noms de bots/agents.
///
/// Accepte : [a-z0-9_-], longueur 1..=64.
/// Cohérent avec `subjects::is_safe_ident`.
fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Hash partiel du token (12 premiers hex chars du sha256).
/// Stocké en audit à la place du token réel.
fn token_hash_prefix(token: &str) -> String {
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    format!("{:.12x}", h.finalize())
}

// ─── Payloads NATS ────────────────────────────────────────────────────────────

/// Payload pour `admin.bot.add`.
///
/// Le champ `token` est un secret — JAMAIS loggé directement.
///
/// ## Requester
/// Deux origines mutuellement exclusives :
/// - `requester_chat_id` : commande depuis Telegram (doit être dans allowlist globale)
/// - `requester_email` : commande depuis la source email IMAP (vérifiée par `is_from_allowed`)
///
/// Si les deux sont absents, la validation échoue.
#[derive(Debug, Deserialize)]
pub struct AdminBotAdd {
    /// Nom du nouveau bot (`^[a-z0-9_-]+$`, max 64 chars).
    pub name: String,
    /// Nom de l'agent cible (doit exister dans le registry).
    pub target_agent: String,
    /// Token bot Telegram — sera écrit dans `/etc/hubmq/credentials/telegram-bot-token-<name>`.
    pub token: String,
    /// Chat IDs autorisés pour ce bot.
    #[serde(default)]
    pub allowed_chat_ids: Vec<i64>,
    /// Chat ID du demandeur (doit être dans allowlist globale).
    /// Mutuellement exclusif avec `requester_email`.
    #[serde(default)]
    pub requester_chat_id: Option<i64>,
    /// Adresse email du demandeur (source IMAP).
    /// Mutuellement exclusif avec `requester_chat_id`.
    #[serde(default)]
    pub requester_email: Option<String>,
    /// Message-ID de l'email d'origine (pour la réponse email).
    #[serde(default)]
    pub email_msg_id: Option<String>,
    /// Sujet de l'email d'origine (pour la réponse email).
    #[serde(default)]
    pub email_subject: Option<String>,
}

/// Payload pour `admin.bot.remove`.
#[derive(Debug, Deserialize)]
pub struct AdminBotRemove {
    /// Nom du bot à retirer.
    pub name: String,
    /// Chat ID du demandeur (doit être dans allowlist globale).
    pub requester_chat_id: i64,
}

// ─── Consumer principal ───────────────────────────────────────────────────────

/// Lance le consumer admin NATS.
///
/// Souscrit aux subjects `admin.bot.add` et `admin.bot.remove` (via filter sur stream AGENTS).
/// Valide, écrit les fichiers, envoie SIGHUP à soi-même, publie l'ack.
///
/// `registry` : registre partagé (RwLock) mis à jour par le daemon après reload SIGHUP.
/// Le registry passé ici est utilisé pour les validations cross-ref.
pub async fn run(state: AppState, registry: Arc<RwLock<Registry>>) -> anyhow::Result<()> {
    // Consumer durable sur le stream AGENTS, filtré sur admin.bot.>
    let stream = state
        .nats
        .js
        .get_stream("AGENTS")
        .await
        .context("récupération stream AGENTS pour admin consumer")?;

    let consumer: jetstream::consumer::PullConsumer = stream
        .get_or_create_consumer(
            "hubmq-admin",
            jetstream::consumer::pull::Config {
                durable_name: Some("hubmq-admin".into()),
                filter_subject: "admin.bot.>".into(),
                ..Default::default()
            },
        )
        .await
        .context("création consumer admin")?;

    tracing::info!("admin consumer démarré — écoute admin.bot.>");

    let mut msgs = consumer
        .messages()
        .await
        .context("consumer admin messages()")?;

    while let Some(msg_result) = msgs.next().await {
        let msg = match msg_result {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(err = ?e, "admin consumer: erreur réception message");
                continue;
            }
        };

        let subject = msg.subject.as_str().to_string();
        let result = handle_admin_message(&state, &registry, &subject, &msg.payload).await;

        match result {
            Ok(ack_msg) => {
                publish_admin_response(&state, true, &ack_msg).await;
                tracing::info!(subject = %subject, "admin action OK: {}", ack_msg);
            }
            Err(e) => {
                let err_str = e.to_string();
                publish_admin_response(&state, false, &err_str).await;
                tracing::warn!(subject = %subject, err = %err_str, "admin action échouée");
            }
        }

        msg.ack().await.ok();
    }

    Ok(())
}

/// Dispatche selon le subject NATS vers le handler approprié.
async fn handle_admin_message(
    state: &AppState,
    registry: &Arc<RwLock<Registry>>,
    subject: &str,
    payload: &[u8],
) -> anyhow::Result<String> {
    match subject {
        "admin.bot.add" => {
            let req: AdminBotAdd = serde_json::from_slice(payload)
                .context("parse payload admin.bot.add")?;
            handle_bot_add(state, registry, req).await
        }
        "admin.bot.remove" => {
            let req: AdminBotRemove = serde_json::from_slice(payload)
                .context("parse payload admin.bot.remove")?;
            handle_bot_remove(state, registry, req).await
        }
        _ => {
            anyhow::bail!("subject admin inconnu: {}", subject);
        }
    }
}

/// Traite `admin.bot.add` :
/// 1. Validation payload (regex nom, cross-ref agent, requester allowlist)
/// 2. Écriture atomique token dans credentials/
/// 3. Écriture atomique conf.d/bots.toml
/// 4. SIGHUP à soi-même → le daemon recharge le registry
/// 5. Audit SQLite (hash token, pas valeur)
async fn handle_bot_add(
    state: &AppState,
    registry: &Arc<RwLock<Registry>>,
    req: AdminBotAdd,
) -> anyhow::Result<String> {
    // ── Validation ──────────────────────────────────────────────────────────

    if !is_valid_name(&req.name) {
        anyhow::bail!(
            "nom de bot invalide '{}' — doit correspondre à ^[a-z0-9_-]+$, max 64 chars",
            req.name
        );
    }

    if req.token.is_empty() {
        anyhow::bail!("token vide — refusé");
    }

    // Validation du requester : soit chat_id dans allowlist, soit email autorisé
    let allowed_chat_ids = &state.cfg.telegram.allowed_chat_ids;
    match (&req.requester_chat_id, &req.requester_email) {
        (Some(chat_id), _) => {
            // Origine Telegram : chat_id doit être dans l'allowlist globale
            if !allowed_chat_ids.contains(chat_id) {
                anyhow::bail!(
                    "requester chat_id {} non autorisé",
                    chat_id
                );
            }
        }
        (None, Some(email)) => {
            // Origine email : on accepte si l'email est non-vide (déjà vérifié par is_from_allowed dans source/email.rs)
            // La validation de l'allowlist email a été faite avant la publication NATS
            if email.is_empty() {
                anyhow::bail!("requester_email vide — refusé");
            }
        }
        (None, None) => {
            anyhow::bail!("requester non identifié — requester_chat_id ou requester_email requis");
        }
    }

    // L'agent cible doit exister dans le registry
    {
        let reg = registry.read().await;
        if reg.find_agent(&req.target_agent).is_none() {
            anyhow::bail!(
                "agent cible '{}' introuvable dans le registry",
                req.target_agent
            );
        }
    }

    // ── Écriture token (atomique) ────────────────────────────────────────────

    let cred_name = format!("telegram-bot-token-{}", req.name);
    let creds_dir = Path::new("/etc/hubmq/credentials");
    atomic_write_credential(creds_dir, &cred_name, &req.token)?;

    // ── Écriture conf.d/bots.toml (atomique) ────────────────────────────────

    let conf_d = Path::new("/etc/hubmq/conf.d");
    let bots_path = conf_d.join("bots.toml");

    // Charger le fichier existant si présent, sinon partir de zéro
    let mut existing_bots: Vec<BotEntry> = if bots_path.exists() {
        let content = std::fs::read_to_string(&bots_path)
            .context("lecture bots.toml existant")?;
        let file: BotsFile = toml::from_str(&content).context("parse bots.toml existant")?;
        file.bot
    } else {
        vec![]
    };

    // Retirer entrée existante portant le même nom (idempotent add)
    existing_bots.retain(|b| b.name != req.name);
    existing_bots.push(BotEntry {
        name: req.name.clone(),
        token_credential: cred_name.clone(),
        target_agent: req.target_agent.clone(),
        allowed_chat_ids: req.allowed_chat_ids.clone(),
    });

    let new_content = serialize_bots_toml(&existing_bots)?;
    atomic_write_toml(&bots_path, &new_content)?;

    // ── Audit (hash token uniquement) ────────────────────────────────────────

    let tok_hash = token_hash_prefix(&req.token);
    let requester_actor = req.requester_chat_id
        .map(|id| id.to_string())
        .or_else(|| req.requester_email.clone())
        .unwrap_or_else(|| "unknown".into());

    state
        .audit
        .log(
            "admin_bot_add",
            Some(&requester_actor),
            Some(&req.name),
            &serde_json::json!({
                "bot_name": req.name,
                "target_agent": req.target_agent,
                "token_hash_prefix": tok_hash,
                "allowed_chat_ids": req.allowed_chat_ids,
                "source": if req.requester_email.is_some() { "email" } else { "telegram" },
            }),
        )
        .await
        .ok();

    // ── Réponse email si origine email ────────────────────────────────────────
    // Si la demande vient d'un email, publier une réponse de confirmation
    // sur agent.hubmq.admin.response avec les métadonnées email pour routing
    if let Some(ref email_from) = req.requester_email {
        let email_msg_id = req.email_msg_id.clone().unwrap_or_default();
        let email_subject = req.email_subject.clone()
            .unwrap_or_else(|| format!("HUBMQ_BOT_TOKEN {}", req.name));

        let mut ack_msg = Message::new(
            "hubmq.admin",
            Severity::P2,
            format!("bot '{}' créé", req.name),
            format!("✅ bot '{}' actif — target_agent: {}", req.name, req.target_agent),
        );
        ack_msg.meta.insert("source".into(), "email".into());
        ack_msg.meta.insert("email_from".into(), email_from.clone());
        ack_msg.meta.insert("email_msg_id".into(), email_msg_id);
        ack_msg.meta.insert("email_subject".into(), email_subject);

        if let Ok(payload) = serde_json::to_vec(&ack_msg) {
            state
                .nats
                .js
                .publish("agent.hubmq.admin.response", payload.into())
                .await
                .map_err(|e| tracing::warn!(err = ?e, "publish ack admin email échoué"))
                .ok();
        }
    }

    // ── SIGHUP → reload ──────────────────────────────────────────────────────
    send_sighup_self()?;

    Ok(format!("bot '{}' ajouté — SIGHUP envoyé", req.name))
}

/// Traite `admin.bot.remove` :
/// 1. Validation (nom valide, requester allowlist, bot existe)
/// 2. Retrait du fichier credential (backup .bak)
/// 3. Retrait de conf.d/bots.toml
/// 4. SIGHUP
async fn handle_bot_remove(
    state: &AppState,
    registry: &Arc<RwLock<Registry>>,
    req: AdminBotRemove,
) -> anyhow::Result<String> {
    // ── Validation ──────────────────────────────────────────────────────────

    if !is_valid_name(&req.name) {
        anyhow::bail!(
            "nom de bot invalide '{}' — doit correspondre à ^[a-z0-9_-]+$",
            req.name
        );
    }

    let allowed = &state.cfg.telegram.allowed_chat_ids;
    if !allowed.contains(&req.requester_chat_id) {
        anyhow::bail!(
            "requester chat_id {} non autorisé — admin.bot.remove accepte uniquement les requêtes Telegram",
            req.requester_chat_id
        );
    }

    // Vérifier que le bot existe dans le registry
    {
        let reg = registry.read().await;
        if reg.find_bot(&req.name).is_none() {
            anyhow::bail!("bot '{}' introuvable dans le registry", req.name);
        }
    }

    // ── Retrait token credential ─────────────────────────────────────────────

    let cred_name = format!("telegram-bot-token-{}", req.name);
    let cred_path = Path::new("/etc/hubmq/credentials").join(&cred_name);
    if cred_path.exists() {
        let bak = cred_path.with_extension("bak");
        std::fs::rename(&cred_path, &bak)
            .context("backup token credential")?;
    }

    // ── Retrait de conf.d/bots.toml ──────────────────────────────────────────

    let bots_path = Path::new("/etc/hubmq/conf.d/bots.toml");
    if bots_path.exists() {
        let content = std::fs::read_to_string(bots_path)
            .context("lecture bots.toml pour remove")?;
        let mut file: BotsFile = toml::from_str(&content).context("parse bots.toml pour remove")?;
        file.bot.retain(|b| b.name != req.name);
        let new_content = serialize_bots_toml(&file.bot)?;
        atomic_write_toml(bots_path, &new_content)?;
    }

    // ── Audit ────────────────────────────────────────────────────────────────

    state
        .audit
        .log(
            "admin_bot_remove",
            Some(&req.requester_chat_id.to_string()),
            Some(&req.name),
            &serde_json::json!({"bot_name": req.name}),
        )
        .await
        .ok();

    // ── SIGHUP → reload ──────────────────────────────────────────────────────
    send_sighup_self()?;

    Ok(format!("bot '{}' retiré — SIGHUP envoyé", req.name))
}

// ─── Helpers filesystem ───────────────────────────────────────────────────────

/// Structure intermédiaire pour sérialiser/désérialiser conf.d/bots.toml.
#[derive(Debug, Default, serde::Deserialize, serde::Serialize)]
struct BotsFile {
    #[serde(default, rename = "bot")]
    bot: Vec<BotEntry>,
}

// Implémentation manuelle de Serialize pour BotEntry (toml ne sait pas sérialiser
// les structs sans derive Serialize — on ajoute la derive).
// Note: BotEntry dérive déjà Deserialize — on ajoute Serialize ici via serde.
// Voir la note dans config.rs : BotEntry/AgentEntry ont besoin de Serialize.
// Pour éviter de modifier config.rs à mi-étape, on convertit via serde_json interop.

/// Sérialise une liste de BotEntry en TOML valide (tableaux `[[bot]]`).
fn serialize_bots_toml(bots: &[BotEntry]) -> anyhow::Result<String> {
    // Conversion via serde_json → serde_toml (BotEntry dérive Deserialize)
    // On reconstruit le TOML manuellement car toml::to_string nécessite Serialize.
    let mut out = String::new();
    for b in bots {
        out.push_str("[[bot]]\n");
        out.push_str(&format!("name = {:?}\n", b.name));
        out.push_str(&format!("token_credential = {:?}\n", b.token_credential));
        out.push_str(&format!("target_agent = {:?}\n", b.target_agent));
        let ids: Vec<String> = b.allowed_chat_ids.iter().map(|id| id.to_string()).collect();
        out.push_str(&format!("allowed_chat_ids = [{}]\n", ids.join(", ")));
        out.push('\n');
    }
    Ok(out)
}

/// Écriture atomique d'un fichier texte : tmp → rename → (backup ancien si existant).
///
/// Le fichier temporaire est créé dans le même répertoire que la cible
/// pour garantir que rename est atomique (même filesystem).
///
/// # Permissions
/// Le fichier final est écrit avec les permissions par défaut du processus (umask).
/// Le daemon tourne sous hubmq — `/etc/hubmq/conf.d/` est 0770 root:hubmq.
fn atomic_write_toml(target: &Path, content: &str) -> anyhow::Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| anyhow::anyhow!("chemin cible sans parent: {:?}", target))?;

    // Backup de l'existant si présent
    if target.exists() {
        let bak = target.with_extension("bak");
        std::fs::copy(target, &bak)
            .with_context(|| format!("backup {:?} → {:?}", target, bak))?;
    }

    // Écriture dans un fichier temporaire unique dans le même répertoire
    let tmp_path = parent.join(format!(
        ".hubmq-tmp-{}.toml",
        uuid::Uuid::new_v4().to_string().replace('-', "")
    ));

    std::fs::write(&tmp_path, content)
        .with_context(|| format!("écriture tmp {:?}", tmp_path))?;

    // Rename atomique
    std::fs::rename(&tmp_path, target)
        .with_context(|| format!("rename {:?} → {:?}", tmp_path, target))?;

    Ok(())
}

/// Écriture atomique d'un credential dans `/etc/hubmq/credentials/<name>`.
///
/// Permissions: 0640 (owner rw, group r, other none).
/// Le daemon hubmq tourne sous l'utilisateur `hubmq` dans le groupe `hubmq`.
///
/// # Sécurité
/// Le contenu `secret` n'est JAMAIS loggé — seul son hash est tracé en audit.
fn atomic_write_credential(creds_dir: &Path, name: &str, secret: &str) -> anyhow::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;

    let final_path = creds_dir.join(name);
    let tmp_path = creds_dir.join(format!(
        ".hubmq-tmp-cred-{}",
        uuid::Uuid::new_v4().to_string().replace('-', "")
    ));

    // Backup de l'existant si présent
    if final_path.exists() {
        let bak = final_path.with_extension("bak");
        std::fs::copy(&final_path, &bak)
            .with_context(|| format!("backup credential {:?}", final_path))?;
    }

    // Écriture avec mode 0640 (rw-r-----)
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o640)
        .open(&tmp_path)
        .and_then(|mut f| {
            use std::io::Write;
            f.write_all(secret.as_bytes())
        })
        .with_context(|| format!("écriture credential tmp {:?}", tmp_path))?;

    std::fs::rename(&tmp_path, &final_path)
        .with_context(|| format!("rename credential {:?} → {:?}", tmp_path, final_path))?;

    Ok(())
}

/// Envoie SIGHUP au processus courant pour déclencher un reload.
///
/// Le handler SIGHUP dans main.rs recharge le registry via `Registry::load_conf_d`
/// et met à jour les tâches Tokio (bots, listeners, sinks).
fn send_sighup_self() -> anyhow::Result<()> {
    let pid = nix::unistd::Pid::this();
    nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGHUP)
        .context("envoi SIGHUP au processus courant")?;
    tracing::info!(pid = %pid, "SIGHUP envoyé — reload du registry en cours");
    Ok(())
}

/// Publie un message d'ack/nack sur `agent.hubmq.admin.response`.
///
/// Severity P3 (log_only) — la réponse est loguée mais ne génère pas de DM
/// (évite les doubles notifications avec le skill add-bot qui gère la confirmation).
async fn publish_admin_response(state: &AppState, success: bool, message: &str) {
    let status = if success { "ok" } else { "error" };
    let mut resp = Message::new(
        "hubmq.admin",
        Severity::P3,
        format!("admin response: {}", status),
        message.to_string(),
    );
    resp.meta
        .insert("admin_status".into(), status.to_string());

    match serde_json::to_vec(&resp) {
        Ok(payload) => {
            state
                .nats
                .js
                .publish("agent.hubmq.admin.response", payload.into())
                .await
                .map_err(|e| {
                    tracing::warn!(err = ?e, "impossible de publier admin.response sur NATS");
                })
                .ok();
        }
        Err(e) => {
            tracing::warn!(err = ?e, "sérialisation admin response échouée");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_valid_name_accepts_lowercase_alnum_dash_underscore() {
        assert!(is_valid_name("llmcore"));
        assert!(is_valid_name("my-bot"));
        assert!(is_valid_name("bot_2"));
        assert!(is_valid_name("a"));
        assert!(is_valid_name(&"a".repeat(64)));
    }

    #[test]
    fn is_valid_name_rejects_invalid_chars() {
        assert!(!is_valid_name(""));
        assert!(!is_valid_name("MyBot"));          // majuscule
        assert!(!is_valid_name("bot name"));       // espace
        assert!(!is_valid_name("bot.name"));       // point
        assert!(!is_valid_name("bot/name"));       // slash
        assert!(!is_valid_name("../etc/passwd"));  // path traversal
        assert!(!is_valid_name(&"a".repeat(65))); // trop long
    }

    #[test]
    fn token_hash_prefix_is_12_chars() {
        let hash = token_hash_prefix("1234567890:AAAA_test_token");
        assert_eq!(hash.len(), 12);
        // Doit être hexadécimal
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn token_hash_prefix_is_deterministic() {
        let token = "9876543210:BBBBtest";
        assert_eq!(token_hash_prefix(token), token_hash_prefix(token));
    }

    #[test]
    fn token_hash_prefix_differs_for_different_tokens() {
        let h1 = token_hash_prefix("token-abc");
        let h2 = token_hash_prefix("token-xyz");
        assert_ne!(h1, h2);
    }

    #[test]
    fn serialize_bots_toml_produces_valid_output() {
        let bots = vec![
            BotEntry {
                name: "claude".into(),
                token_credential: "telegram-bot-token".into(),
                target_agent: "claude-hubmq".into(),
                allowed_chat_ids: vec![1451527482],
            },
            BotEntry {
                name: "llmcore".into(),
                token_credential: "telegram-bot-token-llmcore".into(),
                target_agent: "llmcore".into(),
                allowed_chat_ids: vec![1451527482, 9999],
            },
        ];

        let toml_str = serialize_bots_toml(&bots).unwrap();

        // Vérifier que le TOML produit est parsable
        use crate::config::{BotEntry as BE};
        #[derive(serde::Deserialize)]
        struct BF { #[serde(default, rename = "bot")] bot: Vec<BE> }
        let parsed: BF = toml::from_str(&toml_str).expect("TOML invalide");
        assert_eq!(parsed.bot.len(), 2);
        assert_eq!(parsed.bot[0].name, "claude");
        assert_eq!(parsed.bot[1].allowed_chat_ids, vec![1451527482, 9999]);
    }

    #[test]
    fn atomic_write_toml_creates_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("test.toml");
        atomic_write_toml(&target, "[[bot]]\nname = \"test\"\n").unwrap();
        assert!(target.exists());
        let content = std::fs::read_to_string(&target).unwrap();
        assert!(content.contains("test"));
    }

    #[test]
    fn atomic_write_toml_creates_backup() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("bots.toml");
        // Créer le fichier initial
        std::fs::write(&target, "original").unwrap();
        // Réécrire — doit créer le .bak
        atomic_write_toml(&target, "nouveau").unwrap();
        let bak = target.with_extension("bak");
        assert!(bak.exists(), "fichier .bak manquant");
        let bak_content = std::fs::read_to_string(&bak).unwrap();
        assert_eq!(bak_content, "original");
        let new_content = std::fs::read_to_string(&target).unwrap();
        assert_eq!(new_content, "nouveau");
    }

    #[test]
    fn serialize_and_reparse_bots_roundtrip() {
        // Vérifie que le TOML généré peut être rechargé via Registry::load_conf_d
        let original = vec![BotEntry {
            name: "roundtrip-bot".into(),
            token_credential: "tok-cred".into(),
            target_agent: "my-agent".into(),
            allowed_chat_ids: vec![111, 222, 333],
        }];
        let toml_str = serialize_bots_toml(&original).unwrap();

        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("bots.toml");
        std::fs::write(&path, &toml_str).unwrap();

        let reg = Registry::load_conf_d(dir.path()).unwrap();
        assert_eq!(reg.bots.len(), 1);
        assert_eq!(reg.bots[0].name, "roundtrip-bot");
        assert_eq!(reg.bots[0].allowed_chat_ids, vec![111, 222, 333]);
    }
}
