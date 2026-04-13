/// Dispatcher HubMQ — consomme les streams NATS JetStream et route vers les sinks.
///
/// Deux boucles indépendantes via `tokio::select!` :
/// - `run_downstream` : 2 consumers (AGENTS/CRON) — ALERTS/MONITOR/SYSTEM sont archivés et consommés par BigBrother externe qui publie des digests curés sur AGENTS
/// - `run_upstream`   : consumer USER_IN → bridge msg-relay (commandes Telegram entrantes)
///
/// Aucun crash silencieux : toutes les erreurs de delivery sont loguées via `tracing::warn`.
/// Les messages mal formés sont ackés pour éviter la re-livraison NATS.
use crate::{
    app_state::AppState,
    bridge::Bridge,
    message::Message,
    sink::Sink,
};
use async_nats::jetstream;
use chrono::Local;
use futures_util::StreamExt;
use std::sync::Arc;

/// Dispatcher principal.
///
/// Tous les champs sont publics pour permettre la construction depuis `hubmq-bin`.
pub struct Dispatcher {
    pub state: AppState,
    /// Sink email (optionnel).
    pub email: Option<Arc<dyn Sink>>,
    /// Sink ntfy (optionnel).
    pub ntfy: Option<Arc<dyn Sink>>,
    /// Sink Telegram via Apprise (optionnel).
    pub telegram_apprise: Option<Arc<dyn Sink>>,
    /// Pont msg-relay pour les commandes entrantes.
    pub bridge: Bridge,
}

impl Dispatcher {
    /// Lance le dispatcher : boucle downstream (alertes) + boucle upstream (commandes).
    ///
    /// Bloque jusqu'à ce que l'une des deux boucles se termine (erreur ou signal).
    /// Utilise `tokio::select!` pour une terminaison propre dès la première sortie.
    pub async fn run(self: Arc<Self>) -> anyhow::Result<()> {
        let downstream = self.clone().run_downstream();
        let upstream = self.clone().run_upstream();
        tokio::select! {
            r = downstream => r,
            r = upstream => r,
        }
    }

    /// Spawne 2 consumers JetStream (AGENTS + CRON).
    ///
    /// ALERTS/MONITOR/SYSTEM restent créés par `ensure_streams` pour archivage,
    /// mais ne sont plus consommés ici — BigBrother les lit et publie des digests
    /// curés sur le stream AGENTS avant délivrance aux sinks.
    /// Chaque consumer est exécuté dans sa propre tâche Tokio.
    /// La boucle principale attend indéfiniment (les erreurs de consumer sont loguées,
    /// pas propagées — pour ne pas couper les autres consumers).
    async fn run_downstream(self: Arc<Self>) -> anyhow::Result<()> {
        let streams = [
            ("AGENTS", "hubmq-agents"),
            ("CRON",   "hubmq-cron"),
        ];

        for (stream_name, consumer_name) in streams {
            let js = self.state.nats.js.clone();
            let self_c = self.clone();
            let stream = stream_name.to_string();
            let consumer = consumer_name.to_string();
            tokio::spawn(async move {
                if let Err(e) = self_c.consume(&js, &stream, &consumer).await {
                    tracing::error!(stream = %stream, err = ?e, "consumer downstream terminé avec erreur");
                }
            });
        }

        // Maintenir la boucle active — les consumers tournent en tâches parallèles
        futures_util::future::pending::<()>().await;
        Ok(())
    }

    /// Consomme un stream NATS JetStream avec un consumer pull durable.
    ///
    /// Crée le consumer s'il n'existe pas encore (idempotent).
    /// Pour chaque message :
    /// - Parse JSON → `Message` HubMQ
    /// - Appelle `route_and_deliver`
    /// - Ack dans tous les cas (y compris erreur de parse) pour éviter la re-livraison
    async fn consume(
        &self,
        js: &jetstream::Context,
        stream_name: &str,
        consumer_name: &str,
    ) -> anyhow::Result<()> {
        let stream = js.get_stream(stream_name).await?;
        let consumer: jetstream::consumer::PullConsumer = stream
            .get_or_create_consumer(
                consumer_name,
                jetstream::consumer::pull::Config {
                    durable_name: Some(consumer_name.into()),
                    ..Default::default()
                },
            )
            .await?;

        let mut msgs = consumer.messages().await?;
        while let Some(msg) = msgs.next().await {
            let msg = msg?;
            match serde_json::from_slice::<Message>(&msg.payload) {
                Ok(m) => {
                    if let Err(e) = self.route_and_deliver(&m).await {
                        tracing::warn!(err = ?e, id = %m.id, "delivery error");
                    }
                    msg.ack().await.ok();
                }
                Err(e) => {
                    tracing::warn!(err = ?e, "message parse échoué — ack pour éviter re-livraison");
                    msg.ack().await.ok();
                }
            }
        }

        Ok(())
    }

