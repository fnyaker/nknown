//! Requête tapée → requête du moteur ; extraits de résultats.

use nknown_core::analyze::{Analysis, Pos};
use nknown_core::lang::Lang;
use nknown_engine::schema::TermKind;
use nknown_engine::{Modifier, Query, QueryTerm, RECENT_HALF_LIFE_DAYS, modifier_for};
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

/// Mots liés proposés par mot de la requête, au plus (le moteur garde les meilleurs de ceux
/// que la base contient), et force minimale d'un lien.
const MAX_RELATED: usize = 24;
const MIN_RELATED: f32 = 0.3;

/// Mots de sens proche des lectures de contenu d'un mot (liens `<langue>.rel`), du plus
/// proche au plus lointain, sans les lemmes déjà lus.
fn related_words(
    pipeline: &Pipeline,
    readings: &[(Lang, TermKind, String)],
) -> Vec<(Lang, String, f32)> {
    let own: std::collections::HashSet<(Lang, String)> = readings
        .iter()
        .map(|(l, _, lemma)| (*l, lemma.to_lowercase()))
        .collect();
    let mut best: std::collections::HashMap<(Lang, String), f32> = Default::default();
    for (lang, kind, lemma) in readings {
        let TermKind::Word(pos) = kind else { continue };
        if !nknown_engine::ingest::is_content(*pos) {
            continue;
        }
        let Some(rel) = pipeline.relations(*lang) else {
            continue;
        };
        for (word, w) in rel.related(lemma) {
            if w < MIN_RELATED {
                break;
            }
            let key = (*lang, word.to_owned());
            if own.contains(&key) {
                continue;
            }
            let slot = best.entry(key).or_insert(0.0);
            *slot = slot.max(w);
        }
    }
    let mut out: Vec<(Lang, String, f32)> = best.into_iter().map(|((l, w), p)| (l, w, p)).collect();
    out.sort_by(|a, b| b.2.total_cmp(&a.2).then_with(|| a.1.cmp(&b.1)));
    out.truncate(MAX_RELATED);
    out
}

/// Poids gardé par un mot lié que rien dans le reste de la requête ne soutient.
const UNSUPPORTED: f32 = 0.6;

