/// Daemon principal HubMQ — câblage complet des composants.
///
/// Séquence de démarrage :
///   1. Initialisation tracing JSON (EnvFilter depuis RUST_LOG)
///   2. Chargement config TOML (HUBMQ_CONFIG ou /etc/hubmq/config.toml)
///   3. Lecture des credentials via systemd LoadCredential (D3)
///   4. Connexion NATS + bootstrap streams JetStream
///   5. Ouverture queue SQLite + audit log
///   6. Instanciation des filtres (dedup, rate limit, severity router)
///   7. Construction des sinks (email, ntfy conditionnel, telegram via Apprise)
///   8. Spawn 3 tâches : HTTP :8470, bot Telegram, dispatcher
///   9. Attente ctrl-c → shutdown propre (abort des 3 tâches)
use hubmq_core::{
    app_state::AppState,
    audit::Audit,
    bridge::Bridge,
    config::Config,
    dispatcher::Dispatcher,
    filter::{dedup::DedupCache, ratelimit::AdaptiveRateLimiter, severity::SeverityRouter},
    nats_conn::NatsConn,
    queue::Queue,
    sink::{apprise::AppriseSink, email::EmailSink, ntfy::NtfySink, Sink},
    source::{telegram, webhook},
};
use std::path::PathBuf;
use std::sync::Arc;
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
    let cfg = Arc::new(Config::from_file(std::path::Path::new(&cfg_path))?);

    tracing::info!(path = %cfg_path, "configuration chargée");

    // D3 — lecture des credentials via systemd LoadCredential
    let creds_dir = std::env::var("CREDENTIALS_DIRECTORY").ok().map(PathBuf::from);
    let smtp_password = read_credential(&creds_dir, &cfg.smtp.password_credential)?;
    let telegram_token = read_credential(&creds_dir, &cfg.telegram.token_credential)?;

    // NATS JetStream — connexion + bootstrap des 6 streams (D2)
    let nats = Arc::new(NatsConn::connect(&cfg.nats).await?);
    nats.ensure_streams().await?;

    // Queue SQLite + audit log (pool partagé)
    let queue = Arc::new(
        Queue::open(std::path::Path::new("/var/lib/hubmq/queue.db")).await?,
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

    // Sink email (SMTP Gmail STARTTLS)
    let email: Arc<dyn Sink> =
        Arc::new(EmailSink::new(&cfg.smtp, &smtp_password, &cfg.fallback.email_to)?);

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

    // Sink Telegram sortant via Apprise — URLs tgram://TOKEN/CHAT_ID
    let tg_urls: Vec<String> = cfg
        .telegram
        .allowed_chat_ids
        .iter()
        .map(|id| format!("tgram://{}/{}", telegram_token, id))
        .collect();
    let telegram_apprise: Arc<dyn Sink> = Arc::new(AppriseSink::new(tg_urls));

    // Bridge msg-relay (optionnel — désactivé si msg_relay_url absent)
    // Résolution du bearer credential si configuré (pattern identique à ntfy)
    let bridge_bearer = cfg
        .bridge
        .bearer_credential
        .as_ref()
        .and_then(|c| read_credential(&creds_dir, c).ok());
    let bridge = Bridge::new(&cfg.bridge, bridge_bearer);

    let dispatcher = Arc::new(Dispatcher {
        state: state.clone(),
        email: Some(email),
        ntfy,
        telegram_apprise: Some(telegram_apprise),
        bridge,
    });

    // Spawn tâche 1 : serveur HTTP d'ingestion sur :8470
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

    // Spawn tâche 2 : bot Telegram polling
    let tg_state = state.clone();
    let tg_token = telegram_token.clone();
    let tg_task = tokio::spawn(async move {
        if let Err(e) = telegram::run(tg_state, tg_token).await {
            tracing::error!(err = ?e, "telegram bot crashed");
        }
    });

    // Spawn tâche 3 : dispatcher JetStream → sinks
    let dispatcher_task = tokio::spawn(async move {
        if let Err(e) = dispatcher.run().await {
            tracing::error!(err = ?e, "dispatcher crashed");
        }
    });

    tracing::info!("hubmq daemon démarré — en attente de ctrl-c");

    tokio::signal::ctrl_c().await?;

    tracing::info!("shutdown demandé — arrêt des tâches");
    http_task.abort();
    tg_task.abort();
    dispatcher_task.abort();

    Ok(())
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