    /// Route un message vers les sinks appropriés selon la sévérité et l'heure courante.
    ///
    /// Règles appliquées par `SeverityRouter::route` :
    /// - P3 / quiet hours P2 → log_only : audit + retour immédiat
    /// - P0 : email + ntfy + telegram
    /// - P1 : ntfy + telegram
    /// - P2 hors quiet hours : email + telegram
    ///
    /// Les erreurs de delivery par sink sont loguées sans interrompre les autres canaux.
    /// `queue.mark_delivered` est appelé en fin de livraison (non-bloquant sur erreur).
    async fn route_and_deliver(&self, m: &Message) -> anyhow::Result<()> {
        let now = Local::now().time();
        let channels = self.state.router.route(m.severity, now);
        tracing::info!(id = %m.id, sev = ?m.severity, ?channels, "routing");

        if channels.log_only {
            self.state
                .audit
                .log(
                    "message_log_only",
                    Some(&m.source),
                    Some(&m.title),
                    &serde_json::json!({"id": m.id.to_string()}),
                )
                .await
                .ok();
            return Ok(());
        }

        if channels.email {
            if let Some(s) = &self.email {
                if let Err(e) = s.deliver(m).await {
                    tracing::warn!(err = ?e, "email delivery failed");
                } else {
                    self.state
                        .audit
                        .log(
                            "message_delivered",
                            Some(&m.source),
                            Some("email"),
                            &serde_json::json!({"id": m.id.to_string()}),
                        )
                        .await
                        .ok();
                }
            }
        }

        if channels.ntfy {
            if let Some(s) = &self.ntfy {
                if let Err(e) = s.deliver(m).await {
                    tracing::warn!(err = ?e, "ntfy delivery failed");
                } else {
                    self.state
                        .audit
                        .log(
                            "message_delivered",
                            Some(&m.source),
                            Some("ntfy"),
                            &serde_json::json!({"id": m.id.to_string()}),
                        )
                        .await
                        .ok();
                }
            }
        }

        if channels.telegram {
            if let Some(s) = &self.telegram_apprise {
                if let Err(e) = s.deliver(m).await {
                    tracing::warn!(err = ?e, "telegram delivery failed");
                } else {
                    self.state
                        .audit
                        .log(
                            "message_delivered",
                            Some(&m.source),
                            Some("telegram"),
                            &serde_json::json!({"id": m.id.to_string()}),
                        )
                        .await
                        .ok();
                }
            }
        }

        // Marquer livré dans la file SQLite (non-bloquant sur erreur)
        self.state
            .queue
            .mark_delivered(&m.id.to_string())
            .await
            .ok();

        Ok(())
    }

    /// Consomme le stream USER_IN et transmet via le bridge msg-relay.
    ///
    /// Les messages non-parsables sont ackés silencieusement.
    /// Les résultats du bridge (dispatché, rejeté, erreur) sont loggés.
    async fn run_upstream(self: Arc<Self>) -> anyhow::Result<()> {
        let stream = self.state.nats.js.get_stream("USER_IN").await?;
        let consumer: jetstream::consumer::PullConsumer = stream
            .get_or_create_consumer(
                "hubmq-user-in",
                jetstream::consumer::pull::Config {
                    durable_name: Some("hubmq-user-in".into()),
                    ..Default::default()
                },
            )
            .await?;

        let mut msgs = consumer.messages().await?;
        while let Some(msg) = msgs.next().await {
            let msg = msg?;
            if let Ok(m) = serde_json::from_slice::<Message>(&msg.payload) {
                match self.bridge.forward_from_telegram(&self.state, &m).await {
                    Ok(true)  => tracing::info!(id = %m.id, "bridge dispatched"),
                    Ok(false) => tracing::info!(id = %m.id, "bridge rejected (whitelist or disabled)"),
                    Err(e)    => tracing::warn!(err = ?e, id = %m.id, "bridge error"),
                }
            }
            msg.ack().await.ok();
        }

        Ok(())
    }
}
