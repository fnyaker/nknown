//! Dates et fiabilité des dates.

use std::time::{SystemTime, UNIX_EPOCH};

/// Instant UTC, en secondes depuis l'époque Unix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(pub i64);

impl Timestamp {
    pub fn now() -> Self {
        Self::from_system(SystemTime::now()).unwrap_or(Self(0))
    }

    pub fn from_system(t: SystemTime) -> Option<Self> {
        let secs = t.duration_since(UNIX_EPOCH).ok()?.as_secs();
        i64::try_from(secs).ok().map(Self)
    }
}

/// Origine de la date retenue pour un contenu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DateConfidence {
    /// Fournie explicitement par la source (ex. `pubDate` RSS).
    Declared,
    /// Déduite d'un indice indirect (ex. date de modification d'un fichier).
    Inferred,
    /// Aucune date connue : seule la date de collecte est disponible.
    FetchTimeOnly,
}

/// Une date accompagnée de sa fiabilité.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Dated {
    pub at: Timestamp,
    pub confidence: DateConfidence,
}
