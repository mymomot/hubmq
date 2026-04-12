use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Severity {
    P0, // critique, multi-canal, bypasse les quiet hours
    P1, // important, Telegram + push, bypasse les quiet hours
    P2, // info, digest email
    P3, // debug, log seulement
}

impl Severity {
    /// Mappe le niveau d'alerte Wazuh (0-15) vers la sévérité HubMQ.
    /// Wazuh : 12+ critique, 8-11 élevé, 5-7 moyen, <5 faible.
    pub fn from_wazuh_level(level: u8) -> Self {
        match level {
            12.. => Severity::P0,
            8..=11 => Severity::P1,
            5..=7 => Severity::P2,
            _ => Severity::P3,
        }
    }

    /// D5 : P0 et P1 bypassent les quiet hours.
    pub fn bypasses_quiet_hours(self) -> bool {
        matches!(self, Severity::P0 | Severity::P1)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: Uuid,
    pub ts: DateTime<Utc>,
    pub source: String,
    pub severity: Severity,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub dedup_key: Option<String>,
    #[serde(default)]
    pub meta: BTreeMap<String, String>,
}

impl Message {
    /// Construit un nouveau message avec id et timestamp générés automatiquement.
    pub fn new(
        source: impl Into<String>,
        severity: Severity,
        title: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            ts: Utc::now(),
            source: source.into(),
            severity,
            title: title.into(),
            body: body.into(),
            tags: vec![],
            dedup_key: None,
            meta: Default::default(),
        }
    }

    /// Hash stable pour déduplication.
    /// Utilise dedup_key si présent, sinon source+title.
    pub fn dedup_hash(&self) -> String {
        use sha2::{Digest, Sha256};
        let key = self
            .dedup_key
            .clone()
            .unwrap_or_else(|| format!("{}::{}", self.source, self.title));
        let mut h = Sha256::new();
        h.update(key.as_bytes());
        format!("{:x}", h.finalize())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn serializes_message_roundtrip() {
        let m = Message {
            id: uuid::Uuid::nil(),
            ts: chrono::Utc.with_ymd_and_hms(2026, 4, 12, 12, 0, 0).unwrap(),
            source: "wazuh".into(),
            severity: Severity::P0,
            title: "critical alert".into(),
            body: "service down".into(),
            tags: vec!["wazuh".into(), "critical".into()],
            dedup_key: Some("wazuh:forge-lxc500:5502".into()),
            meta: Default::default(),
        };
        let json = serde_json::to_string(&m).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back.severity, Severity::P0);
        assert_eq!(back.tags.len(), 2);
    }

    #[test]
    fn severity_from_wazuh_level() {
        assert_eq!(Severity::from_wazuh_level(14), Severity::P0);
        assert_eq!(Severity::from_wazuh_level(10), Severity::P1);
        assert_eq!(Severity::from_wazuh_level(6), Severity::P2);
        assert_eq!(Severity::from_wazuh_level(3), Severity::P3);
    }

    #[test]
    fn severity_bypasses_quiet_hours() {
        assert!(Severity::P0.bypasses_quiet_hours());
        assert!(Severity::P1.bypasses_quiet_hours());
        assert!(!Severity::P2.bypasses_quiet_hours());
        assert!(!Severity::P3.bypasses_quiet_hours());
    }
}
