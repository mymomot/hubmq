// Sink email — SMTP STARTTLS (Gmail port 587) avec templates Tera embarqués.
//
// Utilise `lettre::SmtpTransport` (synchrone) dans `spawn_blocking` pour ne pas
// bloquer le runtime Tokio pendant la négociation TLS/SMTP.
use super::Sink;
use crate::{config::SmtpConfig, message::Message};
use anyhow::Context as _;
use async_trait::async_trait;
use lettre::{
    message::Mailbox,
    transport::smtp::{authentication::Credentials, SmtpTransport},
    Message as Mail, Transport,
};
use tera::Tera;

/// Subject minimaliste : `[HubMQ P0] Titre du message`.
const SUBJECT_TMPL: &str = "[HubMQ {{ severity }}] {{ title }}";

/// Corps en texte brut multi-ligne.
const BODY_TMPL: &str = r#"HubMQ alert from {{ source }}

Severity: {{ severity }}
Timestamp: {{ ts }}
ID: {{ id }}

{{ body }}

-- tags: {{ tags }}
-- HubMQ LXC 415"#;

/// Canal email via SMTP Gmail (STARTTLS port 587).
///
/// Le transport lettre est `Clone` car il emballe un `Arc` interne.
pub struct EmailSink {
    /// Transport SMTP réutilisable (thread-safe par clone).
    transport: SmtpTransport,
    from: Mailbox,
    to: Mailbox,
    /// Moteur de templates Tera initialisé avec `subject` et `body`.
    tera: Tera,
}

impl EmailSink {
    /// Construit le sink en ouvrant la connexion SMTP avec les credentials fournis.
    ///
    /// `password` est le mot de passe d'application Gmail (secret résolu au runtime).
    /// `to` est l'adresse de destination (ex : `"motreff@gmail.com"`).
    ///
    /// # Erreurs
    /// - Parsing d'adresse email invalide
    /// - Impossible de joindre le relais STARTTLS
    /// - Echec de compilation des templates Tera
    pub fn new(cfg: &SmtpConfig, password: &str, to: &str) -> anyhow::Result<Self> {
        let creds = Credentials::new(cfg.username.clone(), password.to_string());
        let transport = SmtpTransport::starttls_relay(&cfg.host)
            .context("init SMTP STARTTLS relay")?
            .port(cfg.port)
            .credentials(creds)
            .build();
        let from: Mailbox = cfg.from.parse().context("parse SMTP from address")?;
        let to: Mailbox = to.parse().context("parse SMTP to address")?;
        let mut tera = Tera::default();
        tera.add_raw_template("subject", SUBJECT_TMPL)
            .context("compile subject template")?;
        tera.add_raw_template("body", BODY_TMPL)
            .context("compile body template")?;
        Ok(Self { transport, from, to, tera })
    }
}

#[async_trait]
impl Sink for EmailSink {
    fn name(&self) -> &'static str {
        "email"
    }

    /// Envoie un email via SMTP Gmail en texte brut.
    ///
    /// La sévérité est formatée en `Debug` (`P0`, `P1`, …) pour les templates.
    /// L'envoi SMTP bloquant est délégué à `spawn_blocking` pour libérer le runtime.
    async fn deliver(&self, m: &Message) -> anyhow::Result<()> {
        let mut ctx = tera::Context::new();
        ctx.insert("severity", &format!("{:?}", m.severity));
        ctx.insert("title", &m.title);
        ctx.insert("body", &m.body);
        ctx.insert("source", &m.source);
        ctx.insert("ts", &m.ts.to_rfc3339());
        ctx.insert("id", &m.id.to_string());
        ctx.insert("tags", &m.tags.join(", "));

        let subject = self.tera.render("subject", &ctx).context("render subject")?;
        let body_text = self.tera.render("body", &ctx).context("render body")?;

        let mail = Mail::builder()
            .from(self.from.clone())
            .to(self.to.clone())
            .subject(subject)
            .body(body_text)
            .context("build lettre Message")?;

        // lettre::SmtpTransport est synchrone — déléguer à spawn_blocking
        // pour ne pas bloquer le runtime Tokio pendant TLS+SMTP.
        let transport = self.transport.clone();
        tokio::task::spawn_blocking(move || transport.send(&mail))
            .await
            .context("spawn_blocking join")?
            .context("smtp send")?;

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

    #[tokio::test]
    #[ignore = "requires Gmail credentials, run manually: GMAIL_APP_PASSWORD=xxx cargo test real_gmail_send -- --ignored"]
    async fn real_gmail_send() {
        let pwd = std::env::var("GMAIL_APP_PASSWORD").expect("set GMAIL_APP_PASSWORD");
        let cfg = SmtpConfig {
            host: "smtp.gmail.com".into(),
            port: 587,
            username: "mymomot74@gmail.com".into(),
            from: "HubMQ <mymomot74@gmail.com>".into(),
            password_credential: "gmail-app-password".into(),
        };
        let sink = EmailSink::new(&cfg, &pwd, "motreff@gmail.com").unwrap();
        let m = Message::new("test", Severity::P2, "hubmq integration test", "test body");
        sink.deliver(&m).await.unwrap();
    }
}
