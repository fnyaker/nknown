//! Requête tapée → requête du moteur ; extraits de résultats.

use std::collections::HashMap;

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

/// Lissage et plancher du choix entre lectures d'une forme (voir [`related_words`]).
const READING_SMOOTHING: f32 = 0.1;
const READING_FLOOR: f32 = 0.15;

/// Voisins d'un lemme dans les documents (langue, catégorie, lemme → (lemme, force)).
pub type Corpus<'a> = &'a dyn Fn(Lang, TermKind, &str) -> Vec<(String, f32)>;

/// Un mot écrit dans l'écriture de sa langue : « mouse » n'est pas un mot chinois, même si le
/// lexique chinois le connaît (ses liens chinois prendraient la place des liens anglais).
fn written_in(lang: Lang, lemma: &str) -> bool {
    let letters = || lemma.chars().filter(|c| c.is_alphabetic());
    match lang.as_str() {
        "zho" | "jpn" => lemma.chars().any(tokenize::is_cjk),
        "rus" | "ukr" | "bul" | "srp" => letters().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c)),
        _ => letters().all(|c| c.is_ascii() || ('\u{00C0}'..='\u{024F}').contains(&c)),
    }
}

/// Contexte au second pas : mots liés suivis, voisins pris pour chacun, et poids.
const SECOND_HOP_FROM: usize = 8;
const SECOND_HOP_PER_WORD: usize = 8;
const SECOND_HOP: f32 = 0.25;

/// Poids des voisins d'un mot dans les documents, face aux autres mots de la requête.
const CORPUS_CONTEXT: f32 = 0.5;

/// Contexte d'un mot de la requête : présence (0..=1) d'un lemme autour de lui.
type Context<'a> = &'a dyn Fn(Lang, &str) -> f32;

