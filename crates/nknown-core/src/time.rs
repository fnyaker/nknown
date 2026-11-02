//! Dates et fiabilité des dates.

use std::time::{SystemTime, UNIX_EPOCH};

/// Instant UTC, en secondes depuis l'époque Unix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(pub i64);

impl Timestamp {
    pub fn now() -> Self {
        Self::from_system(SystemTime::now()).unwrap_or(Self(0))
    }

    /// Date ISO 8601 / RFC 3339 : `2026-10-30T12:34:56Z`, `2026-10-30T14:34:56+02:00`,
    /// fractions de seconde ignorées.
    pub fn parse_rfc3339(s: &str) -> Option<Self> {
        let s = s.trim();
        let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
        let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
        let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
        if !matches!(s.as_bytes().get(10), Some(b'T' | b't' | b' ')) || !(1..=12).contains(&mo) {
            return None;
        }
        let mut rest = &s[19..];
        if let Some(frac) = rest.strip_prefix('.') {
            rest = frac.trim_start_matches(|c: char| c.is_ascii_digit());
        }
        let offset = match rest {
            "Z" | "z" | "" => 0,
            _ => {
                let sign = match rest.as_bytes().first()? {
                    b'+' => 1,
                    b'-' => -1,
                    _ => return None,
                };
                let oh: i64 = rest.get(1..3)?.parse().ok()?;
                let om: i64 = rest.get(4..6)?.parse().ok()?;
                sign * (oh * 3600 + om * 60)
            }
        };
        // Jours depuis l'époque (« days from civil », H. Hinnant).
        let y = if mo <= 2 { y - 1 } else { y };
        let era = y.div_euclid(400);
        let yoe = y.rem_euclid(400);
        let mp = (mo + 9) % 12;
        let doy = (153 * mp + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = era * 146_097 + doe - 719_468;
        Some(Timestamp(days * 86_400 + h * 3600 + mi * 60 + sec - offset))
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
        for t in [0, 951_782_400, 1_793_620_800, 1_793_666_096] {
            let (y, m, d) = Timestamp(t).ymd();
            let s = format!("{y:04}-{m:02}-{d:02}T00:00:00Z");
            assert_eq!(
                Timestamp::parse_rfc3339(&s),
                Some(Timestamp(t - t.rem_euclid(86_400)))
            );
        }
        assert_eq!(
            Timestamp::parse_rfc3339("2026-11-02T14:00:00.123+02:00"),
            Timestamp::parse_rfc3339("2026-11-02T12:00:00Z")
        );
        assert_eq!(Timestamp::parse_rfc3339("pas une date"), None);
    }
}
