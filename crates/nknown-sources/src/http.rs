//! Client HTTP partagé par les sources réseau.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nknown_core::source::SourceError;
use nknown_core::time::Timestamp;

/// Réponse brute d'une page web, quel que soit son statut (pour l'exploration de sites).
#[derive(Debug, Clone)]
pub struct Page {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
    /// En-tête `cf-mitigated: challenge` : Cloudflare demande une vérification humaine.
    pub challenge_header: bool,
    pub last_modified: Option<Timestamp>,
}

/// Identification envoyée aux serveurs (Wikimedia l'exige descriptive).
/// Remplaçable par la variable d'environnement `NKNOWN_USER_AGENT`, par exemple pour
/// y ajouter un contact.
pub const DEFAULT_USER_AGENT: &str = "NKnown/0.1 (base de connaissances personnelle; Rust ureq)";

/// Client HTTP poli : délai minimal entre deux requêtes, et reprise sur 429/503
/// en respectant `Retry-After`.
#[derive(Debug, Clone)]
pub struct Http {
    agent: ureq::Agent,
    min_interval: Duration,
    last: Arc<Mutex<Option<Instant>>>,
}

impl Default for Http {
    fn default() -> Self {
        Self::new()
    }
}

/// Nombre maximal de reprises après un refus temporaire.
const RETRIES: u32 = 4;

impl Http {
    pub fn new() -> Self {
        Self::with_interval(Duration::ZERO)
    }

    /// Client qui espace ses requêtes d'au moins `min_interval`.
    pub fn with_interval(min_interval: Duration) -> Self {
        let user_agent =
            std::env::var("NKNOWN_USER_AGENT").unwrap_or_else(|_| DEFAULT_USER_AGENT.to_owned());
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .user_agent(user_agent)
            .http_status_as_error(false)
            .build();
        Self {
            agent: config.into(),
            min_interval,
            last: Arc::new(Mutex::new(None)),
        }
    }

    fn throttle(&self) {
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(t) = *last {
            let elapsed = t.elapsed();
            if elapsed < self.min_interval {
                std::thread::sleep(self.min_interval - elapsed);
            }
        }
        *last = Some(Instant::now());
    }

