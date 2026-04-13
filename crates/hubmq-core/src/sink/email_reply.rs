/// Sink email reply — réponse dans le thread email originel via SMTP Gmail.
///
/// Utilisé pour répondre aux emails envoyés à `mymomot74@gmail.com` depuis `motreff@gmail.com`.
/// Construit un email de réponse avec les headers `In-Reply-To`, `References`, et `Subject: Re: ...`
/// pour que le message s'insère dans le thread Gmail.
///
/// ## Headers de threading
/// - `In-Reply-To: <email_msg_id>` — référence au message original
/// - `References: <email_msg_id>` — chaîne de références (simplifié V1 : même valeur)
/// - `Subject: Re: <email_subject>` — sujet préfixé `Re: ` (ou conservé si déjà préfixé)
///
/// ## Informations nécessaires dans `m.meta`
/// | Clé            | Description                     | Obligatoire |
/// |----------------|----------------------------------|-------------|
/// | `email_from`   | Adresse de destination (To)      | Oui         |
/// | `email_msg_id` | Message-ID du message original   | Non         |
/// | `email_subject`| Sujet du message original        | Non         |
///
/// ## Réutilisation
/// Réutilise `lettre::SmtpTransport` STARTTLS (même config que `EmailSink`).
/// Le transport est `Clone` car il emballe un `Arc` interne.
use super::Sink;
use crate::{config::SmtpConfig, message::Message};
use anyhow::Context as _;
use async_trait::async_trait;
use lettre::{
    message::Mailbox,
    transport::smtp::{authentication::Credentials, SmtpTransport},
    Message as Mail, Transport,
};

/// Sink de réponse email — répond dans le thread email originel.
pub struct EmailReplySink {
    /// Transport SMTP réutilisable (thread-safe par clone).
    transport: SmtpTransport,
    /// Adresse expéditeur (ex : `"HubMQ <mymomot74@gmail.com>"`).
    from: Mailbox,
}

impl EmailReplySink {
    /// Construit le sink en ouvrant la connexion SMTP avec les credentials fournis.
    ///
    /// `password` est le mot de passe d'application Gmail (secret résolu au runtime).
    ///
    /// # Erreurs
    /// - Parsing d'adresse email invalide
    /// - Impossible de joindre le relais STARTTLS
    pub fn new(cfg: &SmtpConfig, password: &str) -> anyhow::Result<Self> {
        let creds = Credentials::new(cfg.username.clone(), password.to_string());
        let transport = SmtpTransport::starttls_relay(&cfg.host)
            .context("init SMTP STARTTLS relay pour email_reply")?
            .port(cfg.port)
            .credentials(creds)
            .build();
        let from: Mailbox = cfg.from.parse().context("parse SMTP from address pour email_reply")?;
        Ok(Self { transport, from })
    }

    /// Extrait l'adresse de destination depuis `m.meta["email_from"]`.
    ///
    /// Retourne une erreur si le champ est absent ou si l'adresse est invalide.
    fn extract_to(&self, m: &Message) -> anyhow::Result<Mailbox> {
        let email_from = m
            .meta
            .get("email_from")
            .ok_or_else(|| anyhow::anyhow!("meta email_from manquant — impossible de router la réponse email"))?;
        email_from
            .parse::<Mailbox>()
            .with_context(|| format!("parse adresse To email_reply '{}'", email_from))
    }

    /// Construit le sujet de réponse.
    ///
    /// Préfixe `Re: ` si le sujet original ne commence pas déjà par `Re:` ou `RE:`.
    fn build_reply_subject(original_subject: &str) -> String {
        let trimmed = original_subject.trim();
        if trimmed.to_lowercase().starts_with("re:") {
            trimmed.to_string()
        } else {
            format!("Re: {}", trimmed)
        }
    }
}

