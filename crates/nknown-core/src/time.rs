//! Dates et fiabilité des dates.

use std::time::{SystemTime, UNIX_EPOCH};

/// Instant UTC, en secondes depuis l'époque Unix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(pub i64);

impl Timestamp {
    pub fn now() -> Self {
        Self::from_system(SystemTime::now()).unwrap_or(Self(0))
    }

    /// Date civile (année, mois, jour) en UTC.
    pub fn ymd(&self) -> (i64, u32, u32) {
        // Algorithme « days from civil » inversé (H. Hinnant).
        let z = self.0.div_euclid(86_400) + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        (yoe + era * 400 + i64::from(m <= 2), m, d)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates() {
        assert_eq!(Timestamp(0).ymd(), (1970, 1, 1));
        assert_eq!(Timestamp(951_782_400).ymd(), (2000, 2, 29));
        assert_eq!(Timestamp(1_793_620_800).ymd(), (2026, 11, 2));
    }
}
