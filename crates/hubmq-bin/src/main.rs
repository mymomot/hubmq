/// Daemon principal HubMQ — câblage complet des composants avec support multi-bot.
///
/// Séquence de démarrage :
///   1. Initialisation tracing JSON (EnvFilter depuis RUST_LOG)
///   2. Chargement config TOML (HUBMQ_CONFIG ou /etc/hubmq/config.toml)
///   3. Chargement registry conf.d (HUBMQ_CONF_D ou /etc/hubmq/conf.d/)
///      → merge avec la config monolithique [telegram] (backward compat)
///   4. Lecture des credentials via systemd LoadCredential (D3)
///   5. Connexion NATS + bootstrap streams JetStream
///   6. Ouverture queue SQLite + audit log
///   7. Instanciation des filtres (dedup, rate limit, severity router)
///   8. Construction des sinks (email, ntfy, telegram per-bot via Apprise)
///   9. Spawn tâches : HTTP :8470, N bots Telegram, M listeners LLM, dispatcher, admin consumer
///  10. Attente ctrl-c → shutdown propre, ou SIGHUP → reload registry (hot-reload bots/listeners)
///
/// ## Reload SIGHUP
///
/// À réception de SIGHUP :
/// - Rechargement conf.d via `Registry::load_conf_d`
/// - Diff old_registry vs new_registry (bots ajoutés/retirés, listeners ajoutés/retirés)
/// - Kill propre des tâches retirées (drain 5s via AbortHandle)
/// - Spawn des nouvelles tâches
/// - Mise à jour des sinks dans le dispatcher (non implémenté atomiquement — voir note)
///
/// Note : la mise à jour atomique du dispatcher en cours d'exécution nécessiterait un
/// `Arc<Mutex<Dispatcher>>` avec swap des sinks. Pour simplifier, le reload reconstruit
/// un nouveau Dispatcher et abort l'ancienne tâche dispatcher. Les messages en vol
/// sur les consumers NATS durables sont préservés (re-délivraison possible).
use hubmq_core::{
    admin,
    app_state::AppState,
    audit::Audit,
    bridge::Bridge,
    config::{Config, Registry},
    dispatcher::Dispatcher,
    filter::{dedup::DedupCache, ratelimit::AdaptiveRateLimiter, severity::SeverityRouter},
    listener,
    nats_conn::NatsConn,
    queue::Queue,
    sink::{apprise::AppriseSink, email::EmailSink, email_reply::EmailReplySink, ntfy::NtfySink, Sink},
    source::{email as email_source, telegram, webhook},
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::task::AbortHandle;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Tracing JSON structuré — niveau piloté par RUST_LOG
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    // Chargement de la configuration TOML
    let cfg_path =
        std::env::var("HUBMQ_CONFIG").unwrap_or_else(|_| "/etc/hubmq/config.toml".into());
    let cfg = Arc::new(Config::from_file(Path::new(&cfg_path))?);

    tracing::info!(path = %cfg_path, "configuration chargée");

    // Chargement du registry conf.d (multi-bot)
    let conf_d_path =
        std::env::var("HUBMQ_CONF_D").unwrap_or_else(|_| "/etc/hubmq/conf.d".into());
    let conf_d = PathBuf::from(&conf_d_path);

    let mut registry = if conf_d.is_dir() {
        Registry::load_conf_d(&conf_d)?
    } else {
        tracing::warn!(path = %conf_d_path, "conf.d absent ou non-répertoire — mode monolithique uniquement");
        Registry::default()
    };

    // Backward compat : si conf.d vide ou [telegram] présente → ajouter BotEntry "default"
    if registry.bots.is_empty() {
        tracing::info!("conf.d sans bots — ajout BotEntry 'default' depuis [telegram]");
        let default_registry = cfg.as_ref().clone().into_registry();
        registry.merge(default_registry);
    } else {
        // Toujours ajouter l'agent claude-hubmq par défaut si absent
        if registry.find_agent("claude-hubmq").is_none() {
            use hubmq_core::config::AgentEntry;
            registry.agents.push(AgentEntry {
                name: "claude-hubmq".into(),
                kind: "claude-code-spawn".into(),
                listener_subject: Some("user.incoming.telegram".into()),
                endpoint: None,
                model: None,
                system_prompt: None,
                timeout_secs: None,
            });
        }
    }

    // Validation cross-refs
    let cross_ref_errors = registry.validate_cross_refs();
    for err in &cross_ref_errors {
        tracing::warn!(error = %err, "avertissement cross-ref registry");
    }

    let registry = Arc::new(RwLock::new(registry));

    // D3 — lecture des credentials via systemd LoadCredential
    let creds_dir = std::env::var("CREDENTIALS_DIRECTORY").ok().map(PathBuf::from);
    let smtp_password = read_credential(&creds_dir, &cfg.smtp.password_credential)?;

    // Lecture config source email (optionnelle — backward compat si section absente)
    let email_source_enabled = cfg.email_source.as_ref().map(|e| e.enabled).unwrap_or(false);

    // NATS JetStream — connexion + bootstrap des streams (D2)
    let nats = Arc::new(NatsConn::connect(&cfg.nats).await?);
    nats.ensure_streams().await?;

    // Queue SQLite + audit log (pool partagé)
    let queue = Arc::new(
        Queue::open(Path::new("/var/lib/hubmq/queue.db")).await?,
    );
    let audit = Arc::new(Audit::new(queue.pool_clone()));

    // Filtres
    let dedup = Arc::new(DedupCache::new(cfg.filter.dedup_window_secs));
    let rate = Arc::new(AdaptiveRateLimiter::new(
        cfg.filter.rate_limit_per_min,
        cfg.filter.rate_limit_p0_per_min,
    ));
    let router = Arc::new(SeverityRouter::new(
        &cfg.filter.quiet_hours_start,
        &cfg.filter.quiet_hours_end,
    )?);

    let state = AppState {
        cfg: cfg.clone(),
        nats: nats.clone(),
        queue,
        audit,
        dedup,
        rate,
        router,
    };

    // Sink email (SMTP Gmail STARTTLS — alertes)
    let email: Arc<dyn Sink> =
        Arc::new(EmailSink::new(&cfg.smtp, &smtp_password, &cfg.fallback.email_to)?);

    // Sink email reply (SMTP Gmail — réponses dans les threads email entrants)
    let email_reply: Option<Arc<dyn Sink>> = match EmailReplySink::new(&cfg.smtp, &smtp_password) {
        Ok(s) => Some(Arc::new(s) as Arc<dyn Sink>),
        Err(e) => {
            tracing::warn!(err = ?e, "impossible d'initialiser le sink email_reply — source email désactivée");
            None
        }
    };

    // Sink ntfy (conditionnel — activé seulement si base_url + topic configurés)
    let ntfy: Option<Arc<dyn Sink>> = match (&cfg.ntfy.base_url, &cfg.ntfy.topic) {
        (Some(base), Some(topic)) => {
            let bearer = cfg
                .ntfy
                .bearer_credential
                .as_ref()
                .and_then(|c| read_credential(&creds_dir, c).ok());
            Some(Arc::new(NtfySink::new(base.clone(), topic.clone(), bearer)) as Arc<dyn Sink>)
        }
        _ => None,
    };

    // Bridge msg-relay (optionnel)
    let bridge_bearer = cfg
        .bridge
        .bearer_credential
        .as_ref()
        .and_then(|c| read_credential(&creds_dir, c).ok());
    let bridge = Bridge::new(&cfg.bridge, bridge_bearer);

    // ── Spawn initial des tâches ────────────────────────────────────────────────
    // Les handles sont stockés dans une map indexée par "clé de tâche" pour le diff SIGHUP.

    // Tâche 1 : serveur HTTP d'ingestion sur :8470
    let http_state = state.clone();
    let http_task = tokio::spawn(async move {
        let app = webhook::routes(http_state);
        let listener = tokio::net::TcpListener::bind("0.0.0.0:8470")
            .await
            .expect("bind :8470 échoué — port déjà utilisé ou permissions insuffisantes");
        tracing::info!("HTTP ingestion server listening on 0.0.0.0:8470");
        axum::serve(listener, app)
            .await
            .expect("axum::serve terminé de manière inattendue");
    });

    // Spawn source email IMAP (optionnel — conditionné par [email_source].enabled)
    if email_source_enabled {
        if let Some(email_cfg) = cfg.email_source.as_ref() {
            let email_src_cfg = email_source::EmailSourceConfig {
                imap_host: email_cfg.imap_host.clone(),
                imap_port: email_cfg.imap_port,
                username: cfg.smtp.username.clone(),
                password: smtp_password.clone(),
                allowed_from: email_cfg.allowed_from.clone(),
                poll_interval_secs: email_cfg.poll_interval_secs,
            };
            let state_email = state.clone();
            tokio::spawn(async move {
                email_source::run(state_email, email_src_cfg).await;
            });
            tracing::info!(
                username = %cfg.smtp.username,
                allowed_from = ?email_cfg.allowed_from,
                poll_secs = email_cfg.poll_interval_secs,
                "source email IMAP activée"
            );
        }
    } else {
        tracing::info!("source email IMAP désactivée (email_source.enabled = false ou section absente)");
    }

    // Spawn initial bots + listeners + dispatcher
    let (
        bot_handles,
        listener_handles,
        dispatcher_task,
        admin_task,
    ) = spawn_all(
        &state,
        &registry,
        &creds_dir,
        SpawnSinks {
            email: email.clone(),
            ntfy: ntfy.clone(),
            email_reply: email_reply.clone(),
        },
        bridge.clone(),
        &conf_d,
    )
    .await?;

    // Map des handles actifs (pour diff SIGHUP)
    let mut active_bot_handles: HashMap<String, AbortHandle> = bot_handles;
    let mut active_listener_handles: HashMap<String, AbortHandle> = listener_handles;
    let mut active_dispatcher: tokio::task::JoinHandle<()> = dispatcher_task;
    let mut active_admin: tokio::task::JoinHandle<()> = admin_task;

    tracing::info!("hubmq daemon démarré — en attente de ctrl-c / SIGHUP");

    // ── Boucle principale : ctrl-c ou SIGHUP ──────────────────────────────────
    let mut sighup = tokio::signal::unix::signal(
        tokio::signal::unix::SignalKind::hangup(),
    )?;

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("shutdown demandé (ctrl-c)");
                break;
            }
            _ = sighup.recv() => {
                tracing::info!("SIGHUP reçu — reload du registry");

                // Rechargement conf.d
                let new_registry = if conf_d.is_dir() {
                    match Registry::load_conf_d(&conf_d) {
                        Ok(r) => r,
                        Err(e) => {
                            tracing::error!(err = ?e, "reload conf.d échoué — registry inchangé");
                            continue;
                        }
                    }
                } else {
                    Registry::default()
                };

                // Diff et mise à jour — write lock court : collecter les handles, relâcher le lock,
                // puis faire le drain + abort hors du lock pour ne pas bloquer dispatcher/admin.
                let (bots_to_abort, listeners_to_abort) = {
                    let mut reg = registry.write().await;
                    let old_bot_names: std::collections::HashSet<String> =
                        reg.bots.iter().map(|b| b.name.clone()).collect();
                    let new_bot_names: std::collections::HashSet<String> =
                        new_registry.bots.iter().map(|b| b.name.clone()).collect();

                    let bots: Vec<(String, AbortHandle)> = old_bot_names
                        .difference(&new_bot_names)
                        .filter_map(|removed| {
                            active_bot_handles
                                .remove(removed)
                                .map(|h| (removed.clone(), h))
                        })
                        .collect();

                    let old_listener_names: std::collections::HashSet<String> =
                        reg.agents.iter()
                            .filter(|a| a.kind == "llm-openai-compat")
                            .map(|a| a.name.clone())
                            .collect();
                    let new_listener_names: std::collections::HashSet<String> =
                        new_registry.agents.iter()
                            .filter(|a| a.kind == "llm-openai-compat")
                            .map(|a| a.name.clone())
                            .collect();
                    let listeners: Vec<(String, AbortHandle)> = old_listener_names
                        .difference(&new_listener_names)
                        .filter_map(|removed| {
                            active_listener_handles
                                .remove(removed)
                                .map(|h| (removed.clone(), h))
                        })
                        .collect();

                    *reg = new_registry;
                    // write lock relâché ici
                    (bots, listeners)
                };

                // Drain 5s + abort HORS du write lock — ne bloque plus le dispatcher/admin
                for (name, handle) in bots_to_abort {
                    tracing::info!(bot = %name, "bot retiré — drain 5s avant abort tâche polling");
                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    handle.abort();
                }
                for (name, handle) in listeners_to_abort {
                    tracing::info!(listener = %name, "listener retiré — abort tâche");
                    handle.abort();
                }

                // FIX critique : abort les tâches encore actives (bots + listeners "unchanged")
                // AVANT de rebuild via spawn_all. Sinon spawn_all crée des doublons polling
                // le même token Telegram → erreur 409 TerminatedByOtherGetUpdates.
                // Les bots/listeners seront respawnés propres ci-dessous.
                for (name, handle) in active_bot_handles.drain() {
                    tracing::debug!(bot = %name, "abort ancien polling bot avant respawn");
                    handle.abort();
                }
                for (name, handle) in active_listener_handles.drain() {
                    tracing::debug!(listener = %name, "abort ancien listener avant respawn");
                    handle.abort();
                }
                // Bref délai pour laisser les connexions Telegram se fermer côté API
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

                // Rebuild dispatcher avec nouveaux sinks + spawn bots/listeners nouveaux
                active_dispatcher.abort();
                active_admin.abort();

                let (new_bot_handles, new_listener_handles, new_dispatcher, new_admin) =
                    match spawn_all(
                        &state,
                        &registry,
                        &creds_dir,
                        SpawnSinks {
                            email: email.clone(),
                            ntfy: ntfy.clone(),
                            email_reply: email_reply.clone(),
                        },
                        bridge.clone(),
                        &conf_d,
                    )
                    .await
                {
                    Ok(handles) => handles,
                    Err(e) => {
                        tracing::error!(err = ?e, "spawn_all après SIGHUP échoué");
                        continue;
                    }
                };

                // Tous les anciens handles ont été aborted ci-dessus : insertion directe
                // (active_bot_handles/active_listener_handles ont été drain())
                for (name, handle) in new_bot_handles {
                    active_bot_handles.insert(name, handle);
                }
                for (name, handle) in new_listener_handles {
                    active_listener_handles.insert(name, handle);
                }
                active_dispatcher = new_dispatcher;
                active_admin = new_admin;

                tracing::info!("reload SIGHUP terminé");
            }
        }
    }

    // Shutdown propre
    tracing::info!("shutdown — arrêt des tâches");
    http_task.abort();
    for (_, handle) in active_bot_handles {
        handle.abort();
    }
    for (_, handle) in active_listener_handles {
        handle.abort();
    }
    active_dispatcher.abort();
    active_admin.abort();

    Ok(())
}

