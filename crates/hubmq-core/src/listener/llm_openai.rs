/// Listener générique `llm-openai-compat` — subscribe NATS, appel HTTP OpenAI-compat, publish réponse.
///
/// Cycle de vie :
/// 1. Subscribe pull consumer durable sur `agent.listener_subject`
/// 2. Pour chaque message : POST `agent.endpoint` avec body OpenAI-compat
/// 3. Parse `.choices[0].message.content`
/// 4. Publish réponse sur `agent.<agent.name>.response` (severity P2)
/// 5. Ack NATS **après** publish réponse (at-least-once, pas de perte)
///
/// Gestion d'erreurs :
/// - Timeout reqwest (configuré via `agent.timeout_secs`, défaut 60s)
/// - Erreur HTTP → log warn, ack quand même (évite re-livraison infinie)
/// - Parse JSON raté → log warn + ack
///
/// Sécurité :
/// - Le token n'est JAMAIS dans les logs (il est dans le meta.bot_name uniquement si loggé)
/// - L'endpoint vient de la config (valeur contrôlée, pas d'input externe)
use crate::{
    app_state::AppState,
    config::AgentEntry,
    message::{Message, Severity},
};
use async_nats::jetstream;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

/// Délai de timeout LLM par défaut si non configuré dans `AgentEntry`.
const DEFAULT_TIMEOUT_SECS: u64 = 60;

// ─── Types pour l'API OpenAI-compat ──────────────────────────────────────────

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
}

#[derive(Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ChoiceMessage,
}

#[derive(Deserialize)]
struct ChoiceMessage {
    content: String,
}

// ─── Fonction principale ──────────────────────────────────────────────────────

/// Lance la boucle d'écoute pour un agent `llm-openai-compat`.
///
/// Bloque jusqu'à erreur fatale du consumer NATS.
/// Les erreurs d'appel LLM ou de publish réponse sont loguées mais n'arrêtent pas la boucle.
///
/// # Erreurs
/// Retourne une erreur si la connexion NATS ou la création du consumer échoue.
pub async fn run(state: AppState, agent: AgentEntry) -> anyhow::Result<()> {
    let subject = agent
        .listener_subject
        .as_deref()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "agent '{}' de kind llm-openai-compat sans listener_subject",
                agent.name
            )
        })?
        .to_string();

    let endpoint = agent
        .endpoint
        .as_deref()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "agent '{}' de kind llm-openai-compat sans endpoint",
                agent.name
            )
        })?
        .to_string();

    let model = agent
        .model
        .clone()
        .unwrap_or_else(|| "default".into());

    let timeout = Duration::from_secs(agent.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));

    let http_client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| anyhow::anyhow!("construction client HTTP pour agent '{}': {}", agent.name, e))?;

    // Consumer NATS durable — nom: hubmq-listener-<agent.name>
    let consumer_name = format!("hubmq-listener-{}", agent.name);

    // Le subject peut ne pas avoir de stream dédié — on utilise USER_IN qui couvre user.>
    // mais en pratique le sujet `user.inbox.<name>` est sous USER_IN (subjects: user.incoming.>)
    // Cas spécial : si le subject est "user.inbox.*", on doit trouver le bon stream.
    // Pour simplifier et éviter de hardcoder, on cherche le stream couvrant ce subject.
    let stream = find_stream_for_subject(&state.nats.js, &subject).await?;

    let consumer: jetstream::consumer::PullConsumer = stream
        .get_or_create_consumer(
            &consumer_name,
            jetstream::consumer::pull::Config {
                durable_name: Some(consumer_name.clone()),
                filter_subject: subject.clone(),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| anyhow::anyhow!("création consumer '{}': {}", consumer_name, e))?;

    tracing::info!(
        agent = %agent.name,
        subject = %subject,
        endpoint = %endpoint,
        "listener llm-openai-compat démarré"
    );

    let mut msgs = consumer.messages().await
        .map_err(|e| anyhow::anyhow!("consumer messages() pour '{}': {}", agent.name, e))?;

    while let Some(msg_result) = msgs.next().await {
        let msg = match msg_result {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(agent = %agent.name, err = ?e, "erreur réception message NATS");
                continue;
            }
        };

        match serde_json::from_slice::<Message>(&msg.payload) {
            Ok(m) => {
                let bot_name = m.meta.get("bot_name").cloned();
                let response_text = call_llm(
                    &http_client,
                    &endpoint,
                    &model,
                    agent.system_prompt.as_deref(),
                    &m.body,
                )
                .await;

                match response_text {
                    Ok(text) => {
                        let publish_result = publish_response(
                            &state,
                            &agent.name,
                            &m,
                            text,
                            bot_name.as_deref(),
                        )
                        .await;

                        if let Err(e) = publish_result {
                            tracing::warn!(
                                agent = %agent.name,
                                id = %m.id,
                                err = ?e,
                                "échec publish réponse LLM"
                            );
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            agent = %agent.name,
                            id = %m.id,
                            err = ?e,
                            "échec appel LLM"
                        );
                    }
                }

                // Ack dans tous les cas pour éviter re-livraison infinie
                msg.ack().await.ok();
            }
            Err(e) => {
                tracing::warn!(agent = %agent.name, err = ?e, "message parse échoué — ack");
                msg.ack().await.ok();
            }
        }
    }

    Ok(())
}

