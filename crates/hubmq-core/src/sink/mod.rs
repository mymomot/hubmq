// Trait commun pour tous les canaux de livraison HubMQ.
use crate::message::Message;
use async_trait::async_trait;

pub mod apprise;
pub mod email;
pub mod ntfy;

/// Canal de livraison : tout backend de notification implémente ce trait.
///
/// # Effets de bord
/// - `deliver` effectue un appel réseau ou un sous-processus externe.
/// - Les erreurs sont propagées à l'appelant pour gestion (retry, dead-letter, log).
#[async_trait]
pub trait Sink: Send + Sync {
    /// Livre le message `m` sur le canal.
    ///
    /// Retourne `Ok(())` si la livraison est confirmée par le backend distant,
    /// ou une erreur décrivant l'échec (HTTP, SMTP, subprocess exit non-zéro).
    async fn deliver(&self, m: &Message) -> anyhow::Result<()>;

    /// Identifiant lisible du canal (ex : `"email"`, `"ntfy"`, `"apprise"`).
    fn name(&self) -> &'static str;
}