/// Sinks passés à `spawn_all` regroupés pour éviter la limite clippy de 7 arguments.
struct SpawnSinks {
    pub email: Arc<dyn Sink>,
    pub ntfy: Option<Arc<dyn Sink>>,
    pub email_reply: Option<Arc<dyn Sink>>,
}

/// Spawn l'ensemble des tâches (bots, listeners, dispatcher, admin) depuis le registry courant.
///
/// Retourne les handles indexés par nom pour permettre le diff SIGHUP.
async fn spawn_all(
    state: &AppState,
    registry: &Arc<RwLock<Registry>>,
    creds_dir: &Option<PathBuf>,
    sinks: SpawnSinks,
    bridge: Bridge,
    _conf_d: &Path,
) -> anyhow::Result<(
    HashMap<String, AbortHandle>,
    HashMap<String, AbortHandle>,
    tokio::task::JoinHandle<()>,
    tokio::task::JoinHandle<()>,
)> {
    let reg = registry.read().await;

    let mut bot_handles: HashMap<String, AbortHandle> = HashMap::new();
    let mut listener_handles: HashMap<String, AbortHandle> = HashMap::new();
    let mut telegram_sinks: HashMap<String, Arc<dyn Sink>> = HashMap::new();

    // ── Spawn bots Telegram ──────────────────────────────────────────────────
    for bot in &reg.bots {
        let token = match read_credential(creds_dir, &bot.token_credential) {
            Ok(t) => t,
            Err(e) => {
                tracing::error!(bot = %bot.name, err = ?e, "credential introuvable — bot ignoré");
                continue;
            }
        };

        // Sink Apprise per-bot : URLs tgram://TOKEN/CHAT_ID
        let tg_urls: Vec<String> = bot
            .allowed_chat_ids
            .iter()
            .map(|id| format!("tgram://{}/{}", token, id))
            .collect();
        let sink: Arc<dyn Sink> = Arc::new(AppriseSink::new(tg_urls));
        telegram_sinks.insert(bot.name.clone(), sink);

        // Spawn tâche polling
        let state_c = state.clone();
        let bot_entry = bot.clone();
        let token_c = token.clone();
        let join_handle = tokio::spawn(async move {
            if let Err(e) = telegram::run_bot(state_c, bot_entry.clone(), token_c).await {
                tracing::error!(bot = %bot_entry.name, err = ?e, "telegram bot crashed");
            }
        });
        let abort_handle = join_handle.abort_handle();
        bot_handles.insert(bot.name.clone(), abort_handle);
        // On ne stocke pas le JoinHandle lui-même — les tâches sont autonomes
        // et leur cycle de vie est géré via AbortHandle.
        std::mem::forget(join_handle);
    }

    // ── Spawn listeners LLM ─────────────────────────────────────────────────
    for agent in reg.agents.iter().filter(|a| a.kind == "llm-openai-compat") {
        let state_c = state.clone();
        let agent_c = agent.clone();
        let join_handle = tokio::spawn(async move {
            if let Err(e) = listener::llm_openai::run(state_c, agent_c.clone()).await {
                tracing::error!(agent = %agent_c.name, err = ?e, "listener llm-openai-compat crashed");
            }
        });
        let abort_handle = join_handle.abort_handle();
        listener_handles.insert(agent.name.clone(), abort_handle);
        std::mem::forget(join_handle);
    }

    // ── Dispatcher ──────────────────────────────────────────────────────────
    let dispatcher = Arc::new(Dispatcher {
        state: state.clone(),
        email: Some(sinks.email),
        ntfy: sinks.ntfy,
        telegram_sinks,
        email_reply: sinks.email_reply,
        bridge,
    });

    let dispatcher_task = tokio::spawn(async move {
        if let Err(e) = dispatcher.run().await {
            tracing::error!(err = ?e, "dispatcher crashed");
        }
    });

    // ── Admin consumer ───────────────────────────────────────────────────────
    let state_admin = state.clone();
    let registry_admin = registry.clone();
    let admin_task = tokio::spawn(async move {
        if let Err(e) = admin::run(state_admin, registry_admin).await {
            tracing::error!(err = ?e, "admin consumer crashed");
        }
    });

    Ok((bot_handles, listener_handles, dispatcher_task, admin_task))
}

/// D3 — Lit un credential depuis $CREDENTIALS_DIRECTORY (systemd LoadCredential)
/// ou depuis le répertoire de fallback /etc/hubmq/credentials/.
///
/// Priorité : $CREDENTIALS_DIRECTORY/{name} si présent, sinon /etc/hubmq/credentials/{name}.
/// La valeur est trimée des espaces et retours à la ligne.
///
/// # Erreurs
/// Retourne une erreur si le fichier est absent des deux emplacements ou illisible.
fn read_credential(creds_dir: &Option<PathBuf>, name: &str) -> anyhow::Result<String> {
    if let Some(dir) = creds_dir {
        let p = dir.join(name);
        if p.exists() {
            return Ok(std::fs::read_to_string(&p)?.trim().to_string());
        }
    }
    // Fallback développement / hors systemd
    let fallback = format!("/etc/hubmq/credentials/{}", name);
    Ok(std::fs::read_to_string(&fallback)
        .map_err(|e| anyhow::anyhow!("credential '{}' introuvable ({}) : {}", name, fallback, e))?
        .trim()
        .to_string())
}
