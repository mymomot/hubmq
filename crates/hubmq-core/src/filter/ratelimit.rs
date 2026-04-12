// filter/ratelimit.rs — Limiteur de débit adaptatif avec bucket P0 séparé (D6).
//
// Design : token bucket par (source, tier) — P0 a son propre bucket à haute capacité
// pour ne jamais bloquer les alertes critiques quand le flux normal est saturé.
use crate::message::Severity;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

/// Bucket à jetons pour un (source, tier) donné.
struct Bucket {
    /// Nombre de jetons disponibles (valeur flottante pour la précision du refill).
    tokens: f64,
    /// Dernier instant de refill.
    last_refill: Instant,
    /// Capacité maximale du bucket.
    capacity: f64,
    /// Taux de refill en jetons par seconde.
    refill_per_sec: f64,
}

/// Limiteur de débit adaptatif.
///
/// Chaque source dispose de deux buckets indépendants :
/// - `source::normal` : capacité `normal_per_min`, partagé par P1/P2/P3
/// - `source::p0`     : capacité `p0_per_min`, exclusif P0
///
/// Thread-safe via Mutex sur la map des buckets.
pub struct AdaptiveRateLimiter {
    normal_per_min: u32,
    p0_per_min: u32,
    buckets: Mutex<HashMap<String, Bucket>>,
}

impl AdaptiveRateLimiter {
    /// Crée un limiteur avec les capacités par minute données.
    pub fn new(normal_per_min: u32, p0_per_min: u32) -> Self {
        Self {
            normal_per_min,
            p0_per_min,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Tente de consommer 1 jeton pour `source` à la sévérité `severity`.
    /// Retourne `true` si autorisé, `false` si le bucket est vide.
    ///
    /// D6 : P0 utilise un bucket séparé `source::p0` avec capacité `p0_per_min`.
    ///      Tous les autres utilisent `source::normal` avec capacité `normal_per_min`.
    pub fn allow(&self, source: &str, severity: Severity) -> bool {
        let (cap, refill) = match severity {
            Severity::P0 => {
                let c = self.p0_per_min as f64;
                (c, c / 60.0)
            }
            _ => {
                let c = self.normal_per_min as f64;
                (c, c / 60.0)
            }
        };

        let tier = match severity {
            Severity::P0 => "p0",
            _ => "normal",
        };
        let key = format!("{}::{}", source, tier);

        let mut buckets = self
            .buckets
            .lock()
            .expect("AdaptiveRateLimiter mutex empoisonné — état interne incohérent");

        let bucket = buckets.entry(key).or_insert_with(|| Bucket {
            tokens: cap,
            last_refill: Instant::now(),
            capacity: cap,
            refill_per_sec: refill,
        });

        // Refill proportionnel au temps écoulé depuis le dernier appel
        let now = Instant::now();
        let elapsed = now.duration_since(bucket.last_refill).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * bucket.refill_per_sec).min(bucket.capacity);
        bucket.last_refill = now;

        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_source_limited_to_10_per_min() {
        let rl = AdaptiveRateLimiter::new(10, 100);
        // Les 10 premiers passages sont autorisés
        for i in 0..10 {
            assert!(rl.allow("wazuh", Severity::P2), "passage {} refusé", i);
        }
        // Le 11e est bloqué — bucket vide
        assert!(!rl.allow("wazuh", Severity::P2));
    }

    #[test]
    fn p0_has_separate_higher_bucket() {
        let rl = AdaptiveRateLimiter::new(10, 100);
        // Épuise le bucket normal
        for _ in 0..10 {
            rl.allow("wazuh", Severity::P2);
        }
        assert!(!rl.allow("wazuh", Severity::P2), "bucket normal devrait être épuisé");
        // P0 a son propre bucket, toujours disponible
        assert!(rl.allow("wazuh", Severity::P0), "P0 doit avoir son bucket indépendant");
    }
}
