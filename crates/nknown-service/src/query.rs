//! Requête tapée → requête du moteur ; extraits de résultats.

use nknown_core::analyze::{Analysis, Pos};
use nknown_core::lang::Lang;
use nknown_engine::schema::TermKind;
use nknown_engine::{Query, QueryTerm, intent_for};
use nknown_nlp::langid::{self, LangDetector};
use nknown_nlp::{Pipeline, segment, tokenize};

/// Requête → mots et lectures possibles (lexique, plus la forme brute pour les mots inconnus
/// et les noms propres), et intention éventuelle (« actu », « code »...).
pub fn build_query(pipeline: &Pipeline, text: &str) -> Query {
    let mut intent = None;
    let mut terms = Vec::new();
    for chunk in text.split_whitespace() {
        if intent.is_none()
            && let Some(i) = intent_for(chunk)
        {
            intent = Some(i);
            continue;
        }
        // Identifiant manifeste (`read_to_string`, `HashMap::new`, `f()`) : on cherche
        // l'identifiant exact, pas ses morceaux (« string » est partout dans le code).
        if !segment::inline_code(chunk, 0..chunk.len()).is_empty() {
            let ident = chunk.trim_end_matches("()").trim_matches('`');
            terms.push(QueryTerm {
                text: chunk.to_owned(),
                readings: vec![(Lang::NONE, TermKind::Code, ident.to_lowercase())],
            });
            continue;
        }
        // Mot ordinaire : ses lectures en prose (lexique), et le même mot comme terme de code.
        for span in tokenize::words(chunk) {
            let form = &chunk[span];
            let mut readings: Vec<(Lang, TermKind, String)> = Vec::new();
            let with_lexicon = pipeline.lexicon_langs();
            for &lang in &with_lexicon {
                let lexicon = pipeline.lexicon(lang).expect("langue listée");
                readings.extend(
                    lexicon
                        .lookup(form)
                        .filter(|e| nknown_engine::ingest::is_content(e.pos))
                        .map(|e| (lang, TermKind::Word(e.pos), e.lemma.to_owned())),
                );
                for pos in [Pos::Propn, Pos::Noun, Pos::X] {
                    readings.push((lang, TermKind::Word(pos), form.to_owned()));
                }
            }
            // Autres langues détectables : indexées par racines.
            for lang in LangDetector::languages()
                .into_iter()
                .filter(|l| !with_lexicon.contains(l))
            {
                let stem = langid::stem(lang, form).unwrap_or_else(|| form.to_lowercase());
                readings.push((lang, TermKind::Word(Pos::X), stem));
            }
            readings.push((Lang::NONE, TermKind::Code, form.to_lowercase()));
            readings.dedup();
            terms.push(QueryTerm {
                text: form.to_owned(),
                readings,
            });
        }
    }
    Query { terms, intent }
}

/// Le mot de la requête est un identifiant de code exact (une seule lecture, de code).
pub fn is_code_term(t: &QueryTerm) -> bool {
    matches!(t.readings.as_slice(), [(_, TermKind::Code, _)])
}

/// Extrait de `text` d'environ `width` caractères, centré sur la première occurrence
/// d'un des mots trouvés.
pub fn snippet(text: &str, words: &[String], width: usize) -> String {
    let lower = text.to_lowercase();
    let hit = words
        .iter()
        .filter_map(|w| lower.find(&w.to_lowercase()))
        .min()
        .unwrap_or(0);
    let hit_chars = lower.get(..hit).map_or(0, |s| s.chars().count());
    let start = hit_chars.saturating_sub(width / 3);
    let total = text.chars().count();
    let body = text
        .chars()
        .skip(start)
        .take(width)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let before = if start > 0 { "…" } else { "" };
    let after = if start + width < total { "…" } else { "" };
    format!("{before}{body}{after}")
}

/// Langue qui couvre le plus de tokens de prose.
pub fn main_lang(a: &Analysis) -> Option<Lang> {
    let mut counts: Vec<(Lang, usize)> = Vec::new();
    for seg in &a.segments {
        let Some(l) = seg.lang else { continue };
        let n = seg.tokens.len();
        match counts.iter_mut().find(|(c, _)| *c == l) {
            Some((_, c)) => *c += n,
            None => counts.push((l, n)),
        }
    }
    counts.into_iter().max_by_key(|&(_, n)| n).map(|(l, _)| l)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_is_centered_on_the_first_match() {
        let text = format!("{} needle {}", "a ".repeat(200), "b ".repeat(200));
        let s = snippet(&text, &["needle".to_owned()], 60);
        assert!(s.starts_with('…') && s.ends_with('…') && s.contains("needle"));
    }

    #[test]
    fn identifiers_are_exact_code_terms() {
        let q = build_query(&Pipeline::default(), "code read_to_string map");
        assert_eq!(q.intent.map(|i| i.name), Some("code"));
        assert!(is_code_term(&q.terms[0]));
        assert!(!is_code_term(&q.terms[1]));
    }
}
