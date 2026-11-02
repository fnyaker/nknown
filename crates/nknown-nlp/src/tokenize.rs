//! Découpage en mots. Provisoire : sera remplacé par la segmentation ICU4X
//! (nécessaire pour les langues sans espaces).

use std::ops::Range;

/// Positions en octets des mots de `text`. L'apostrophe sépare (« l'actu » → « l », « actu »),
/// le trait d'union lie (« porte-monnaie »).
pub fn words(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (i, c) in text.char_indices() {
        let in_word = c.is_alphanumeric() || (c == '-' && start.is_some());
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

#[cfg(test)]
mod tests {
    use super::*;

    fn split(text: &str) -> Vec<&str> {
        words(text).into_iter().map(|r| &text[r]).collect()
    }

    #[test]
    fn splits_french() {
        assert_eq!(
            split("L'actu de Paris, porte-monnaie !"),
            ["L", "actu", "de", "Paris", "porte-monnaie"]
        );
    }
}