/// Mots de sens proche des lectures de contenu d'un mot (liens `<langue>.rel`), du plus
/// proche au plus lointain, sans les lemmes déjà lus. Avec un `context`, les liens d'un sens
/// que le reste de la requête écarte perdent leur poids.
///
/// `tag` : catégorie du mot dans la requête, selon le tagueur. Les liens viennent alors des
/// seules lectures de cette catégorie : « livre » nom n'apporte pas ceux du verbe « livrer ».
fn related_words(
    pipeline: &Pipeline,
    readings: &[(Lang, TermKind, String)],
    tag: Option<Pos>,
    context: Option<Context<'_>>,
) -> Vec<(Lang, String, f32)> {
    let class = |p: Pos| match p {
        Pos::Propn => Pos::Noun,
        p => p,
    };
    let tagged = tag.filter(|p| nknown_engine::ingest::is_content(*p) && *p != Pos::X);
    let matches = |k: &TermKind| match (k, tagged) {
        (TermKind::Word(p), Some(t)) => class(*p) == class(t),
        _ => true,
    };
    // Sans lecture de la catégorie donnée (lexique et tagueur en désaccord) : toutes.
    let restrict = tagged.is_some()
        && readings.iter().any(|(_, k, _)| {
            matches!(k, TermKind::Word(p) if nknown_engine::ingest::is_content(*p)) && matches(k)
        });
    let own: std::collections::HashSet<(Lang, String)> = readings
        .iter()
        .map(|(l, _, lemma)| (*l, lemma.to_lowercase()))
        .collect();
    let mut best: std::collections::HashMap<(Lang, String), f32> = Default::default();
    let mut records = Vec::new();
    for (lang, kind, lemma) in readings {
        let TermKind::Word(pos) = kind else { continue };
        if !nknown_engine::ingest::is_content(*pos)
            || (restrict && !matches(kind))
            || !written_in(*lang, lemma)
        {
            continue;
        }
        let Some(rel) = pipeline.relations(*lang) else {
            continue;
        };
        records.push((*lang, rel.record(lemma)));
    }
    // Lectures : celle que le contexte soutient l'emporte (« livre sterling » : le nom, pas
    // le verbe « livrer »), comme les sens d'un même mot.
    let support: Vec<f32> = records
        .iter()
        .map(|(lang, r)| context.map_or(0.0, |ctx| r.support(&|w| ctx(*lang, w))))
        .collect();
    let top = support.iter().copied().fold(0.0, f32::max);
    for ((lang, record), sup) in records.iter().zip(&support) {
        let lang = *lang;
        let factor = if top > 0.0 {
            ((READING_SMOOTHING + sup) / (READING_SMOOTHING + top)).max(READING_FLOOR)
        } else {
            1.0
        };
        let links: Vec<(&str, f32)> = match context {
            None => record.links.iter().map(|l| (l.word, l.weight)).collect(),
            Some(ctx) => {
                let rel = pipeline.relations(lang).expect("fiche lue dans ces liens");
                record
                    .in_context(&|w| ctx(lang, w))
                    .into_iter()
                    .zip(&record.links)
                    .map(|((word, by_sense), link)| {
                        // Le mot lié plaide aussi sa propre cause : « heron », rattaché à un
                        // sens voisin de l'oiseau, est lui-même lié à « bird ».
                        // ses liens et ses mots plus généraux (« heron » → « bird »).
                        let rec = rel.record(word);
                        let own = rec
                            .links
                            .iter()
                            .map(|l| l.weight * ctx(lang, l.word))
                            .chain(rec.broader.iter().map(|w| BROADER_SUPPORT * ctx(lang, w)))
                            .fold(ctx(lang, word), f32::max)
                            .min(1.0);
                        let sense = by_sense * factor;
                        let w = (sense * (UNSUPPORTED + (1.0 - UNSUPPORTED) * own))
                            .max(link.weight * own);
                        (word, w)
                    })
                    .collect()
            }
        };
        // Seuil sur le lien du dictionnaire ; le contexte classe ensuite, sans éliminer (un
        // lien écarté par le sens recule, le moteur ne garde que les meilleurs).
        let links = links
            .into_iter()
            .zip(&record.links)
            .filter(|(_, l)| l.weight >= MIN_RELATED)
            .map(|((w, p), _)| (w, if context.is_some() { p } else { p * factor }));
        let lang = &lang;
        for (word, w) in links {
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

/// Soutien qu'apporte au mot lié un mot plus général présent dans le contexte.
const BROADER_SUPPORT: f32 = 0.8;

/// Poids gardé par un mot lié que rien dans le contexte ne soutient (voir [`related_words`]).
const UNSUPPORTED: f32 = 0.75;

/// Lemmes de contenu d'un mot de la requête et ses mots liés, avec leur présence.
fn surroundings(t: &QueryTerm) -> HashMap<(Lang, String), f32> {
    let mut m: HashMap<(Lang, String), f32> = t
        .readings
        .iter()
        .filter(|(_, k, _)| matches!(k, TermKind::Word(p) if nknown_engine::ingest::is_content(*p)))
        .map(|(l, _, w)| ((*l, w.to_lowercase()), 1.0))
        .collect();
    for (l, w, p) in &t.related {
        let slot = m.entry((*l, w.clone())).or_insert(0.0);
        *slot = slot.max(*p);
    }
    m
}

/// Sens de chaque mot selon les autres : « portable » près d'« ordinateur » est l'ordinateur,
/// pas le téléphone ; « livre » près de « sterling » est la monnaie. Les mots liés de chaque
/// mot sont recalculés avec, pour contexte, les autres mots et leurs propres mots liés.
///
/// `corpus` : voisins d'un lemme dans les documents de l'utilisateur (liens appris). Ils
/// donnent l'usage dominant du mot chez lui (« battery » au milieu de « charge », « laptop »),
/// qui départage les sens même quand la requête n'a qu'un mot, avec un poids moindre que les
/// autres mots de la requête.
fn choose_senses(
    pipeline: &Pipeline,
    terms: &mut [QueryTerm],
    tags: &[Option<Pos>],
    corpus: Option<Corpus<'_>>,
) {
    if terms.iter().all(|t| t.related.is_empty()) {
        return;
    }
    let own: Vec<HashMap<(Lang, String), f32>> = terms
        .iter()
        .map(|t| {
            let mut m = HashMap::new();
            let Some(corpus) = corpus else { return m };
            for (lang, kind, lemma) in &t.readings {
                if !matches!(kind, TermKind::Word(p) if nknown_engine::ingest::is_content(*p)) {
                    continue;
                }
                for (w, p) in corpus(*lang, *kind, lemma) {
                    let slot = m.entry((*lang, w.to_lowercase())).or_insert(0.0f32);
                    *slot = slot.max(CORPUS_CONTEXT * p);
                }
            }
            m
        })
        .collect();
    if terms.len() < 2 && own.iter().all(HashMap::is_empty) {
        return;
    }
    let around: Vec<HashMap<(Lang, String), f32>> = terms
        .iter()
        .map(|t| {
            let mut m = surroundings(t);
            // Un pas de plus : les voisins des mots liés les plus proches, à poids réduit
            // (« vanille » → « dessert » ← « sorbet »).
            let mut near: Vec<(Lang, String, f32)> = t.related.clone();
            near.sort_by(|a, b| b.2.total_cmp(&a.2));
            for (lang, w, p) in near.into_iter().take(SECOND_HOP_FROM) {
                let Some(rel) = pipeline.relations(lang) else {
                    continue;
                };
                for (x, q) in rel.related(&w).into_iter().take(SECOND_HOP_PER_WORD) {
                    let slot = m.entry((lang, x.to_owned())).or_insert(0.0);
                    *slot = slot.max(SECOND_HOP * p * q);
                }
            }
            m
        })
        .collect();
    for (i, term) in terms.iter_mut().enumerate() {
        if term.related.is_empty() {
            continue;
        }
        let ctx = |lang: Lang, w: &str| {
            let key = (lang, w.to_owned());
            around
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .filter_map(|(_, m)| m.get(&key).copied())
                .chain(own[i].get(&key).copied())
                .fold(0.0, f32::max)
        };
        let tag = tags.get(i).copied().flatten();
        term.related = related_words(pipeline, &term.readings, tag, Some(&ctx));
    }
}

/// Liens appris du corpus (voir `nknown_engine::learn`) :
/// - un lien du dictionnaire que les documents confirment se renforce ;
/// - un lien appris seul ne sert que pour un mot dont le dictionnaire ne dit rien (jargon,
///   termes techniques, mots nouveaux) : là où le dictionnaire connaît le mot, les liens
///   appris seuls se sont montrés surtout bruyants (« way » ~ « kind ») ; il doit venir des
///   contextes syntaxiques, être réciproque, net, et pèse peu ;
/// - les contraires du dictionnaire (« hausse » / « baisse ») sont écartés.
const CONFIRM: f32 = 0.5;
const LEARNED_SHARE: f32 = 0.6;
const LEARNED_CAP: f32 = 0.35;
const LEARNED_MIN_COSINE: f32 = 0.45;
const LEARNED_MIN_SHARED: u16 = 8;

/// Origine d'un mot lié, pour l'explication d'une recherche.
#[derive(Debug, Clone, PartialEq)]
pub struct LearnedNote {
    pub term: usize,
    pub word: String,
    pub weight: f32,
    /// Lien du dictionnaire confirmé (sinon : appris seul).
    pub confirmed: bool,
}

/// Ajoute aux mots liés de la requête ceux que les documents ont appris (voir [`CONFIRM`]).
pub fn add_learned<S: nknown_engine::Storage>(
    pipeline: &Pipeline,
    engine: &nknown_engine::Engine<S>,
    q: &mut Query,
) -> Vec<LearnedNote> {
    let mut notes = Vec::new();
    for (t, term) in q.terms.iter_mut().enumerate() {
        let own: std::collections::HashSet<(Lang, String)> = term
            .readings
            .iter()
            .map(|(l, _, w)| (*l, w.to_lowercase()))
            .collect();
        let mut found: HashMap<(Lang, String), (f32, bool)> = HashMap::new();
        for (lang, kind, lemma) in &term.readings {
            let TermKind::Word(pos) = kind else { continue };
            if !nknown_engine::ingest::is_content(*pos) {
                continue;
            }
            let Ok(links) = engine.learned(*lang, *kind, lemma) else {
                continue;
            };
            let record = pipeline.relations(*lang).map(|r| r.record(lemma));
            let silent = record.as_ref().is_none_or(|r| r.links.is_empty());
            for l in links {
                let word = l.lemma.to_lowercase();
                let key = (*lang, word.clone());
                if own.contains(&key) {
                    continue;
                }
                let contrary = record
                    .as_ref()
                    .is_some_and(|r| r.antonyms.contains(&word.as_str()))
                    || pipeline.relations(*lang).is_some_and(|r| {
                        r.record(&word)
                            .antonyms
                            .iter()
                            .any(|a| *a == lemma.to_lowercase())
                    });
                if contrary {
                    continue;
                }
                let dict = term
                    .related
                    .iter()
                    .position(|(l2, w, _)| *l2 == *lang && *w == word);
                let entry = match dict {
                    Some(i) => {
                        let w = &mut term.related[i].2;
                        *w = 1.0 - (1.0 - *w) * (1.0 - CONFIRM * l.cosine.min(1.0));
                        (*w, true)
                    }
                    None if silent
                        && l.kind == nknown_engine::LinkKind::Syntactic
                        && l.cosine >= LEARNED_MIN_COSINE
                        && l.shared >= LEARNED_MIN_SHARED =>
                    {
                        ((LEARNED_SHARE * l.cosine).min(LEARNED_CAP), false)
                    }
                    None => continue,
                };
                let slot = found.entry(key).or_insert(entry);
                if entry.0 > slot.0 {
                    *slot = entry;
                }
            }
        }
        for ((lang, word), (w, confirmed)) in found {
            if !confirmed {
                term.related.push((lang, word.clone(), w));
            }
            notes.push(LearnedNote {
                term: t,
                word,
                weight: w,
                confirmed,
            });
        }
        term.related
            .sort_by(|a, b| b.2.total_cmp(&a.2).then_with(|| a.1.cmp(&b.1)));
        term.related.truncate(MAX_RELATED);
    }
    notes.sort_by(|a, b| a.term.cmp(&b.term).then(b.weight.total_cmp(&a.weight)));
    notes
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
    build_query_inner(pipeline, detector, text, None)
}

/// Comme [`build_query_with`], en laissant les documents de `engine` (liens appris) départager
/// les sens des mots (voir [`choose_senses`]).
pub fn build_query_in<S: nknown_engine::Storage>(
    pipeline: &Pipeline,
    detector: Option<&LangDetector>,
    text: &str,
    engine: &nknown_engine::Engine<S>,
) -> Query {
    let corpus = |lang: Lang, kind: TermKind, lemma: &str| -> Vec<(String, f32)> {
        engine
            .learned(lang, kind, lemma)
            .unwrap_or_default()
            .into_iter()
            .map(|l| (l.lemma, l.cosine))
            .collect()
    };
    build_query_inner(pipeline, detector, text, Some(&corpus))
}

fn build_query_inner(
    pipeline: &Pipeline,
    detector: Option<&LangDetector>,
    text: &str,
    corpus: Option<Corpus<'_>>,
) -> Query {
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
    // Catégorie de chaque mot selon le tagueur, s'il l'a vu.
    let mut tags: Vec<Option<Pos>> = Vec::new();
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
            tags.push(None);
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
            let tag = tagged.get(&at).copied();
            let related = related_words(pipeline, &readings, tag, None);
            terms.push(QueryTerm {
                text: form.to_owned(),
                readings,
                related,
            });
            tags.push(tag);
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
        let mut keep = function.iter().map(|f| !f);
        tags.retain(|_| keep.next().unwrap_or(true));
    }
    choose_senses(pipeline, &mut terms, &tags, corpus);
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
