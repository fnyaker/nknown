//! Découpage en mots. Provisoire : sera remplacé par la segmentation ICU4X
//! (nécessaire pour les langues sans espaces).

use std::ops::Range;

/// Positions en octets des mots de `text`. Le trait d'union et l'apostrophe restent dans
/// le mot (« porte-monnaie », « aujourd'hui », « l'actu ») : séparer les élisions demande
/// le lexique, c'est le rôle du pipeline.
pub fn words(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (i, c) in text.char_indices() {
        let in_word = c.is_alphanumeric() || (start.is_some() && is_joiner(c));
        match (in_word, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                out.push(s..i);
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push(s..text.len());
    }
    out
}

/// Caractères qui prolongent un mot commencé.
pub fn is_joiner(c: char) -> bool {
    matches!(c, '-' | '\'' | '\u{2019}')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(text: &str) -> Vec<&str> {
        words(text).into_iter().map(|r| &text[r]).collect()
    }

    #[test]
    fn splits_french() {
        assert_eq!(
            split("L'actu de Paris, porte-monnaie, aujourd\u{2019}hui !"),
            [
                "L'actu",
                "de",
                "Paris",
                "porte-monnaie",
                "aujourd\u{2019}hui"
            ]
        );
    }
}