/// Sens d'un mot selon les autres mots de la requête : dans « ordinateur portable »,
/// « laptop » (lié aussi à « ordinateur ») garde son poids, « gsm » (le téléphone) en perd.
/// Un mot lié est soutenu s'il est lié à un mot de la requête ou à l'un de leurs mots liés.
fn weigh_by_context(pipeline: &Pipeline, terms: &mut [QueryTerm]) {
    let content = |t: &QueryTerm| -> Vec<(Lang, String)> {
        t.readings
            .iter()
            .filter(
                |(_, k, _)| matches!(k, TermKind::Word(p) if nknown_engine::ingest::is_content(*p)),
            )
            .map(|(l, _, w)| (*l, w.to_lowercase()))
            .collect()
    };
    if terms.iter().filter(|t| !t.related.is_empty()).count() == 0 || terms.len() < 2 {
        return;
    }
    // Pour chaque mot : ses lemmes et ses mots liés, avec leur force.
    let around: Vec<std::collections::HashMap<(Lang, String), f32>> = terms
        .iter()
        .map(|t| {
            let mut m: std::collections::HashMap<(Lang, String), f32> =
                content(t).into_iter().map(|k| (k, 1.0)).collect();
            for (l, w, p) in &t.related {
                let slot = m.entry((*l, w.clone())).or_insert(0.0);
                *slot = slot.max(*p);
            }
            m
        })
        .collect();
    for (i, term) in terms.iter_mut().enumerate() {
        for (lang, word, weight) in &mut term.related {
            let mut support = 0.0f32;
            for (j, other) in around.iter().enumerate() {
                if j == i {
                    continue;
                }
                // Le mot lié est lui-même lié à ce mot de la requête…
                if let Some(&p) = other.get(&(*lang, word.clone())) {
                    support = support.max(p);
                }
                // … ou l'un de ses propres liens y mène.
                if let Some(rel) = pipeline.relations(*lang) {
                    for (w, p) in rel.related(word) {
                        if let Some(&q) = other.get(&(*lang, w.to_owned())) {
                            support = support.max(p * q);
                        }
                    }
                }
            }
            *weight *= UNSUPPORTED + (1.0 - UNSUPPORTED) * support.min(1.0);
        }
        term.related
            .sort_by(|a, b| b.2.total_cmp(&a.2).then_with(|| a.1.cmp(&b.1)));
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
    let mut recency = None;
    let mut kinds: Vec<u8> = Vec::new();
    let mut terms = Vec::new();
    let mut function = Vec::new();
    for chunk in text.split_whitespace() {
        // Modificateurs : `@code`, `@new`, `@wiki`… (combinables).
        if let Some(m) = chunk.strip_prefix('@').and_then(modifier_for) {
            match m {
                Modifier::Intent(i) => intent = Some(i),
                Modifier::Recent => recency = Some(RECENT_HALF_LIFE_DAYS),
                Modifier::Kinds(k) => kinds.extend(k),
            }
            continue;
        }
        // Identifiant manifeste (`read_to_string`, `HashMap::new`, `f()`) : on cherche
        // l'identifiant exact, pas ses morceaux (« string » est partout dans le code).
        if !segment::inline_code(chunk, 0..chunk.len()).is_empty() {
            let ident = chunk.trim_end_matches("()").trim_matches('`');
            terms.push(QueryTerm {
                text: chunk.to_owned(),
                readings: vec![(Lang::NONE, TermKind::Code, ident.to_lowercase())],
                ..Default::default()
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
            let related = related_words(pipeline, &readings);
            terms.push(QueryTerm {
                text: form.to_owned(),
                readings,
                related,
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
    weigh_by_context(pipeline, &mut terms);
    kinds.sort_unstable();
    kinds.dedup();
    Query {
        terms,
        intent,
        recency,
        kinds,
    }
}

/// Nom affiché des modificateurs d'une requête (« code · récent · wiki »).
pub fn modifiers_label(q: &Query) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    if let Some(i) = &q.intent {
        parts.push(i.name);
    }
    if q.recency.is_some() {
        parts.push("récent");
    }
    for k in &q.kinds {
        parts.push(match k {
            0 => "flux",
            1 => "wiki",
            2 => "web",
            3 => "fichiers",
            _ => "autres",
        });
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
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
        let q = build_query(&Pipeline::default(), "@code read_to_string map");
        assert_eq!(q.intent.map(|i| i.name), Some("code"));
        assert!(is_code_term(&q.terms[0]));
        assert!(!is_code_term(&q.terms[1]));
    }

    #[test]
    fn modifiers_combine_and_bare_words_stay_words() {
        let q = build_query(
            &Pipeline::default(),
            "@new code postal @wiki @fichiers @inconnu",
        );
        assert_eq!(q.recency, Some(RECENT_HALF_LIFE_DAYS));
        assert_eq!(q.kinds, [1, 3]);
        assert!(q.intent.is_none());
        let words: Vec<&str> = q.terms.iter().map(|t| t.text.as_str()).collect();
        // « code » sans arobase est un mot ; un modificateur inconnu aussi.
        assert_eq!(words, ["code", "postal", "inconnu"]);
        assert_eq!(
            modifiers_label(&q).as_deref(),
            Some("récent · wiki · fichiers")
        );
    }

    #[test]
    fn related_words_come_from_the_relations_file() {
        use nknown_core::analyze::Feats;
        use nknown_nlp::lexicon::LexiconBuilder;
        use nknown_nlp::relations::{BuildOptions, Evidence};
        let mut b = LexiconBuilder::new();
        b.insert("élections", "élection", Pos::Noun, Feats::default());
        let mut r = nknown_nlp::RelationsBuilder::new();
        r.link("élection", "scrutin", Evidence::Synonym, 1.0);
        r.link("élection", "choix", Evidence::Related, 0.1);
        let rel = r.build(BuildOptions::default(), |_| 1.0).unwrap();
        let pipeline = Pipeline::default()
            .with_lexicon(Lang::FRA, b.build().unwrap())
            .with_relations(Lang::FRA, rel);
        let q = build_query(&pipeline, "élections");
        let related: Vec<&str> = q.terms[0]
            .related
            .iter()
            .map(|(_, w, _)| w.as_str())
            .collect();
        // Lien trop faible : écarté.
        assert_eq!(related, ["scrutin"]);
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
