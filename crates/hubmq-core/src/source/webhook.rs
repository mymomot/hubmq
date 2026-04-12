/// Routes HTTP d'ingestion HubMQ — generic, Wazuh, Forgejo.
///
/// Chaque handler appelle `publish` qui enchaîne :
///   1. déduplication (check_and_record)
///   2. rate limiting (allow)
///   3. publication NATS JetStream
///   4. enqueue SQLite
///   5. audit log
///
/// Codes de retour :
///   202 Accepted        — message publié ou dédoublonné
///   400 Bad Request     — payload JSON invalide
///   429 Too Many Requests — rate limit atteint
///   503 Service Unavailable — publication NATS échouée
use crate::{
    app_state::AppState,
    message::{Message, Severity},
    subjects::Subject,
};
use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::Value;

/// Construit le routeur Axum avec les 4 routes d'ingestion.
pub fn routes(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/in/generic", post(ingest_generic))
        .route("/in/wazuh", post(ingest_wazuh))
        .route("/in/forgejo", post(ingest_forgejo))
        .with_state(state)
}

async fn health() -> &'static str {
    "ok"
}

/// Payload d'ingestion générique — source et sévérité explicites.
#[derive(Deserialize)]
pub struct GenericIn {
    pub source: String,
    pub severity: Severity,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub dedup_key: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// POST /in/generic — ingestion d'une alerte quelconque.
async fn ingest_generic(
    State(s): State<AppState>,
    Json(p): Json<GenericIn>,
) -> Result<StatusCode, StatusCode> {
    let mut m = Message::new(p.source, p.severity, p.title, p.body);
    m.dedup_key = p.dedup_key;
    m.tags = p.tags;
    publish(&s, m).await
}

// --- Wazuh ---

#[derive(Deserialize)]
struct WazuhAlert {
    rule: WazuhRule,
    agent: WazuhAgent,
    full_log: String,
}

#[derive(Deserialize)]
struct WazuhRule {
    level: u8,
    description: String,
    id: u32,
}

#[derive(Deserialize)]
struct WazuhAgent {
    name: String,
}

/// POST /in/wazuh — reçoit un payload Wazuh et le traduit en Message HubMQ.
///
/// La sévérité est dérivée de `rule.level` via `Severity::from_wazuh_level`.
/// La dedup_key est `wazuh:{agent.name}:{rule.id}`.
async fn ingest_wazuh(
    State(s): State<AppState>,
    Json(p): Json<Value>,
) -> Result<StatusCode, StatusCode> {
    let alert: WazuhAlert =
        serde_json::from_value(p).map_err(|_| StatusCode::BAD_REQUEST)?;
    let sev = Severity::from_wazuh_level(alert.rule.level);
    let mut m = Message::new(
        format!("wazuh:{}", alert.agent.name),
        sev,
        alert.rule.description,
        alert.full_log,
    );
    m.dedup_key = Some(format!("wazuh:{}:{}", alert.agent.name, alert.rule.id));
    m.tags = vec!["wazuh".into(), Subject::from_severity(sev).into()];
    publish(&s, m).await
}

// --- Forgejo ---

/// POST /in/forgejo — webhook Forgejo CI.
///
/// Si `workflow_run.conclusion == "success"` → 204 No Content (succès ignoré).
/// Sinon construit un Message P1 avec repo + workflow + url.
async fn ingest_forgejo(
    State(s): State<AppState>,
    Json(p): Json<Value>,
) -> Result<StatusCode, StatusCode> {
    let conclusion = p
        .get("workflow_run")
        .and_then(|w| w.get("conclusion"))
        .and_then(|c| c.as_str())
        .unwrap_or("unknown");

    if conclusion == "success" {
        return Ok(StatusCode::NO_CONTENT);
    }

    let repo = p
        .get("repository")
        .and_then(|r| r.get("full_name"))
        .and_then(|s| s.as_str())
        .unwrap_or("?");
    let workflow = p
        .get("workflow_run")
        .and_then(|w| w.get("name"))
        .and_then(|s| s.as_str())
        .unwrap_or("?");
    let html_url = p
        .get("workflow_run")
        .and_then(|w| w.get("html_url"))
        .and_then(|s| s.as_str())
        .unwrap_or("");

    let mut m = Message::new(
        format!("forgejo:{}", repo),
        Severity::P1,
        format!("CI failed: {} — {}", repo, workflow),
        format!(
            "Workflow {} on {} concluded: {}\n{}",
            workflow, repo, conclusion, html_url
        ),
    );
    m.dedup_key = Some(format!("forgejo:{}:{}", repo, workflow));
    m.tags = vec!["forgejo".into(), "ci".into()];
    publish(&s, m).await
}

// --- Pipeline de publication ---

/// Pipeline complet : dedup → rate limit → NATS → queue → audit.
///
/// Retourne 202 si publié ou dédoublonné, 429 si rate limit, 503 si NATS fail.
async fn publish(s: &AppState, m: Message) -> Result<StatusCode, StatusCode> {
    // 1. Déduplication
    let h = m.dedup_hash();
    if s.dedup.check_and_record(&h) {
        s.audit
            .log(
                "message_deduped",
                Some(&m.source),
                Some(&m.title),
                &serde_json::json!({"hash": h}),
            )
            .await
            .ok();
        return Ok(StatusCode::ACCEPTED);
    }

    // 2. Rate limit
    if !s.rate.allow(&m.source, m.severity) {
        s.audit
            .log(
                "message_rate_limited",
                Some(&m.source),
                Some(&m.title),
                &serde_json::json!({}),
            )
            .await
            .ok();
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }

    // 3. Sujet NATS — identifiant fixe, jamais de texte utilisateur (D8)
    let subject = if m.source.starts_with("wazuh") {
        Subject::alert_wazuh(Subject::from_severity(m.severity))
    } else if m.source.starts_with("forgejo") {
        Subject::alert_forgejo_failed().to_string()
    } else {
        format!(
            "alert.{}.{}",
            sanitize(&m.source),
            Subject::from_severity(m.severity)
        )
    };

    // 4. Publication NATS
    let payload = serde_json::to_vec(&m).map_err(|e| {
        tracing::error!(err = ?e, "serialize message échouée");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    s.nats
        .js
        .publish(subject.clone(), payload.into())
        .await
        .map_err(|e| {
            tracing::error!(err = ?e, subject, "nats publish échouée");
            StatusCode::SERVICE_UNAVAILABLE
        })?;

    // 5. Enqueue SQLite (non-bloquant sur erreur)
    s.queue.enqueue(&m).await.ok();

    // 6. Audit
    s.audit
        .log(
            "message_ingested",
            Some(&m.source),
            Some(&subject),
            &serde_json::json!({"id": m.id.to_string()}),
        )
        .await
        .ok();

    Ok(StatusCode::ACCEPTED)
}

/// Sanitise un identifiant de source pour usage dans un subject NATS.
///
/// Conserve `[a-z0-9_-]`, remplace tout autre caractère par `-`, met en minuscule.
/// Ex : `"my.source@v2"` → `"my-source-v2"`.
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::sanitize;

    #[test]
    fn sanitize_remplace_caracteres_speciaux() {
        assert_eq!(sanitize("my.source@v2"), "my-source-v2");
        assert_eq!(sanitize("Wazuh-LXC 500"), "wazuh-lxc-500");
        assert_eq!(sanitize("ok_source-1"), "ok_source-1");
        assert_eq!(sanitize(""), "");
    }
}
