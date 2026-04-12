use crate::{
    audit::Audit,
    config::Config,
    filter::{dedup::DedupCache, ratelimit::AdaptiveRateLimiter, severity::SeverityRouter},
    nats_conn::NatsConn,
    queue::Queue,
};
use std::sync::Arc;

/// État partagé entre tous les handlers Axum.
///
/// Tous les champs sont `Arc` — le clone est O(1) et thread-safe.
/// Axum requiert `Clone` sur le state pour le distribuer aux workers.
#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub nats: Arc<NatsConn>,
    pub queue: Arc<Queue>,
    pub audit: Arc<Audit>,
    pub dedup: Arc<DedupCache>,
    pub rate: Arc<AdaptiveRateLimiter>,
    pub router: Arc<SeverityRouter>,
}
