//! Client HTTP partagé par les sources réseau.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nknown_core::source::SourceError;

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

    pub fn get_json(&self, url: &str) -> Result<serde_json::Value, SourceError> {
        serde_json::from_slice(&self.get_bytes(url)?)
            .map_err(|e| SourceError::Parse(format!("{url} : {e}")))
    }
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
    fn encodes_components() {
        assert_eq!(encode_component("Tour Eiffel"), "Tour%20Eiffel");
        assert_eq!(encode_component("Île-de-France"), "%C3%8Ele-de-France");
        assert_eq!(encode_component("a&b=c"), "a%26b%3Dc");
    }
}
