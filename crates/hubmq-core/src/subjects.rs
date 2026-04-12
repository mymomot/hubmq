/// D8 — subject builders. Identifiants fixes, jamais de texte utilisateur en subject NATS.
use crate::message::Severity;

pub struct Subject;

impl Subject {
    /// alert.wazuh.<level_category> — level_category doit être l'un de
    /// {"critical","high","medium","low"}. Utiliser `from_severity()` pour obtenir
    /// la catégorie depuis une `Severity`.
    pub fn alert_wazuh(level_category: &str) -> String {
        debug_assert!(
            is_safe_ident(level_category),
            "level_category contient des caractères interdits: {}",
            level_category
        );
        format!("alert.wazuh.{}", level_category)
    }

    pub fn alert_forgejo_failed() -> &'static str {
        "alert.forgejo.failed"
    }

    pub fn alert_forgejo_deploy_failed() -> &'static str {
        "alert.forgejo.deploy_failed"
    }

    /// monitor.heartbeat.<component> — component validé par `is_safe_ident`.
    ///
    /// # Panics
    /// Panique si `component` contient des points, wildcards ou caractères non-alnum.
    pub fn monitor_heartbeat(component: &str) -> String {
        assert!(
            is_safe_ident(component),
            "composant de subject invalide (D8): {}",
            component
        );
        format!("monitor.heartbeat.{}", component)
    }

    /// system.<unit>.failed — unit validé par `is_safe_ident`.
    ///
    /// # Panics
    /// Panique si `unit` contient des points, wildcards ou caractères non-alnum.
    pub fn system_failed(unit: &str) -> String {
        assert!(
            is_safe_ident(unit),
            "nom d'unité invalide (D8): {}",
            unit
        );
        format!("system.{}.failed", unit)
    }

    pub fn user_incoming_telegram() -> &'static str {
        "user.incoming.telegram"
    }

    /// Convertit une `Severity` en catégorie de niveau pour les subjects NATS.
    pub fn from_severity(sev: Severity) -> &'static str {
        match sev {
            Severity::P0 => "critical",
            Severity::P1 => "high",
            Severity::P2 => "medium",
            Severity::P3 => "low",
        }
    }
}

/// D8 — accepte uniquement [a-z0-9_-], longueur 1..=64.
/// Rejette les points (`.`), wildcards (`*`, `>`), et tout caractère non-alnum.
pub fn is_safe_ident(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Severity;

    #[test]
    fn safe_ident_rejects_dots_and_stars() {
        // chaîne vide
        assert!(!is_safe_ident(""));
        // point — séparateur NATS interdit
        assert!(!is_safe_ident("a.b"));
        // wildcard simple
        assert!(!is_safe_ident("a*"));
        // wildcard full
        assert!(!is_safe_ident("a>b"));
        // majuscules interdites
        assert!(!is_safe_ident("Wazuh"));
        // espace interdit
        assert!(!is_safe_ident("foo bar"));
        // trop long (65 chars)
        assert!(!is_safe_ident(&"a".repeat(65)));

        // cas valides
        assert!(is_safe_ident("wazuh-forge-lxc500"));
        assert!(is_safe_ident("hubmq"));
        assert!(is_safe_ident("lxc_500"));
        assert!(is_safe_ident(&"a".repeat(64)));
    }

    #[test]
    fn alert_wazuh_builder() {
        assert_eq!(Subject::alert_wazuh("critical"), "alert.wazuh.critical");
        assert_eq!(Subject::alert_wazuh("high"), "alert.wazuh.high");
        assert_eq!(Subject::alert_wazuh("medium"), "alert.wazuh.medium");
        assert_eq!(Subject::alert_wazuh("low"), "alert.wazuh.low");
    }

    #[test]
    fn static_subjects_correct() {
        assert_eq!(Subject::alert_forgejo_failed(), "alert.forgejo.failed");
        assert_eq!(
            Subject::alert_forgejo_deploy_failed(),
            "alert.forgejo.deploy_failed"
        );
        assert_eq!(Subject::user_incoming_telegram(), "user.incoming.telegram");
    }

    #[test]
    fn monitor_heartbeat_builder() {
        assert_eq!(
            Subject::monitor_heartbeat("hubmq"),
            "monitor.heartbeat.hubmq"
        );
    }

    #[test]
    fn system_failed_builder() {
        assert_eq!(Subject::system_failed("hubmq"), "system.hubmq.failed");
        assert_eq!(Subject::system_failed("nats"), "system.nats.failed");
    }

    #[test]
    fn from_severity_mapping() {
        assert_eq!(Subject::from_severity(Severity::P0), "critical");
        assert_eq!(Subject::from_severity(Severity::P1), "high");
        assert_eq!(Subject::from_severity(Severity::P2), "medium");
        assert_eq!(Subject::from_severity(Severity::P3), "low");
    }

    #[test]
    fn alert_wazuh_combined_with_severity() {
        let sev = Severity::P0;
        let subject = Subject::alert_wazuh(Subject::from_severity(sev));
        assert_eq!(subject, "alert.wazuh.critical");
    }
}
