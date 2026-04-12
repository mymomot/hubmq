/// Source Telegram — polling bot avec rejet des messages transférés (B2) et allowlist chat_id.
///
/// B2 : tout message avec `forward_origin` présent est rejeté silencieusement (audit log).
/// Allowlist : seuls les `chat_id` présents dans `cfg.telegram.allowed_chat_ids` sont acceptés.
///
/// Le message reçu est publié sur le subject NATS `user.incoming.telegram` sous forme de
/// `Message` HubMQ sérialisé en JSON. Un ACK visuel est envoyé à l'expéditeur.
use crate::{
    app_state::AppState,
    message::{Message, Severity},
    subjects::Subject,
};
use std::sync::Arc;
use teloxide::{
    prelude::*,
    types::{Message as TgMessage, MessageKind},
};

/// Lance le bot Telegram en mode polling.
///
/// Bloque jusqu'à erreur fatale ou signal d'arrêt.
///
/// # Effets de bord
/// - Consomme les mises à jour Telegram via long polling.
/// - Publie sur NATS les messages acceptés.
/// - Enregistre des entrées d'audit pour chaque événement significatif.
pub async fn run(state: AppState, token: String) -> anyhow::Result<()> {
    let bot = Bot::new(token);
    let allowed: Vec<i64> = state.cfg.telegram.allowed_chat_ids.clone();
    let state = Arc::new(state);

    teloxide::repl(bot.clone(), move |bot: Bot, msg: TgMessage| {
        let state = state.clone();
        let allowed = allowed.clone();
        async move {
            if let Err(e) = handle(&bot, &msg, &state, &allowed).await {
                tracing::warn!(err = ?e, "telegram handler error");
            }
            respond(())
        }
    })
    .await;

    Ok(())
}

/// Traite un message Telegram entrant.
///
/// Séquence :
/// 1. Rejet B2 — forward_origin présent → audit + retour silencieux
/// 2. Allowlist — chat_id absent → audit + retour silencieux
/// 3. Corps vide → retour silencieux
/// 4. Construction du `Message` HubMQ + publication NATS + audit + ACK visuel
async fn handle(
    bot: &Bot,
    msg: &TgMessage,
    state: &AppState,
    allowed: &[i64],
) -> anyhow::Result<()> {
    // B2 — rejeter les messages transférés (forward_origin présent dans MessageCommon)
    if let MessageKind::Common(ref c) = msg.kind {
        if c.forward_origin.is_some() {
            state
                .audit
                .log(
                    "telegram_forward_rejected",
                    Some(&msg.chat.id.0.to_string()),
                    None,
                    &serde_json::json!({"message_id": msg.id.0}),
                )
                .await
                .ok();
            return Ok(());
        }
    }

    // Allowlist chat_id
    let chat_id = msg.chat.id.0;
    if !allowed.contains(&chat_id) {
        state
            .audit
            .log(
                "telegram_auth_reject",
                Some(&chat_id.to_string()),
                None,
                &serde_json::json!({"message_id": msg.id.0}),
            )
            .await
            .ok();
        return Ok(());
    }

    let text = msg.text().unwrap_or("").trim().to_string();
    if text.is_empty() {
        return Ok(());
    }

    // Construction du Message HubMQ
    let mut m = Message::new(
        "telegram",
        Severity::P2,
        format!("tg msg from {}", chat_id),
        text.clone(),
    );
    m.tags = vec!["telegram".into(), "upstream".into()];
    m.meta.insert("chat_id".into(), chat_id.to_string());
    m.meta.insert("message_id".into(), msg.id.0.to_string());

    // Publication NATS JetStream
    let payload = serde_json::to_vec(&m)?;
    state
        .nats
        .js
        .publish(Subject::user_incoming_telegram(), payload.into())
        .await?;

    // Audit de réception
    state
        .audit
        .log(
            "telegram_message_received",
            Some(&chat_id.to_string()),
            Some("user.incoming.telegram"),
            &serde_json::json!({"len": text.len(), "id": m.id.to_string()}),
        )
        .await
        .ok();

    // ACK visuel à l'expéditeur
    bot.send_message(msg.chat.id, "\u{2713} Reçu — dispatché à HubMQ")
        .await?;

    Ok(())
}