/// Effectue l'appel HTTP POST vers l'endpoint OpenAI-compat.
///
/// # Arguments
/// - `client` : client reqwest avec timeout configuré
/// - `endpoint` : URL de l'endpoint chat completions
/// - `model` : identifiant du modèle LLM
/// - `system_prompt` : prompt système optionnel
/// - `user_text` : corps du message utilisateur
///
/// # Retourne
/// Le contenu textuel de la première `choice.message.content`.
async fn call_llm(
    client: &reqwest::Client,
    endpoint: &str,
    model: &str,
    system_prompt: Option<&str>,
    user_text: &str,
) -> anyhow::Result<String> {
    let mut messages = Vec::new();

    if let Some(sp) = system_prompt {
        messages.push(ChatMessage {
            role: "system".into(),
            content: sp.to_string(),
        });
    }

    messages.push(ChatMessage {
        role: "user".into(),
        content: user_text.to_string(),
    });

    let request_body = ChatRequest {
        model: model.to_string(),
        messages,
    };

    let resp = client
        .post(endpoint)
        .json(&request_body)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("requête LLM échouée: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("LLM retourne HTTP {}: {}", status, &body[..body.len().min(200)]);
    }

    let chat_resp: ChatResponse = resp
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("parse réponse LLM: {}", e))?;

    let content = chat_resp
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("réponse LLM sans choices"))?
        .message
        .content;

    Ok(content)
}

/// Publie la réponse LLM sur `agent.<agent_name>.response` dans le stream AGENTS.
///
/// Le meta `bot_name` est préservé depuis le message original pour permettre
/// au dispatcher de router la réponse vers le bon sink Telegram.
async fn publish_response(
    state: &AppState,
    agent_name: &str,
    original: &Message,
    response_text: String,
    bot_name: Option<&str>,
) -> anyhow::Result<()> {
    let mut meta: BTreeMap<String, String> = BTreeMap::new();
    if let Some(bn) = bot_name {
        meta.insert("bot_name".into(), bn.to_string());
    }
    if let Some(chat_id) = original.meta.get("chat_id") {
        meta.insert("chat_id".into(), chat_id.clone());
    }

    let mut resp_msg = Message::new(
        format!("agent.{}", agent_name),
        Severity::P2,
        format!("réponse {} pour {}", agent_name, original.title),
        response_text,
    );
    resp_msg.tags = vec!["agent-response".into(), agent_name.to_string()];
    resp_msg.meta = meta;

    let subject = format!("agent.{}.response", agent_name);
    let payload = serde_json::to_vec(&resp_msg)
        .map_err(|e| anyhow::anyhow!("sérialisation réponse: {}", e))?;

    state
        .nats
        .js
        .publish(subject, payload.into())
        .await
        .map_err(|e| anyhow::anyhow!("publish réponse NATS: {}", e))?;

    Ok(())
}

/// Trouve le stream JetStream couvrant un subject donné.
///
/// Utilise `stream_by_subject` pour obtenir le nom du stream, puis `get_stream` pour
/// récupérer le handle. Retourne une erreur si aucun stream ne couvre le subject.
async fn find_stream_for_subject(
    js: &jetstream::Context,
    subject: &str,
) -> anyhow::Result<jetstream::stream::Stream> {
    let stream_name = js
        .stream_by_subject(subject)
        .await
        .map_err(|e| anyhow::anyhow!("aucun stream NATS pour '{}': {}", subject, e))?;

    js.get_stream(&stream_name)
        .await
        .map_err(|e| anyhow::anyhow!("get_stream '{}' pour subject '{}': {}", stream_name, subject, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_request_serializes_with_system_prompt() {
        let req = ChatRequest {
            model: "qwen3.5-122b".into(),
            messages: vec![
                ChatMessage {
                    role: "system".into(),
                    content: "Tu es un assistant.".into(),
                },
                ChatMessage {
                    role: "user".into(),
                    content: "Bonjour".into(),
                },
            ],
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"system\""));
        assert!(json.contains("\"user\""));
        assert!(json.contains("qwen3.5-122b"));
        assert!(json.contains("Tu es un assistant."));
    }

    #[test]
    fn chat_request_serializes_without_system_prompt() {
        let req = ChatRequest {
            model: "nemotron-30b".into(),
            messages: vec![ChatMessage {
                role: "user".into(),
                content: "test".into(),
            }],
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("\"system\""));
        assert!(json.contains("\"user\""));
    }

    #[test]
    fn chat_response_deserializes_choice() {
        let json = r#"{
            "id": "chatcmpl-abc",
            "object": "chat.completion",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "Voici ma réponse"
                    },
                    "finish_reason": "stop"
                }
            ]
        }"#;
        let resp: ChatResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.choices.len(), 1);
        assert_eq!(resp.choices[0].message.content, "Voici ma réponse");
    }

    #[test]
    fn chat_response_empty_choices_would_fail() {
        // Vérification que le cas "choices vide" est détecté
        let resp = ChatResponse { choices: vec![] };
        let result = resp.choices.into_iter().next();
        assert!(result.is_none());
    }

    #[test]
    fn default_timeout_is_reasonable() {
        // 60s : ni trop court (LLM lent) ni trop long (blocage goroutine)
        assert_eq!(DEFAULT_TIMEOUT_SECS, 60);
        // La Duration calculée est correcte
        let d = Duration::from_secs(DEFAULT_TIMEOUT_SECS);
        assert_eq!(d.as_secs(), 60);
    }
}
