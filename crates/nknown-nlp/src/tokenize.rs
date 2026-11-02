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
        if text[start..end].chars().any(is_cjk) {
            segment_cjk(text, start..end, &mut out);
        } else {
            out.push(start..end);
        }
    }
    out
}

/// Idéogrammes et kana : écritures sans espaces entre les mots.
pub fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{3040}'..='\u{30FF}'   // hiragana, katakana
        | '\u{3400}'..='\u{4DBF}' // CJK extension A
        | '\u{4E00}'..='\u{9FFF}' // CJK unifiés
        | '\u{F900}'..='\u{FAFF}' // compatibilité
        | '\u{20000}'..='\u{2EBEF}')
}

/// Découpe en mots une suite contenant du chinois ou du japonais (dictionnaire ICU4X).
fn segment_cjk(text: &str, span: Range<usize>, out: &mut Vec<Range<usize>>) {
    use icu_segmenter::WordSegmenter;
    let piece = &text[span.clone()];
    let segmenter = WordSegmenter::new_dictionary(Default::default());
    let mut prev = 0;
    for b in segmenter.segment_str(piece) {
        if b > prev && piece[prev..b].chars().any(|c| c.is_alphanumeric()) {
            out.push(span.start + prev..span.start + b);
        }
        prev = b;
    }
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
    token.starts_with(['.', '!', '?', '…', '。', '！', '？'])
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

    #[test]
    fn segments_chinese() {
        let text = "我在北京大学学习中文。Rust很好";
        let toks = split(text);
        assert!(toks.len() >= 6, "{toks:?}");
        assert!(toks.contains(&"。"));
        assert!(toks.contains(&"Rust"));
        // Chaque morceau reste dans le texte, sans chevauchement.
        let spans = tokens(text);
        assert!(spans.windows(2).all(|w| w[0].end <= w[1].start));
        assert!(ends_sentence("。"));
    }
}
