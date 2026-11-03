//! Authentification : jeton (`Authorization: Bearer`) pour les clients natifs, cookie de
//! session `HttpOnly` pour l'interface web. Le jeton ne passe jamais dans une URL.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Durée de vie d'une session de l'interface web.
const SESSION_TTL: Duration = Duration::from_secs(12 * 3600);

#[derive(Debug)]
pub enum Auth {
    /// Démon local sur socket Unix : l'accès au socket est déjà réservé à l'utilisateur.
    None,
    Token {
        token: String,
        sessions: Mutex<HashMap<String, Instant>>,
        /// Ajouter `Secure` au cookie (à désactiver seulement derrière du HTTP en clair).
        secure_cookie: bool,
    },
}

impl Auth {
    pub fn token(token: String, secure_cookie: bool) -> Self {
        Auth::Token {
            token,
            sessions: Mutex::new(HashMap::new()),
            secure_cookie,
        }
    }

    /// Vérifie un en-tête `Authorization` ou un cookie de session.
    pub fn check(&self, bearer: Option<&str>, session: Option<&str>) -> bool {
        let Auth::Token {
            token, sessions, ..
        } = self
        else {
            return true;
        };
        if let Some(b) = bearer
            && constant_time_eq(b.as_bytes(), token.as_bytes())
        {
            return true;
        }
        let Some(id) = session else { return false };
        let mut sessions = sessions.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        sessions.retain(|_, exp| *exp > now);
        sessions
            .keys()
            .any(|k| constant_time_eq(k.as_bytes(), id.as_bytes()))
    }

    /// Échange un jeton valide contre un identifiant de session.
    pub fn open_session(&self, candidate: &str) -> Option<String> {
        let Auth::Token {
            token, sessions, ..
        } = self
        else {
            return None;
        };
        if !constant_time_eq(candidate.as_bytes(), token.as_bytes()) {
            return None;
        }
        let id = random_hex(32);
        sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.clone(), Instant::now() + SESSION_TTL);
        Some(id)
    }

    pub fn close_session(&self, id: &str) {
        if let Auth::Token { sessions, .. } = self {
            sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(id);
        }
    }

    pub fn cookie(&self, id: &str) -> String {
        let secure = matches!(
            self,
            Auth::Token {
                secure_cookie: true,
                ..
            }
        );
        format!(
            "{}={id}; Path=/api; HttpOnly; SameSite=Strict; Max-Age={}{}",
            nknown_api::SESSION_COOKIE,
            SESSION_TTL.as_secs(),
            if secure { "; Secure" } else { "" }
        )
    }
}

/// Comparaison dont la durée ne dépend pas de la position de la première différence.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// `bytes` octets aléatoires du système, en hexadécimal.
pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).expect("générateur aléatoire du système");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_and_sessions() {
        let auth = Auth::token("secret".into(), true);
        assert!(auth.check(Some("secret"), None));
        assert!(!auth.check(Some("secreT"), None));
        assert!(!auth.check(None, Some("x")));
        assert!(auth.open_session("nope").is_none());
        let id = auth.open_session("secret").unwrap();
        assert_eq!(id.len(), 64);
        assert!(auth.check(None, Some(&id)));
        assert!(auth.cookie(&id).contains("HttpOnly; SameSite=Strict"));
        auth.close_session(&id);
        assert!(!auth.check(None, Some(&id)));
        assert!(Auth::None.check(None, None));
    }
}
