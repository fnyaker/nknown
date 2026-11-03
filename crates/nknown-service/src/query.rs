//! Requête tapée → requête du moteur ; extraits de résultats.

use nknown_core::analyze::{Analysis, Pos};
use nknown_core::lang::Lang;
use nknown_engine::schema::TermKind;
use nknown_engine::{Query, QueryTerm, intent_for};
use nknown_nlp::langid::{self, LangDetector};
use nknown_nlp::{Pipeline, segment, tokenize};

/// Langue d'une requête trop courte pour la détection statistique : celle dont le lexique
/// connaît au moins deux tiers des mots, et strictement plus que toute autre.
fn lexicon_lang(pipeline: &Pipeline, text: &str) -> Option<Lang> {
    let words: Vec<&str> = text
        .split_whitespace()
        .flat_map(|chunk| tokenize::words(chunk).into_iter().map(move |s| &chunk[s]))
        .collect();
    if words.len() < 2 {
        return None;
    }
    let mut scores: Vec<(usize, Lang)> = pipeline
        .lexicon_langs()
        .into_iter()
        .filter_map(|lang| {
            let lex = pipeline.lexicon(lang)?;
            let known = words
                .iter()
                .filter(|w| lex.lookup(w).next().is_some())
                .count();
            Some((known, lang))
        })
        .collect();
    scores.sort_by_key(|&(n, _)| std::cmp::Reverse(n));
    match scores.as_slice() {
        [(best, lang), rest @ ..]
            if *best * 3 >= words.len() * 2 && rest.first().is_none_or(|(n, _)| n < best) =>
        {
            Some(*lang)
        }
        _ => None,
    }
}

/// Part d'usage au-delà de laquelle une forme est un mot outil.
const FUNCTION_SHARE: f32 = 0.8;

/// Requête → mots et lectures possibles (lexique, plus la forme brute pour les mots inconnus
/// et les noms propres), et intention éventuelle (« actu », « code »...).
///
/// Les mots outils sont retirés : ils ne disent rien du sujet, et un document ne doit pas être
/// pénalisé de ne pas contenir « des ». Si la langue de la requête est reconnue (`detector`),
/// c'est le tagueur qui en décide, en contexte (« the cat is on the mat » : the, is, on). Sinon,
/// est un mot outil une forme dont l'usage, dans la langue où elle est la plus fréquente, est à
/// plus de [`FUNCTION_SHARE`] celui d'un article, d'une préposition, d'un pronom… (« en » :
/// préposition et pronom à 98 %, adverbe à 2 %). Une requête faite seulement de mots outils les
/// garde.
pub fn build_query(pipeline: &Pipeline, text: &str) -> Query {
    build_query_with(pipeline, None, text)
}

/// Comme [`build_query`], avec la détection de langue pour étiqueter la requête en contexte.
pub fn build_query_with(pipeline: &Pipeline, detector: Option<&LangDetector>, text: &str) -> Query {
    // Catégorie de chaque mot (par position dans `text`) selon le tagueur de sa langue.
    let tagged: std::collections::HashMap<usize, Pos> = detector
        .and_then(|d| d.detect(text))
        .filter(|lang| pipeline.lexicon(*lang).is_some())
        .or_else(|| detector.and_then(|_| lexicon_lang(pipeline, text)))
        .map(|lang| {
            // Sans l'analyseur neuronal : le tagueur suffit à reconnaître les mots outils, et
            // la requête reste instantanée (sur GPU, un aller-retour coûte des centaines de ms).
            pipeline
                .analyze_light(lang, text)
                .tokens
                .into_iter()
                .map(|t| (t.span.start, t.pos))
                .collect()
        })
        .unwrap_or_default();
    let mut intent = None;
    let mut terms = Vec::new();
    let mut function = Vec::new();
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
            function.push(false);
            continue;
        }
        // Mot ordinaire : ses lectures en prose (lexique), et le même mot comme terme de code.
        let offset = chunk.as_ptr() as usize - text.as_ptr() as usize;
        for span in tokenize::words(chunk) {
            let at = offset + span.start;
            let form = &chunk[span];
            let mut readings: Vec<(Lang, TermKind, String)> = Vec::new();
            let with_lexicon = pipeline.lexicon_langs();
            // (fréquence totale, fréquence des lectures de mot outil, lecture de contenu) de la
            // forme dans la langue où elle est la plus fréquente.
            let mut usage: Option<(f32, f32, bool)> = None;
            for &lang in &with_lexicon {
                let lexicon = pipeline.lexicon(lang).expect("langue listée");
                let (mut total, mut function, mut content, mut any) = (0.0, 0.0, false, false);
                for e in lexicon.lookup(form) {
                    any = true;
                    total += e.freq;
                    if nknown_engine::ingest::is_content(e.pos) {
                        content = true;
                    } else {
                        function += e.freq;
                    }
                }
                if any && usage.is_none_or(|(t, _, _)| total > t) {
                    usage = Some((total, function, content));
                }
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
            function.push(match (tagged.get(&at), usage) {
                (Some(pos), _) => !nknown_engine::ingest::is_content(*pos) && *pos != Pos::X,
                (None, usage) => match usage {
                    Some((total, f, _)) if total > 0.0 => f / total > FUNCTION_SHARE,
                    // Sans fréquences : mot outil s'il n'a aucune lecture de contenu.
                    Some((_, _, content)) => !content,
                    None => false,
                },
            });
        }
    }
    if function.iter().any(|f| !f) {
        let mut keep = function.iter().map(|f| !f);
        terms.retain(|_| keep.next().unwrap_or(true));
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

    #[test]
    fn function_words_are_dropped_unless_alone() {
        use nknown_core::analyze::Feats;
        use nknown_nlp::lexicon::LexiconBuilder;
        let mut b = LexiconBuilder::new();
        b.insert("carte", "carte", Pos::Noun, Feats::default());
        b.insert("des", "un", Pos::Det, Feats::default());
        b.insert("des", "de", Pos::Adp, Feats::default());
        b.insert_with_frequency("en", "en", Pos::Adp, Feats::default(), 7211.0);
        b.insert_with_frequency("en", "en", Pos::Pron, Feats::default(), 2997.0);
        b.insert_with_frequency("en", "en", Pos::Adv, Feats::default(), 192.0);
        // Lecture de contenu rare : « direct » reste un mot de contenu.
        b.insert_with_frequency("direct", "direct", Pos::Adj, Feats::default(), 60.0);
        let pipeline = Pipeline::default().with_lexicon(Lang::FRA, b.build().unwrap());
        let words = |q: &str| -> Vec<String> {
            build_query(&pipeline, q)
                .terms
                .into_iter()
                .map(|t| t.text)
                .collect()
        };
        assert_eq!(
            words("carte des résultats en direct"),
            ["carte", "résultats", "direct"]
        );
        // Seulement des mots outils : on les garde.
        assert_eq!(words("des en"), ["des", "en"]);
    }
}
