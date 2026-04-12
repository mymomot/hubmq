// Sink Apprise — délégation via sous-processus Python avec JSON sur stdin.
//
// Sécurité (SecurityAuditor V7 + V2) :
// - Aucune donnée utilisateur n'est passée en argv (injection argv impossible).
// - Seules les URLs de config (contrôlées) sont passées en positional args.
// - Le script Python, le titre et le body transitent par stdin JSON.
// - Le binaire `python3-apprise` est un venv dédié sur LXC 415, pas le python système.
use super::Sink;
use crate::message::Message;
use async_trait::async_trait;
use serde_json::json;
use tokio::io::AsyncWriteExt as _;
use tokio::process::Command;

/// Payload JSON passé sur stdin au script Python Apprise.
///
/// Le script Python embarqué lit ce JSON depuis `sys.stdin` :
/// - `title` : titre de la notification
/// - `body` : corps du message
/// - `tag` : tags concaténés par virgule (filtre Apprise optionnel)
///
/// Les URLs Apprise sont passées en argv[1:] — elles proviennent de la config
/// (valeurs contrôlées), jamais des données de messages entrants.
const PYTHON_SCRIPT: &str = r#"import sys, json, apprise
data = json.load(sys.stdin)
a = apprise.Apprise()
for url in sys.argv[1:]:
    a.add(url)
ok = a.notify(title=data.get("title", ""), body=data.get("body", ""), tag=data.get("tag", None))
sys.exit(0 if ok else 1)
"#;

/// Canal Apprise via sous-processus Python.
///
/// Apprise supporte 80+ services (Telegram, Slack, PagerDuty, etc.) via des URL schemes.
/// Ex : `"tgram://BOT_TOKEN/CHAT_ID"`, `"slack://T.../..."`.
pub struct AppriseSink {
    /// URLs Apprise (ex : `["tgram://TOKEN/CHAT_ID"]`).
    /// Proviennent exclusivement de la config — jamais de l'input externe.
    urls: Vec<String>,
}

impl AppriseSink {
    /// Construit le sink Apprise avec les URLs de destination.
    ///
    /// Les URLs ne doivent contenir que des valeurs de config connues.
    /// Elles ne doivent JAMAIS provenir d'input utilisateur externe.
    pub fn new(urls: Vec<String>) -> Self {
        Self { urls }
    }
}

#[async_trait]
impl Sink for AppriseSink {
    fn name(&self) -> &'static str {
        "apprise"
    }

    /// Livre le message via Apprise en spawning un sous-processus Python.
    ///
    /// Le payload `{title, body, tag}` est sérialisé en JSON et écrit sur stdin.
    /// Les URLs Apprise sont passées en argv (source config uniquement).
    ///
    /// # Erreurs
    /// - Échec de spawn du processus `python3-apprise`
    /// - Erreur d'écriture sur stdin
    /// - Exit code non-zéro du script Python (stderr inclus dans le message d'erreur)
    async fn deliver(&self, m: &Message) -> anyhow::Result<()> {
        // Note: `tag` param is NOT passed here. In Apprise, `tag` filters WHICH destinations
        // to notify (e.g. only URLs tagged "mobile"). Passing tags from messages would match
        // no destination and cause notify() to silently return false (exit 1).
        // Message tags are carried in title/body instead (handled upstream via Tera templates).
        let payload = json!({
            "title": m.title,
            "body": m.body,
        });

        // argv : ["-c", <script>, url1, url2, ...]
        // Les URLs viennent de la config — valeurs contrôlées, pas d'injection possible.
        let mut args: Vec<String> = vec!["-c".into(), PYTHON_SCRIPT.to_string()];
        for url in &self.urls {
            args.push(url.clone());
        }

        let mut child = Command::new("python3-apprise")
            .args(&args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

        // Écrire le JSON sur stdin puis fermer pour signaler EOF au script Python.
        if let Some(mut stdin) = child.stdin.take() {
            let bytes = serde_json::to_vec(&payload)?;
            stdin.write_all(&bytes).await?;
            stdin.shutdown().await?;
        }

        let out = child.wait_with_output().await?;
        if !out.status.success() {
            anyhow::bail!(
                "apprise subprocess failed (exit {}): {}",
                out.status
                    .code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "signal".into()),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sink_name() {
        let sink = AppriseSink::new(vec![]);
        assert_eq!(sink.name(), "apprise");
    }

    #[test]
    fn python_script_non_empty() {
        // Vérification que la constante n'est pas accidentellement vide.
        assert!(!PYTHON_SCRIPT.is_empty());
        assert!(PYTHON_SCRIPT.contains("apprise.Apprise()"));
        assert!(PYTHON_SCRIPT.contains("json.load(sys.stdin)"));
    }
}
