// filter/severity.rs — Routage par sévérité + quiet hours (D5).
//
// D5 : P0 bypasse tout | P1 bypasse les quiet hours | P2/P3 respectent la plage silencieuse.
// Wraparound géré : quiet_start > quiet_end signifie passage à minuit (ex. 23:00→06:00).
use crate::message::Severity;
use anyhow::Context;
use chrono::NaiveTime;

/// Canaux de livraison activés pour un message donné.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeliveryChannels {
    pub ntfy: bool,
    pub email: bool,
    pub telegram: bool,
    pub log_only: bool,
}

impl DeliveryChannels {
    fn all() -> Self {
        Self { ntfy: true, email: true, telegram: true, log_only: true }
    }

    fn ntfy_telegram() -> Self {
        Self { ntfy: true, email: false, telegram: true, log_only: false }
    }

    fn email_telegram() -> Self {
        Self { ntfy: false, email: true, telegram: true, log_only: false }
    }

    fn log_only() -> Self {
        Self { ntfy: false, email: false, telegram: false, log_only: true }
    }
}

/// Routeur de livraison basé sur la sévérité et l'heure courante.
///
/// Quiet hours : plage horaire où les alertes P2 sont supprimées sauf log.
/// P0 et P1 bypassent toujours les quiet hours (D5).
pub struct SeverityRouter {
    quiet_start: NaiveTime,
    quiet_end: NaiveTime,
}

impl SeverityRouter {
    /// Crée un routeur avec la plage silencieuse donnée.
    /// Format : "HH:MM" (ex. "23:00", "07:00").
    pub fn new(start_str: &str, end_str: &str) -> anyhow::Result<Self> {
        let quiet_start = NaiveTime::parse_from_str(start_str, "%H:%M")
            .with_context(|| format!("quiet_start invalide : {}", start_str))?;
        let quiet_end = NaiveTime::parse_from_str(end_str, "%H:%M")
            .with_context(|| format!("quiet_end invalide : {}", end_str))?;
        Ok(Self { quiet_start, quiet_end })
    }

    /// Retourne vrai si `t` est dans la plage silencieuse.
    ///
    /// Wraparound : si quiet_start > quiet_end (ex. 23:00→06:00),
    /// la plage couvre deux jours : t ≥ start || t < end.
    pub fn is_quiet_hours(&self, t: NaiveTime) -> bool {
        if self.quiet_start <= self.quiet_end {
            // Plage normale : ex. 01:00→07:00
            t >= self.quiet_start && t < self.quiet_end
        } else {
            // Wraparound minuit : ex. 23:00→06:00
            t >= self.quiet_start || t < self.quiet_end
        }
    }

    /// Détermine les canaux de livraison pour `sev` à l'heure `now_time`.
    ///
    /// Règles D5 :
    /// - P0 : tous les canaux, bypasse quiet hours
    /// - P1 : ntfy + telegram (pas email), bypasse quiet hours
    /// - P2 en quiet hours : log_only
    /// - P2 hors quiet hours : email + telegram
    /// - P3 : log_only (toujours)
    pub fn route(&self, sev: Severity, now_time: NaiveTime) -> DeliveryChannels {
        match sev {
            Severity::P0 => DeliveryChannels::all(),
            Severity::P1 => DeliveryChannels::ntfy_telegram(),
            Severity::P2 => {
                if self.is_quiet_hours(now_time) {
                    DeliveryChannels::log_only()
                } else {
                    DeliveryChannels::email_telegram()
                }
            }
            Severity::P3 => DeliveryChannels::log_only(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveTime;

    fn time(s: &str) -> NaiveTime {
        NaiveTime::parse_from_str(s, "%H:%M").unwrap()
    }

    #[test]
    fn p0_bypasses_quiet_hours() {
        // Quiet hours 23:00→07:00, on est en plein dedans
        let router = SeverityRouter::new("23:00", "07:00").unwrap();
        let ch = router.route(Severity::P0, time("02:00"));
        assert_eq!(ch, DeliveryChannels::all());
    }

    #[test]
    fn p2_suppressed_during_quiet_hours() {
        // Quiet hours 23:00→07:00 (wraparound), test à 01:30
        let router = SeverityRouter::new("23:00", "07:00").unwrap();
        let ch = router.route(Severity::P2, time("01:30"));
        assert_eq!(ch, DeliveryChannels::log_only());
    }

    #[test]
    fn p2_normal_hours_emails_and_telegram() {
        // Quiet hours 23:00→07:00, test à 14:00 (hors quiet)
        let router = SeverityRouter::new("23:00", "07:00").unwrap();
        let ch = router.route(Severity::P2, time("14:00"));
        assert_eq!(ch, DeliveryChannels::email_telegram());
    }
}
