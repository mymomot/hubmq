/// Source Telegram — polling bot avec rejet des messages transférés (B2) et allowlist chat_id.
///
/// B2 : tout message avec `forward_origin` présent est rejeté silencieusement (audit log).
/// Allowlist : seuls les `chat_id` présents dans `bot.allowed_chat_ids` sont acceptés.
///
/// Le message reçu est publié sur le subject NATS `user.incoming.telegram` sous forme de
/// `Message` HubMQ sérialisé en JSON. Un ACK visuel est envoyé à l'expéditeur.
///
/// En mode multi-bot, chaque instance reçoit son `BotEntry` et le `meta.bot_name` est
/// injecté sur chaque message publié pour permettre le routage côté dispatcher.
use crate::{
    app_state::AppState,
    config::BotEntry,
    message::{Message, Severity},
    subjects::Subject,
};
use std::sync::Arc;
use teloxide::{
    prelude::*,
    types::{Message as TgMessage, MessageKind},
};

/// Lance le bot Telegram en mode polling (mode multi-bot).
///
/// Chaque bot a son propre `BotEntry` (nom, allowlist, credential).
/// Le token est passé directement (déjà résolu par main.rs via `read_credential`).
///
/// Bloque jusqu'à erreur fatale ou signal d'arrêt.
///
/// # Effets de bord
/// - Consomme les mises à jour Telegram via long polling.
/// - Publie sur NATS les messages acceptés avec `meta.bot_name = bot.name`.
/// - Enregistre des entrées d'audit pour chaque événement significatif.
pub async fn run_bot(state: AppState, bot: BotEntry, token: String) -> anyhow::Result<()> {
    let tg_bot = Bot::new(token);
    let bot_entry = Arc::new(bot);
    let state = Arc::new(state);

    teloxide::repl(tg_bot.clone(), move |tg_bot: Bot, msg: TgMessage| {
        let state = state.clone();
        let bot_entry = bot_entry.clone();
        async move {
            if let Err(e) = handle(&tg_bot, &msg, &state, &bot_entry).await {
                tracing::warn!(err = ?e, bot = %bot_entry.name, "telegram handler error");
            }
            respond(())
        }
    })
    .await;

    Ok(())
}

/// Backward compat — wrappeur utilisant la config monolithique `[telegram]`.
///
/// Convertit la config existante en `BotEntry` "default" pour maintenir la
/// compatibilité avec le code appelant qui passe uniquement un token.
///
/// # Effets de bord
/// Identiques à `run_bot`.
pub async fn run(state: AppState, token: String) -> anyhow::Result<()> {
    let bot = state.cfg.telegram.clone().into_bot_entry();
    run_bot(state, bot, token).await
}

/// Traite un message Telegram entrant.
///
/// Séquence :
/// 1. Rejet B2 — forward_origin présent → audit + retour silencieux
/// 2. Allowlist — chat_id absent → audit + retour silencieux
/// 3. Corps vide → retour silencieux
/// 4. Construction du `Message` HubMQ + injection meta.bot_name + publication NATS + audit + ACK visuel
async fn handle(
    bot: &Bot,
    msg: &TgMessage,
    state: &AppState,
    bot_entry: &BotEntry,
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
                    &serde_json::json!({"message_id": msg.id.0, "bot": bot_entry.name}),
                )
                .await
                .ok();
            return Ok(());
        }
    }

    // Allowlist chat_id — par-bot (utilise les allowed_chat_ids du BotEntry)
    let chat_id = msg.chat.id.0;
    if !bot_entry.allowed_chat_ids.contains(&chat_id) {
        state
            .audit
            .log(
                "telegram_auth_reject",
                Some(&chat_id.to_string()),
                None,
                &serde_json::json!({"message_id": msg.id.0, "bot": bot_entry.name}),
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
    // Injection du nom du bot pour le routage multi-sink dans le dispatcher
    m.meta.insert("bot_name".into(), bot_entry.name.clone());

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
            &serde_json::json!({"len": text.len(), "id": m.id.to_string(), "bot": bot_entry.name}),
        )
        .await
        .ok();

    // ACK visuel à l'expéditeur
    bot.send_message(msg.chat.id, "\u{2713} Reçu — dispatché à HubMQ")
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BotEntry;

    #[test]
    fn bot_entry_has_name_field() {
        // Vérifie que BotEntry peut être construit avec un nom de bot
        let entry = BotEntry {
            name: "my-bot".into(),
            token_credential: "tok".into(),
            target_agent: "claude-hubmq".into(),
            allowed_chat_ids: vec![1234],
        };
        assert_eq!(entry.name, "my-bot");
        assert_eq!(entry.allowed_chat_ids, vec![1234]);
    }

    #[test]
    fn backward_compat_run_uses_telegram_config() {
        // Vérifie que TelegramConfig::into_bot_entry() produit "default"
        // (le wrapper `run()` s'en sert — testé ici sans appel réseau)
        use crate::config::TelegramConfig;
        let cfg = TelegramConfig {
            allowed_chat_ids: vec![42],
            token_credential: "telegram-bot-token".into(),
        };
        let entry = cfg.into_bot_entry();
        assert_eq!(entry.name, "default");
        assert_eq!(entry.target_agent, "claude-hubmq");
        assert_eq!(entry.allowed_chat_ids, vec![42]);
    }
}
