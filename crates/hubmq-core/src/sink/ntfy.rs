// Sink ntfy — HTTP POST vers un serveur ntfy (LAN only en Phase Core).
//
// Authentification Bearer optionnelle.
// Priority mapping : P0→5 (max), P1→4, P2→3, P3→2 (min).
use super::Sink;
use crate::message::{Message, Severity};
use async_trait::async_trait;
use reqwest::Client;

/// Canal ntfy.sh via HTTP POST.
///
/// Le client reqwest est réutilisé entre appels (pool de connexions).
pub struct NtfySink {
    /// URL de base du serveur ntfy (ex : `http://192.168.10.x:8080`).
    base_url: String,
    /// Topic ntfy cible (ex : `hubmq-alerts`).
    topic: String,
    /// Token Bearer optionnel pour l'authentification ntfy.
    bearer: Option<String>,
    client: Client,
}

impl NtfySink {
    /// Construit le sink ntfy.
    ///
    /// `bearer` : token secret résolu au runtime depuis le gestionnaire de secrets.
    pub fn new(
        base_url: impl Into<String>,
        topic: impl Into<String>,
        bearer: Option<String>,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            topic: topic.into(),
            bearer,
            client: Client::new(),
        }
    }

    /// Mappe la sévérité HubMQ vers la priorité ntfy (1–5).
    ///
    /// | HubMQ | ntfy | Signification ntfy |
    /// |-------|------|--------------------|
    /// | P0    | 5    | max — push immédiat |
    /// | P1    | 4    | high |
    /// | P2    | 3    | default |
    /// | P3    | 2    | low |
    fn priority(severity: Severity) -> &'static str {
        match severity {
            Severity::P0 => "5",
            Severity::P1 => "4",
            Severity::P2 => "3",
            Severity::P3 => "2",
        }
    }
}

#[async_trait]
impl Sink for NtfySink {
    fn name(&self) -> &'static str {
        "ntfy"
    }

    /// Envoie une notification ntfy via HTTP POST.
    ///
    /// Headers ntfy utilisés :
    /// - `Title` : titre du message
    /// - `Priority` : entier 2–5 mappé depuis la sévérité
    /// - `Tags` : liste de tags séparés par virgule
    ///
    /// # Erreurs
    /// - Erreur réseau reqwest
    /// - Réponse HTTP non-success (4xx/5xx)
    async fn deliver(&self, m: &Message) -> anyhow::Result<()> {
        let url = format!(
            "{}/{}",
            self.base_url.trim_end_matches('/'),
            self.topic
        );

        let mut req = self
            .client
            .post(&url)
            .header("Title", &m.title)
            .header("Priority", Self::priority(m.severity))
            .header("Tags", m.tags.join(","))
            .body(m.body.clone());

        if let Some(token) = &self.bearer {
            req = req.bearer_auth(token);
        }

        let resp = req.send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("ntfy HTTP {}", resp.status());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{Message, Severity};

    #[test]
    fn priority_mapping() {
        assert_eq!(NtfySink::priority(Severity::P0), "5");
        assert_eq!(NtfySink::priority(Severity::P1), "4");
        assert_eq!(NtfySink::priority(Severity::P2), "3");
        assert_eq!(NtfySink::priority(Severity::P3), "2");
    }

    #[test]
    fn sink_name() {
        let sink = NtfySink::new("http://localhost", "test", None);
        assert_eq!(sink.name(), "ntfy");
    }

    #[test]
    fn url_construction_trims_slash() {
        let sink = NtfySink::new("http://ntfy.local/", "alerts", None);
        let expected = "http://ntfy.local/alerts";
        let actual = format!(
            "{}/{}",
            sink.base_url.trim_end_matches('/'),
            sink.topic
        );
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    #[ignore = "requires a running ntfy server, run manually"]
    async fn real_ntfy_send() {
        let sink = NtfySink::new("http://localhost:8080", "hubmq-test", None);
        let m = Message::new("test", Severity::P1, "test ntfy", "integration test");
        sink.deliver(&m).await.unwrap();
    }
}