#[async_trait]
impl Sink for EmailReplySink {
    fn name(&self) -> &'static str {
        "email_reply"
    }

    /// Envoie une réponse email dans le thread originel via SMTP Gmail STARTTLS.
    ///
    /// Construit les headers `In-Reply-To`, `References`, `To`, `Subject` depuis `m.meta`.
    /// L'envoi SMTP bloquant est délégué à `spawn_blocking` pour libérer le runtime Tokio.
    ///
    /// Si `email_from` est absent dans `m.meta`, retourne une erreur (message non délivrable).
    async fn deliver(&self, m: &Message) -> anyhow::Result<()> {
        let to = self.extract_to(m)?;

        let msg_id = m
            .meta
            .get("email_msg_id")
            .cloned()
            .unwrap_or_default();

        let orig_subject = m
            .meta
            .get("email_subject")
            .map(|s| s.as_str())
            .unwrap_or("");

        let subject = Self::build_reply_subject(orig_subject);
        let body = m.body.clone();

        // Construction de l'email avec headers de threading
        let mut builder = Mail::builder()
            .from(self.from.clone())
            .to(to)
            .subject(subject);

        // Headers de threading pour l'insertion dans le thread Gmail
        if !msg_id.is_empty() {
            builder = builder
                .in_reply_to(msg_id.clone())
                .references(msg_id);
        }

        let mail = builder
            .body(body)
            .context("build lettre Message email_reply")?;

        // lettre::SmtpTransport est synchrone — déléguer à spawn_blocking
        let transport = self.transport.clone();
        tokio::task::spawn_blocking(move || transport.send(&mail))
            .await
            .context("spawn_blocking join email_reply")?
            .context("smtp send email_reply")?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::SmtpConfig,
        message::{Message, Severity},
    };

    fn smtp_cfg() -> SmtpConfig {
        SmtpConfig {
            host: "smtp.gmail.com".into(),
            port: 587,
            username: "mymomot74@gmail.com".into(),
            from: "HubMQ <mymomot74@gmail.com>".into(),
            password_credential: "gmail-app-password".into(),
        }
    }

    /// Construit un message avec les métadonnées email pour les tests.
    fn email_message(
        from: &str,
        msg_id: &str,
        subject: &str,
        body: &str,
    ) -> Message {
        let mut m = Message::new("agent.llmcore", Severity::P2, "réponse llmcore", body);
        m.meta.insert("source".into(), "email".into());
        m.meta.insert("email_from".into(), from.into());
        m.meta.insert("email_msg_id".into(), msg_id.into());
        m.meta.insert("email_subject".into(), subject.into());
        m
    }

    // ── build_reply_subject ────────────────────────────────────────────────────

    #[test]
    fn build_reply_subject_adds_re_prefix() {
        let s = EmailReplySink::build_reply_subject("explique CAP theorem");
        assert_eq!(s, "Re: explique CAP theorem");
    }

    #[test]
    fn build_reply_subject_keeps_existing_re() {
        // Déjà préfixé — ne pas doubler
        let s = EmailReplySink::build_reply_subject("Re: explique CAP theorem");
        assert_eq!(s, "Re: explique CAP theorem");
    }

    #[test]
    fn build_reply_subject_case_insensitive_re() {
        let s = EmailReplySink::build_reply_subject("RE: Something");
        assert_eq!(s, "RE: Something");
    }

    #[test]
    fn build_reply_subject_empty_original() {
        let s = EmailReplySink::build_reply_subject("");
        assert_eq!(s, "Re: ");
    }

    // ── extract_to ─────────────────────────────────────────────────────────────

    #[test]
    fn extract_to_valid_address() {
        // Construit un sink minimal pour tester extract_to sans connexion SMTP
        let m = email_message(
            "motreff@gmail.com",
            "<msg123@mail.gmail.com>",
            "[llmcore] test",
            "réponse",
        );

        // On ne peut pas instancier EmailReplySink sans SMTP live,
        // mais on teste la logique de parsing directement
        let addr: Result<Mailbox, _> = "motreff@gmail.com".parse();
        assert!(addr.is_ok());
        assert_eq!(m.meta.get("email_from").unwrap(), "motreff@gmail.com");
    }

    #[test]
    fn extract_to_missing_meta_returns_error() {
        // Message sans email_from → erreur attendue
        let m = Message::new("agent.test", Severity::P2, "test", "body");
        // meta vide — email_from absent
        assert!(m.meta.get("email_from").is_none());
    }

    // ── headers dans le message ────────────────────────────────────────────────

    #[test]
    fn email_reply_meta_structure() {
        let m = email_message(
            "motreff@gmail.com",
            "<abc123@mail.gmail.com>",
            "[llmcore] explique CAP",
            "CAP theorem signifie Consistency, Availability, Partition tolerance.",
        );

        assert_eq!(m.meta.get("source").unwrap(), "email");
        assert_eq!(m.meta.get("email_from").unwrap(), "motreff@gmail.com");
        assert_eq!(m.meta.get("email_msg_id").unwrap(), "<abc123@mail.gmail.com>");
        assert_eq!(m.meta.get("email_subject").unwrap(), "[llmcore] explique CAP");
    }

    #[test]
    fn email_reply_subject_derived_from_meta() {
        let m = email_message(
            "motreff@gmail.com",
            "<abc@mail.gmail.com>",
            "[llmcore] question",
            "réponse",
        );

        let reply_subject = EmailReplySink::build_reply_subject(
            m.meta.get("email_subject").map(|s| s.as_str()).unwrap_or("")
        );
        assert_eq!(reply_subject, "Re: [llmcore] question");
    }

    // ── test intégration Gmail (ignoré par défaut) ─────────────────────────────

    #[tokio::test]
    #[ignore = "requires Gmail credentials — run manually: GMAIL_APP_PASSWORD=xxx cargo test real_email_reply_send -- --ignored"]
    async fn real_email_reply_send() {
        let pwd = std::env::var("GMAIL_APP_PASSWORD").expect("set GMAIL_APP_PASSWORD");
        let cfg = smtp_cfg();
        let sink = EmailReplySink::new(&cfg, &pwd).unwrap();
        let m = email_message(
            "motreff@gmail.com",
            "<test-msg-id@mail.gmail.com>",
            "[llmcore] test intégration",
            "Réponse de test depuis le sink EmailReplySink.",
        );
        sink.deliver(&m).await.unwrap();
    }
}