    pub fn get_bytes(&self, url: &str) -> Result<Vec<u8>, SourceError> {
        let fail = |m: String| SourceError::Unavailable(format!("{url} : {m}"));
        for attempt in 0..=RETRIES {
            self.throttle();
            let mut resp = self
                .agent
                .get(url)
                .call()
                .map_err(|e| fail(e.to_string()))?;
            let status = resp.status().as_u16();
            if (200..300).contains(&status) {
                return resp
                    .body_mut()
                    .with_config()
                    .limit(32 << 20)
                    .read_to_vec()
                    .map_err(|e| fail(e.to_string()));
            }
            if !matches!(status, 429 | 503) || attempt == RETRIES {
                return Err(fail(format!("HTTP {status}")));
            }
            // Attente demandée par le serveur, sinon 2, 4, 8, 16 s.
            let wait = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok())
                .unwrap_or(2 << attempt)
                .min(60);
            std::thread::sleep(Duration::from_secs(wait));
        }
        unreachable!("la boucle renvoie toujours")
    }

    /// Une requête, sans reprise, quel que soit le statut : l'appelant juge de la réponse
    /// (protection anti-robot, page absente…). Corps limité à 8 Mio.
    pub fn get_page(&self, url: &str) -> Result<Page, SourceError> {
        let fail = |m: String| SourceError::Unavailable(format!("{url} : {m}"));
        self.throttle();
        let mut resp = self
            .agent
            .get(url)
            .header("Accept", "text/html,application/xhtml+xml;q=0.9,*/*;q=0.1")
            .call()
            .map_err(|e| fail(e.to_string()))?;
        let header = |name: &str| {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        let content_type = header("content-type")
            .unwrap_or_default()
            .to_ascii_lowercase();
        let challenge_header = header("cf-mitigated").is_some_and(|v| v.contains("challenge"));
        let last_modified = header("last-modified").and_then(|d| parse_http_date(&d));
        let status = resp.status().as_u16();
        let body = resp
            .body_mut()
            .with_config()
            .limit(8 << 20)
            .read_to_vec()
            .map_err(|e| fail(e.to_string()))?;
        Ok(Page {
            status,
            content_type,
            body,
            challenge_header,
            last_modified,
        })
    }

    /// Taille et date de modification (`Last-Modified`, telle quelle) d'une ressource.
    pub fn head(&self, url: &str) -> Result<(u64, String), SourceError> {
        let fail = |m: String| SourceError::Unavailable(format!("{url} : {m}"));
        self.throttle();
        let resp = self
            .agent
            .head(url)
            .call()
            .map_err(|e| fail(e.to_string()))?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(fail(format!("HTTP {status}")));
        }
        let header = |n: &str| {
            resp.headers()
                .get(n)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        let len = header("content-length")
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| fail("taille inconnue".into()))?;
        Ok((len, header("last-modified").unwrap_or_default()))
    }

    /// Octets `start..end` (fin exclue) d'une ressource (requête `Range`), avec reprises.
    pub fn get_range(&self, url: &str, start: u64, end: u64) -> Result<Vec<u8>, SourceError> {
        let fail = |m: String| SourceError::Unavailable(format!("{url} : {m}"));
        let mut last = String::new();
        for attempt in 0..=RETRIES {
            self.throttle();
            let result = self
                .agent
                .get(url)
                .header("Range", &format!("bytes={start}-{}", end.saturating_sub(1)))
                .call();
            match result {
                Ok(mut resp) if resp.status().as_u16() == 206 => {
                    match resp
                        .body_mut()
                        .with_config()
                        .limit(end - start + 1)
                        .read_to_vec()
                    {
                        Ok(v) => return Ok(v),
                        Err(e) => last = e.to_string(),
                    }
                }
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    if !matches!(status, 429 | 500..=599) {
                        return Err(fail(format!("HTTP {status} (plage refusée)")));
                    }
                    last = format!("HTTP {status}");
                }
                Err(e) => last = e.to_string(),
            }
            if attempt < RETRIES {
                std::thread::sleep(Duration::from_secs(2 << attempt));
            }
        }
        Err(fail(last))
    }

    /// Corps d'une réponse en flux (gros fichiers), sans limite de taille.
    pub fn open(&self, url: &str) -> Result<Box<dyn std::io::Read + Send>, SourceError> {
        let fail = |m: String| SourceError::Unavailable(format!("{url} : {m}"));
        self.throttle();
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(None)
            .timeout_recv_body(Some(Duration::from_secs(120)))
            .user_agent(
                std::env::var("NKNOWN_USER_AGENT").unwrap_or_else(|_| DEFAULT_USER_AGENT.into()),
            )
            .http_status_as_error(false)
            .build()
            .into();
        let resp = agent.get(url).call().map_err(|e| fail(e.to_string()))?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(fail(format!("HTTP {status}")));
        }
        Ok(Box::new(resp.into_body().into_reader()))
    }

    pub fn get_json(&self, url: &str) -> Result<serde_json::Value, SourceError> {
        serde_json::from_slice(&self.get_bytes(url)?)
            .map_err(|e| SourceError::Parse(format!("{url} : {e}")))
    }
}

/// Date HTTP (`Sun, 06 Nov 1994 08:49:37 GMT`).
pub fn parse_http_date(s: &str) -> Option<Timestamp> {
    let mut it = s.split_whitespace().skip(1);
    let day: u32 = it.next()?.parse().ok()?;
    let month = it.next()?;
    let year: u32 = it.next()?.parse().ok()?;
    let time = it.next()?;
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let m = MONTHS.iter().position(|n| month.eq_ignore_ascii_case(n))? + 1;
    Timestamp::parse_rfc3339(&format!("{year:04}-{m:02}-{day:02}T{time}Z"))
}

/// Encodage d'un composant d'URL (RFC 3986 : tout sauf les caractères non réservés).
pub fn encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_http_dates() {
        let t = parse_http_date("Sun, 06 Nov 1994 08:49:37 GMT").unwrap();
        assert_eq!(t.ymd(), (1994, 11, 6));
        assert!(parse_http_date("hier").is_none());
    }

    #[test]
    fn encodes_components() {
        assert_eq!(encode_component("Tour Eiffel"), "Tour%20Eiffel");
        assert_eq!(encode_component("Île-de-France"), "%C3%8Ele-de-France");
        assert_eq!(encode_component("a&b=c"), "a%26b%3Dc");
    }
}
