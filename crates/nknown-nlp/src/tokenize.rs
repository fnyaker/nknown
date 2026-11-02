//! Découpage en tokens. Provisoire : sera remplacé par la segmentation ICU4X
//! (nécessaire pour les langues sans espaces).

use std::ops::Range;

/// Positions en octets des tokens de `text` : mots et ponctuation.
///
/// Le trait d'union et l'apostrophe restent dans le mot (« porte-monnaie », « aujourd'hui »,
/// « l'actu ») : séparer les élisions demande le lexique, c'est le rôle du pipeline.
/// Les signes de ponctuation identiques consécutifs forment un seul token (« ... »).
pub fn tokens(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some((start, c)) = chars.next() {
        if c.is_whitespace() {
            continue;
        }
        let mut end = start + c.len_utf8();
        let continues = |d: char| {
            if c.is_alphanumeric() {
                d.is_alphanumeric() || is_joiner(d)
            } else {
                d == c
            }
        };
        while let Some(&(i, d)) = chars.peek() {
            if !continues(d) {
                break;
            }
            end = i + d.len_utf8();
            chars.next();
        }
        out.push(start..end);
    }
    out
}

/// Les tokens qui sont des mots (ils commencent par une lettre ou un chiffre).
pub fn words(text: &str) -> Vec<Range<usize>> {
    tokens(text)
        .into_iter()
        .filter(|r| is_word(&text[r.clone()]))
        .collect()
}

pub fn is_word(token: &str) -> bool {
    token.starts_with(char::is_alphanumeric)
}

/// Caractères qui prolongent un mot commencé.
pub fn is_joiner(c: char) -> bool {
    matches!(c, '-' | '\'' | '\u{2019}')
}

/// Fin de phrase (« . », « ! », « ? », « … »).
pub fn ends_sentence(token: &str) -> bool {
    token.starts_with(['.', '!', '?', '…'])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(text: &str) -> Vec<&str> {
        tokens(text).into_iter().map(|r| &text[r]).collect()
    }

    #[test]
    fn splits_french() {
        assert_eq!(
            split("L'actu de Paris, porte-monnaie, aujourd\u{2019}hui !"),
            [
                "L'actu",
                "de",
                "Paris",
                ",",
                "porte-monnaie",
                ",",
                "aujourd\u{2019}hui",
                "!"
            ]
        );
    }

    #[test]
    fn groups_repeated_punctuation() {
        assert_eq!(
            split("Quoi?! « Non... »"),
            ["Quoi", "?", "!", "«", "Non", "...", "»"]
        );
        assert_eq!(words("Quoi?! « Non... »").len(), 2);
    }
}
