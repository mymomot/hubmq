// filter/dedup.rs — Cache de déduplication avec fenêtre TTL.
// Stratégie : HashMap<hash, Instant> protégé par Mutex.
// GC des entrées expirées à chaque appel pour éviter la croissance mémoire illimitée.
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Cache de déduplication en mémoire avec fenêtre temporelle glissante.
///
/// Thread-safe via Mutex. GC automatique à chaque `check_and_record`.
pub struct DedupCache {
    window: Duration,
    seen: Mutex<HashMap<String, Instant>>,
}

impl DedupCache {
    /// Crée un cache avec une fenêtre de déduplication de `window_secs` secondes.
    pub fn new(window_secs: u64) -> Self {
        Self {
            window: Duration::from_secs(window_secs),
            seen: Mutex::new(HashMap::new()),
        }
    }

    /// Retourne `true` si ce hash a déjà été vu dans la fenêtre courante.
    /// Enregistre toujours le hash (upsert du timestamp).
    /// Effet de bord : purge les entrées expirées à chaque appel.
    pub fn check_and_record(&self, hash: &str) -> bool {
        let now = Instant::now();
        let mut map = self
            .seen
            .lock()
            .expect("DedupCache mutex empoisonné — état interne incohérent");

        // GC : supprimer les entrées plus anciennes que la fenêtre
        map.retain(|_, t| now.duration_since(*t) < self.window);

        let is_dup = map.contains_key(hash);
        map.insert(hash.to_string(), now);
        is_dup
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_duplicate_within_window() {
        let cache = DedupCache::new(60);
        // Premier appel : pas un doublon
        assert!(!cache.check_and_record("h1"));
        // Second appel dans la fenêtre : doublon détecté
        assert!(cache.check_and_record("h1"));
    }

    #[test]
    fn distinct_hashes_dont_collide() {
        let cache = DedupCache::new(60);
        // Deux hashes distincts ne se gênent pas
        assert!(!cache.check_and_record("h1"));
        assert!(!cache.check_and_record("h2"));
    }
}
