//! Requête tapée → requête du moteur ; extraits de résultats.

use std::collections::HashMap;

use nknown_core::analyze::{Analysis, Pos};
use nknown_core::lang::Lang;
use nknown_engine::schema::TermKind;
use nknown_engine::{Modifier, Query, QueryTerm, RECENT_HALF_LIFE_DAYS, Variety, modifier_for};
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

/// Niveaux Strict et Diversifié : bornes du dictionnaire et du nombre de mots liés.
const STRICT_MIN_RELATED: f32 = 0.55;
const STRICT_MIN_EFFECTIVE: f32 = 0.50;
const STRICT_MAX_RELATED: usize = 12;
const DIVERSE_MIN_RELATED: f32 = 0.20;
const DIVERSE_MAX_RELATED: usize = 48;

/// Politique d'élargissement d'un niveau de variété.
#[derive(Debug, Clone, Copy)]
struct Relax {
    /// Force minimale du lien dans le dictionnaire.
    min_related: f32,
    /// Mots liés gardés côté requête.
    max_related: usize,
    /// Poids minimal après pondération par le contexte (0 : pas de plancher).
    min_effective: f32,
    /// Second pas utilisé pour le contexte de choix du sens ; aucun niveau n'en fait un lien
    /// (Diversifié ajoute les siens, validés, dans [`add_diverse_links`]).
    context_second_hop: bool,
    /// Signatures de définitions ajoutées aux mots liés.
    signatures: bool,
    /// Politique des liens appris seuls.
    learned_alone: LearnedAlone,
}

/// Ce qu'un niveau accepte comme lien appris seul.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LearnedAlone {
    /// Aucun (Strict).
    None,
    /// Dictionnaire silencieux et lien syntactique net (comportement historique).
    SilentSyntactic,
    /// Syntactique et thématique, même quand le dictionnaire connaît le mot (Diversifié).
    Broad,
}

impl Relax {
    fn for_variety(variety: Variety) -> Self {
        match variety {
            Variety::Strict => Self {
                min_related: STRICT_MIN_RELATED,
                max_related: STRICT_MAX_RELATED,
                min_effective: STRICT_MIN_EFFECTIVE,
                // Le prototype mesuré filtrait après construction de la requête Normal : le
                // second pas aidait encore à choisir le sens (« bug » logiciel, pas « bog »).
                // Il n'ajoute jamais de lien de requête, même en Normal.
                context_second_hop: true,
                signatures: false,
                learned_alone: LearnedAlone::None,
            },
            Variety::Normal => Self {
                min_related: MIN_RELATED,
                max_related: MAX_RELATED,
                min_effective: 0.0,
                context_second_hop: true,
                signatures: false,
                learned_alone: LearnedAlone::SilentSyntactic,
            },
            Variety::Diverse => Self {
                min_related: DIVERSE_MIN_RELATED,
                max_related: DIVERSE_MAX_RELATED,
                min_effective: 0.0,
                context_second_hop: true,
                signatures: true,
                learned_alone: LearnedAlone::Broad,
            },
        }
    }
}

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
    relax: Relax,
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
            .filter(|(_, l)| l.weight >= relax.min_related)
            .map(|((w, p), _)| (w, if context.is_some() { p } else { p * factor }))
            .filter(|(_, p)| *p >= relax.min_effective);
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
    out.truncate(relax.max_related);
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
    relax: Relax,
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
            // (« vanille » → « dessert » ← « sorbet »). Ce contexte ne crée jamais de lien.
            if relax.context_second_hop {
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
        term.related = related_words(pipeline, &term.readings, tag, Some(&ctx), relax);
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

/// Diversifié : liens appris seuls, plus larges mais seuillés plus haut.
const DIVERSE_LEARNED_CAP: f32 = 0.30;
const DIVERSE_SYNTACTIC_MIN_COSINE: f32 = 0.55;
const DIVERSE_SYNTACTIC_MIN_SHARED: u16 = 12;
const DIVERSE_TOPICAL_MIN_COSINE: f32 = 0.45;
const DIVERSE_TOPICAL_MIN_SHARED: u16 = 15;

/// Signatures de définitions (Diversifié).
const SIGNATURE_MIN_SENSES: usize = 2;
const SIGNATURE_MAX: usize = 12;
const SIGNATURE_BASE: f32 = 0.10;
const SIGNATURE_PER_SENSE: f32 = 0.06;
const SIGNATURE_CAP: f32 = 0.32;
const SIGNATURE_REVERSE: f32 = 0.50;

/// Second pas explicite (Diversifié) : voisins des meilleurs mots liés.
const SECOND_STEP_MIN: f32 = 0.45;
const SECOND_STEP_REVERSE: f32 = 0.50;
const SECOND_STEP_SHARE: f32 = 0.12;
const SECOND_STEP_CAP: f32 = 0.20;
const SECOND_STEP_MAX: usize = 12;

/// Origine d'un mot lié, pour l'explication d'une recherche.
#[derive(Debug, Clone, PartialEq)]
pub struct LearnedNote {
    pub term: usize,
    pub word: String,
    pub weight: f32,
    /// Lien du dictionnaire confirmé (sinon : appris seul).
    pub confirmed: bool,
}

/// Poids d'un lien appris seul selon la politique du niveau ; `None` : refusé.
fn learned_alone_weight(
    policy: LearnedAlone,
    silent: bool,
    kind: nknown_engine::LinkKind,
    cosine: f32,
    shared: u16,
) -> Option<f32> {
    use nknown_engine::LinkKind;
    let mut best = None;
    // Règle historique : dictionnaire silencieux, contexte syntaxique, lien net.
    if silent
        && matches!(policy, LearnedAlone::SilentSyntactic | LearnedAlone::Broad)
        && kind == LinkKind::Syntactic
        && cosine >= LEARNED_MIN_COSINE
        && shared >= LEARNED_MIN_SHARED
    {
        best = Some((LEARNED_SHARE * cosine).min(LEARNED_CAP));
    }
    // Diversifié : mêmes liens même quand le dictionnaire connaît le mot, avec des seuils
    // renforcés ; le thématique devient utilisable seul.
    if policy == LearnedAlone::Broad {
        let (min_cosine, min_shared) = match kind {
            LinkKind::Syntactic => (DIVERSE_SYNTACTIC_MIN_COSINE, DIVERSE_SYNTACTIC_MIN_SHARED),
            LinkKind::Topical => (DIVERSE_TOPICAL_MIN_COSINE, DIVERSE_TOPICAL_MIN_SHARED),
        };
        if cosine >= min_cosine && shared >= min_shared {
            let w = (LEARNED_SHARE * cosine).min(DIVERSE_LEARNED_CAP);
            best = Some(best.map_or(w, |b: f32| b.max(w)));
        }
    }
    best
}

/// Ajoute aux mots liés de la requête ceux que les documents ont appris (voir [`CONFIRM`]),
/// avec le niveau porté par `q.variety` ; en Diversifié, ajoute aussi les signatures et le
/// second pas validé.
pub fn add_learned<S: nknown_engine::Storage>(
    pipeline: &Pipeline,
    engine: &nknown_engine::Engine<S>,
    q: &mut Query,
) -> Vec<LearnedNote> {
    add_learned_in(pipeline, engine, q, q.variety)
}

/// Comme [`add_learned`], avec le niveau explicite ; en Diversifié, ajoute aussi les
/// signatures de définitions et le second pas validé.
pub fn add_learned_in<S: nknown_engine::Storage>(
    pipeline: &Pipeline,
    engine: &nknown_engine::Engine<S>,
    q: &mut Query,
    variety: Variety,
) -> Vec<LearnedNote> {
    let relax = Relax::for_variety(variety);
    q.variety = variety;
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
                    None => match learned_alone_weight(
                        relax.learned_alone,
                        silent,
                        l.kind,
                        l.cosine,
                        l.shared,
                    ) {
                        Some(w) => (w, false),
                        None => continue,
                    },
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
        term.related.truncate(relax.max_related);
        if relax.signatures {
            add_diverse_links(pipeline, term);
        }
    }
    notes.sort_by(|a, b| {
        a.term
            .cmp(&b.term)
            .then(b.weight.total_cmp(&a.weight))
            .then_with(|| a.word.cmp(&b.word))
    });
    notes
}

/// Entrée de contenu du lexique pour un mot de signature (catégorie la plus fréquente).
fn content_entry(pipeline: &Pipeline, lang: Lang, word: &str) -> Option<(Pos, String)> {
    let lex = pipeline.lexicon(lang)?;
    lex.lookup(word)
        .filter(|e| nknown_engine::ingest::is_content(e.pos))
        .max_by(|a, b| a.freq.total_cmp(&b.freq))
        .map(|e| (e.pos, e.lemma.to_owned()))
}

/// Trie et garde les `max` meilleurs candidats (poids décroissant, mot croissant).
fn keep_best(mut candidates: Vec<((Lang, String), f32)>, max: usize) -> Vec<((Lang, String), f32)> {
    candidates.sort_by(|a, b| {
        b.1.total_cmp(&a.1)
            .then_with(|| a.0.1.cmp(&b.0.1))
            .then_with(|| a.0.0.as_str().cmp(b.0.0.as_str()))
    });
    candidates.truncate(max);
    candidates
}

/// Diversifié : signatures de définitions validées par un lien retour, puis second pas
/// validé. Les deux plafonds s'appliquent après validation.
fn add_diverse_links(pipeline: &Pipeline, term: &mut QueryTerm) {
    let own: std::collections::HashSet<(Lang, String)> = term
        .readings
        .iter()
        .map(|(l, _, w)| (*l, w.to_lowercase()))
        .collect();
    let source_lemmas: Vec<String> = term
        .readings
        .iter()
        .filter(|(_, k, _)| matches!(k, TermKind::Word(p) if nknown_engine::ingest::is_content(*p)))
        .map(|(_, _, lemma)| lemma.to_lowercase())
        .collect();
    let mut added: HashMap<(Lang, String), f32> = term
        .related
        .iter()
        .map(|(l, w, p)| ((*l, w.clone()), *p))
        .collect();

    // Signatures de définition : un mot présent dans au moins deux sens est un indice.
    let mut signatures: HashMap<(Lang, String), f32> = HashMap::new();
    for (lang, kind, lemma) in &term.readings {
        if !matches!(kind, TermKind::Word(p) if nknown_engine::ingest::is_content(*p)) {
            continue;
        }
        let Some(rel) = pipeline.relations(*lang) else {
            continue;
        };
        let record = rel.record(lemma);
        let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
        for sense in &record.senses {
            for word in &sense.signature {
                *counts.entry(word).or_default() += 1;
            }
        }
        for (word, n) in counts {
            if n < SIGNATURE_MIN_SENSES {
                continue;
            }
            let Some((_pos, candidate)) = content_entry(pipeline, *lang, word) else {
                continue;
            };
            let key = (*lang, candidate.to_lowercase());
            if own.contains(&key) || added.contains_key(&key) {
                continue;
            }
            // Le mot de définition ne compte que si son propre lien revient vers le mot
            // source : « vulnerability » définit « flaw » et « vulnerability » → « flaw »
            // existe. Cela écarte « level », « high »… des sens voisins.
            if !rel
                .record(word)
                .links
                .iter()
                .any(|l| l.word == lemma.as_str() && l.weight >= SIGNATURE_REVERSE)
            {
                continue;
            }
            let weight = (SIGNATURE_BASE + SIGNATURE_PER_SENSE * n as f32).min(SIGNATURE_CAP);
            let slot = signatures.entry(key).or_insert(0.0);
            *slot = slot.max(weight);
        }
    }
    for (key, weight) in keep_best(signatures.into_iter().collect(), SIGNATURE_MAX) {
        added.entry(key).or_insert(weight);
    }

    // Second pas : les voisins des meilleurs mots liés, à poids faible, validés par un lien
    // retour vers le mot source.
    let mut near = term.related.clone();
    near.sort_by(|a, b| b.2.total_cmp(&a.2).then_with(|| a.1.cmp(&b.1)));
    let mut second: HashMap<(Lang, String), f32> = HashMap::new();
    for (lang, word, p) in near.into_iter().take(SECOND_HOP_FROM) {
        let Some(rel) = pipeline.relations(lang) else {
            continue;
        };
        for (neighbour, q) in rel.related(&word).into_iter().take(SECOND_HOP_PER_WORD) {
            if q < SECOND_STEP_MIN {
                continue;
            }
            let key = (lang, neighbour.to_owned());
            if own.contains(&key) || added.contains_key(&key) {
                continue;
            }
            if !source_lemmas.iter().any(|lemma| {
                rel.record(neighbour)
                    .links
                    .iter()
                    .any(|l| l.word == lemma.as_str() && l.weight >= SECOND_STEP_REVERSE)
            }) {
                continue;
            }
            let weight = (SECOND_STEP_SHARE * p * q).min(SECOND_STEP_CAP);
            let slot = second.entry(key).or_insert(0.0);
            *slot = slot.max(weight);
        }
    }
    for (key, weight) in keep_best(second.into_iter().collect(), SECOND_STEP_MAX) {
        added.entry(key).or_insert(weight);
    }

    term.related = added.into_iter().map(|((l, w), p)| (l, w, p)).collect();
    term.related.sort_by(|a, b| {
        b.2.total_cmp(&a.2)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.0.as_str().cmp(b.0.as_str()))
    });
    term.related.truncate(DIVERSE_MAX_RELATED);
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
    build_query_with_variety(pipeline, None, text, Variety::Normal)
}

/// Comme [`build_query`], avec la détection de langue pour étiqueter la requête en contexte.
pub fn build_query_with(pipeline: &Pipeline, detector: Option<&LangDetector>, text: &str) -> Query {
    build_query_with_variety(pipeline, detector, text, Variety::Normal)
}

/// Comme [`build_query_with`], avec un niveau de variété explicite.
pub fn build_query_with_variety(
    pipeline: &Pipeline,
    detector: Option<&LangDetector>,
    text: &str,
    variety: Variety,
) -> Query {
    build_query_inner(pipeline, detector, text, None, variety)
}

/// Comme [`build_query_with`], en laissant les documents de `engine` (liens appris) départager
/// les sens des mots (voir [`choose_senses`]).
pub fn build_query_in<S: nknown_engine::Storage>(
    pipeline: &Pipeline,
    detector: Option<&LangDetector>,
    text: &str,
    engine: &nknown_engine::Engine<S>,
) -> Query {
    build_query_in_variety(pipeline, detector, text, engine, Variety::Normal)
}

/// Comme [`build_query_in`], avec un niveau de variété explicite.
pub fn build_query_in_variety<S: nknown_engine::Storage>(
    pipeline: &Pipeline,
    detector: Option<&LangDetector>,
    text: &str,
    engine: &nknown_engine::Engine<S>,
    variety: Variety,
) -> Query {
    let corpus = |lang: Lang, kind: TermKind, lemma: &str| -> Vec<(String, f32)> {
        engine
            .learned(lang, kind, lemma)
            .unwrap_or_default()
            .into_iter()
            .map(|l| (l.lemma, l.cosine))
            .collect()
    };
    build_query_inner(pipeline, detector, text, Some(&corpus), variety)
}

fn build_query_inner(
    pipeline: &Pipeline,
    detector: Option<&LangDetector>,
    text: &str,
    corpus: Option<Corpus<'_>>,
    variety: Variety,
) -> Query {
    let relax = Relax::for_variety(variety);
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
            let related = related_words(pipeline, &readings, tag, None, relax);
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
    choose_senses(pipeline, &mut terms, &tags, corpus, relax);
    kinds.sort_unstable();
    kinds.dedup();
    Query {
        terms,
        intent,
        recency,
        kinds,
        variety,
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

/// Octet de `text` au début du `char_index`-ième caractère (fin du texte au-delà).
fn char_byte(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map_or(text.len(), |(at, _)| at)
}

/// La coupure à `at` tombe-t-elle au milieu d'un mot (deux caractères qui ne sont pas des
/// espaces se font face) ?
fn splits_word(text: &str, at: usize) -> bool {
    let before = text.get(..at).and_then(|s| s.chars().next_back());
    let after = text.get(at..).and_then(|s| s.chars().next());
    matches!((before, after), (Some(a), Some(b)) if !a.is_whitespace() && !b.is_whitespace())
}

/// Fin du mot qui commence avant `from` : juste avant l'espace suivant, ou fin du texte.
fn word_end(text: &str, from: usize) -> usize {
    text.get(from..)
        .and_then(|s| s.char_indices().find(|(_, c)| c.is_whitespace()))
        .map_or(text.len(), |(at, _)| from + at)
}

/// Texte replié en minuscules, avec l'index du caractère d'origine pour chaque caractère replié.
///
/// La casse peut changer le nombre de caractères (`İ` U+0130 devient `i` + point combinant) :
/// garder la correspondance en caractères d'origine évite de décaler la fenêtre.
fn folded_with_origin(text: &str) -> (String, Vec<usize>) {
    let mut folded = String::with_capacity(text.len());
    let mut origin = Vec::with_capacity(text.len());
    for (char_index, (_, c)) in text.char_indices().enumerate() {
        for lower in c.to_lowercase() {
            folded.push(lower);
            origin.push(char_index);
        }
    }
    (folded, origin)
}

/// Texte du passage montré dans la fiche d'un résultat : fenêtre d'environ `width` caractères
/// centrée sur la première correspondance, coupée aux limites de mots, espaces et retours à la
/// ligne conservés (la fiche est pré-encadrée et défilante, contrairement à [`snippet`]).
///
/// Le texte n'est pas échappé : l'interface s'en charge. Les coupures tombent toujours sur une
/// limite de caractère, y compris en écriture multioctet (accents, CJK).
pub fn passage_text(text: &str, words: &[String], width: usize) -> String {
    let total = text.chars().count();
    if width == 0 || total <= width {
        return text.to_owned();
    }
    let (folded, origin) = folded_with_origin(text);
    // Première correspondance : caractère d'origine et longueur en caractères d'origine.
    let first = words
        .iter()
        .filter_map(|w| {
            let needle = w.to_lowercase();
            if needle.is_empty() {
                return None;
            }
            let at = folded.find(&needle)?;
            let start = folded[..at].chars().count();
            let end = start + needle.chars().count();
            let start_char = origin.get(start).copied()?;
            let end_char = origin.get(end).copied().unwrap_or(total);
            // Un motif qui ne couvre qu'une partie d'un caractère replié (`i` dans `İ`)
            // couvre tout de même ce caractère d'origine.
            Some((start_char, (end_char - start_char).max(1)))
        })
        .min_by_key(|&(at, _)| at);
    let (hit_chars, hit_len) = first.map_or((0, 0), |(at, n)| (at.min(total), n));
    // Fenêtre centrée sur la correspondance ; sans correspondance, le début du texte.
    let start_char = match first {
        Some(_) => hit_chars.saturating_sub(width / 2).min(total - width),
        None => 0,
    };
    let end_char = start_char + width;
    let hit_start = char_byte(text, hit_chars);
    let hit_end = char_byte(text, (hit_chars + hit_len).min(total));
    let mut start = char_byte(text, start_char);
    let mut end = char_byte(text, end_char);

    // Début au milieu d'un mot : avancer au mot suivant, sans dépasser la correspondance.
    if start > 0 && splits_word(text, start) {
        if let Some((ws, c)) = text[start..end]
            .char_indices()
            .find(|(_, c)| c.is_whitespace())
        {
            let next = start + ws + c.len_utf8();
            if next <= hit_start && next < end {
                start = next;
            }
        }
    }
    // Fin au milieu d'un mot : reculer à la fin du mot précédent ; si cela couperait la
    // correspondance, aller jusqu'à la fin de son mot.
    if end < text.len() && splits_word(text, end) {
        let cut = text[start..end]
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_whitespace())
            .map(|(at, _)| start + at);
        end = match cut {
            Some(at) if at >= hit_end => at,
            Some(_) => word_end(text, end.max(hit_end)),
            None => end,
        };
    }

    let mut body = &text[start..end];
    if start > 0 {
        body = body.trim_start();
    }
    if end < text.len() {
        body = body.trim_end();
    }
    let before = if start > 0 { "…" } else { "" };
    let after = if end < text.len() { "…" } else { "" };
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
    fn passage_text_returns_short_text_unchanged() {
        let text = "Une phrase\ncourte, avec <b>balises</b> et accents : élève.";
        assert_eq!(passage_text(text, &["élève".to_owned()], 100), text);
        // Largeur comptée en caractères, pas en octets.
        let accents = "é".repeat(30);
        assert_eq!(passage_text(&accents, &[], 30), accents);
    }

    #[test]
    fn passage_text_centers_the_window_on_the_match() {
        let text = format!("{} aiguille {}", "mot ".repeat(200), "fin ".repeat(200));
        let s = passage_text(&text, &["aiguille".to_owned()], 80);
        assert!(s.starts_with('…') && s.ends_with('…'), "{s}");
        assert!(s.contains(" aiguille "), "{s}");
        assert!(
            s.chars().count() <= 82,
            "{} caractères : {s}",
            s.chars().count()
        );
    }

    #[test]
    fn passage_text_keeps_ellipses_only_when_clipped() {
        let start = passage_text(&"début ".repeat(100), &[], 40);
        assert!(start.starts_with("début "), "{start:?}");
        assert!(!start.starts_with('…') && start.ends_with('…'), "{start:?}");

        let text = format!("{} cible", "début ".repeat(100));
        let end = passage_text(&text, &["cible".to_owned()], 50);
        assert!(end.starts_with('…') && end.ends_with("cible"), "{end:?}");
        assert!(!end.ends_with('…'), "{end:?}");
    }

    #[test]
    fn passage_text_preserves_whitespace_and_newlines() {
        let text = format!(
            "{}premier paragraphe\n\nligne   deux\nligne trois {}",
            "avant ".repeat(50),
            "après ".repeat(50)
        );
        // Trois espaces et deux retours à la ligne : rien n'est replié.
        let s = passage_text(&text, &["deux".to_owned()], 100);
        assert!(s.contains("\n\nligne   deux\nligne trois"), "{s:?}");
    }

    #[test]
    fn passage_text_handles_multibyte_text() {
        let text = format!("{} élève {}", "é ".repeat(200), "é ".repeat(200));
        let s = passage_text(&text, &["élève".to_owned()], 60);
        assert!(s.contains("élève"), "{s}");
        assert!(s.starts_with('…') && s.ends_with('…'), "{s}");

        let text = format!("{} 検索 {}", "漢字 ".repeat(200), "テスト ".repeat(200));
        let s = passage_text(&text, &["検索".to_owned()], 60);
        assert!(s.contains("検索"), "{s}");
        assert!(s.starts_with('…') && s.ends_with('…'), "{s}");
    }

    #[test]
    fn passage_text_keeps_alignment_when_lowercase_expands() {
        // Chaque `İ` devient deux caractères en minuscules : la correspondance doit rester
        // calculée sur les caractères d'origine, pas sur les octets repliés.
        let text = format!("{}cible {}", "İ ".repeat(200), "fin ".repeat(200));
        let s = passage_text(&text, &["cible".to_owned()], 60);
        assert!(s.contains("cible"), "{s:?}");
        assert!(s.starts_with('…') && s.ends_with('…'), "{s:?}");
    }

    #[test]
    fn passage_text_finds_a_match_beyond_the_start_window() {
        let text = format!("{} cible", "début ".repeat(100));
        let s = passage_text(&text, &["cible".to_owned()], 50);
        assert!(s.contains("cible"), "{s}");
        assert!(s.starts_with('…'), "{s}");
    }

    #[test]
    fn passage_text_without_match_returns_the_start_window() {
        let text = "début ".repeat(100);
        let s = passage_text(&text, &["absent".to_owned()], 40);
        assert!(s.starts_with("début "), "{s:?}");
        assert!(s.ends_with('…'), "{s:?}");
        assert!(!s.contains("absent"), "{s:?}");
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

    fn lexicon(words: &[&str]) -> nknown_nlp::Lexicon {
        use nknown_core::analyze::Feats;
        use nknown_nlp::lexicon::LexiconBuilder;
        let mut b = LexiconBuilder::new();
        for word in words {
            b.insert(word, word, Pos::Noun, Feats::default());
        }
        b.build().unwrap()
    }

    #[test]
    fn normal_variety_matches_legacy_entry_points() {
        let pipeline = Pipeline::default().with_lexicon(Lang::FRA, lexicon(&["carte"]));
        let engine = nknown_engine::Engine::new(nknown_engine::MemStorage::default());
        assert_eq!(
            build_query(&pipeline, "carte"),
            build_query_with_variety(&pipeline, None, "carte", Variety::Normal)
        );
        assert_eq!(
            build_query_in(&pipeline, None, "carte", &engine),
            build_query_in_variety(&pipeline, None, "carte", &engine, Variety::Normal)
        );
        assert_eq!(build_query(&pipeline, "carte").variety, Variety::Normal);
    }

    /// Normal reste la référence sur une base synthétique : mêmes contenus et poids de liens
    /// que le comportement historique, un document direct devant un document lié, et des
    /// niveaux Strict / Diversifié qui resserrent ou élargissent dans le sens documenté.
    #[test]
    fn normal_variety_fixture_keeps_documented_weights() {
        use nknown_core::item::RawItem;
        use nknown_core::source::{SourceId, SourceKind};
        use nknown_core::time::Timestamp;
        use nknown_engine::{Document, Engine, MemStorage};
        use nknown_nlp::relations::{BuildOptions, Evidence};

        let mut r = nknown_nlp::RelationsBuilder::new();
        r.link("alpha", "beta", Evidence::Synonym, 1.0);
        r.link("alpha", "gamma", Evidence::Related, 1.0);
        r.link("alpha", "delta", Evidence::Related, 0.5);
        let rel = r
            .build(
                BuildOptions {
                    per_lemma: 16,
                    min_weight: 0.1,
                },
                |_| 1.0,
            )
            .unwrap();
        let pipeline = Pipeline::default()
            .with_lexicon(Lang::FRA, lexicon(&["alpha", "beta", "gamma", "delta"]))
            .with_relations(Lang::FRA, rel);

        let related = |variety| -> Vec<(String, f32)> {
            build_query_with_variety(&pipeline, None, "alpha", variety).terms[0]
                .related
                .iter()
                .map(|(_, w, p)| (w.clone(), *p))
                .collect()
        };
        // Poids du constructeur : force du type (`Synonym` 0,9 ; `Related` 0,45) × force
        // d'indice, puis quantifiés une seconde fois sur 255 par le fichier de relations.
        let normal = related(Variety::Normal);
        assert_eq!(
            normal,
            [
                ("beta".to_owned(), 230.0 / 255.0),
                ("gamma".to_owned(), 115.0 / 255.0),
            ]
        );
        assert_eq!(
            related(Variety::Strict),
            [("beta".to_owned(), 230.0 / 255.0)]
        );
        let diverse = related(Variety::Diverse);
        assert_eq!(
            diverse,
            [
                ("beta".to_owned(), 230.0 / 255.0),
                ("gamma".to_owned(), 115.0 / 255.0),
                ("delta".to_owned(), 58.0 / 255.0),
            ]
        );

        // Un document direct doit battre un document qui ne contient qu'un mot lié.
        let mut engine = Engine::new(MemStorage::default()).with_stemmer(langid::stem);
        let now = Timestamp::now();
        for (external_id, text) in [
            ("direct", "alpha sujet"),
            ("related", "beta sujet"),
            ("weak", "gamma sujet"),
        ] {
            let analysis = pipeline.analyze_light(Lang::FRA, text);
            let item = RawItem {
                external_id: Some(external_id.to_owned()),
                ..RawItem::new(SourceId("test:normal".to_owned()), text.to_owned(), now)
            };
            engine
                .ingest(Document {
                    item: &item,
                    kind: SourceKind::LocalFile,
                    lang: Lang::FRA,
                    analysis: &analysis,
                    title: None,
                })
                .unwrap();
        }
        let q = build_query_with_variety(&pipeline, None, "alpha", Variety::Normal);
        assert_eq!(q.terms.len(), 1);
        assert_eq!(q.terms[0].text, "alpha");
        let hits = engine.search(&q, now, 10).unwrap();
        let score = |id: &str| {
            hits.iter()
                .find(|h| h.doc.external_id.as_deref() == Some(id))
                .map(|h| h.score)
        };
        let direct = score("direct").expect("document direct trouvé");
        let linked = score("related").expect("document lié trouvé");
        assert!(direct > linked, "direct {direct} doit battre lié {linked}");
    }

    /// Strict ne garde que les liens forts du dictionnaire, et seulement s'ils résistent au
    /// contexte ; Normal garde les liens faibles et non soutenus.
    #[test]
    fn strict_filters_weak_and_unsupported_links() {
        use nknown_nlp::relations::{BuildOptions, Evidence, Origin};
        let mut r = nknown_nlp::RelationsBuilder::new();
        let entry = r.entry("alpha");
        let unsupported = r.sense(entry);
        r.sign(unsupported, "autre");
        let supported = r.sense(entry);
        r.sign(supported, "ctx");
        r.link_from(
            "alpha",
            "beta",
            Evidence::Synonym,
            0.9,
            Origin::Sense(unsupported),
        );
        r.link("alpha", "faible", Evidence::Related, 0.8);
        r.link("alpha", "fort", Evidence::Synonym, 1.0);
        let rel = r.build(BuildOptions::default(), |_| 1.0).unwrap();
        let pipeline = Pipeline::default()
            .with_lexicon(Lang::FRA, lexicon(&["alpha", "ctx"]))
            .with_relations(Lang::FRA, rel);

        let related = |variety| -> Vec<String> {
            build_query_with_variety(&pipeline, None, "alpha ctx", variety).terms[0]
                .related
                .iter()
                .map(|(_, w, _)| w.clone())
                .collect()
        };
        let strict = related(Variety::Strict);
        assert!(!strict.iter().any(|w| w == "faible"), "{strict:?}");
        // Lien du dictionnaire fort, mais que le contexte du mauvais sens enterre.
        assert!(!strict.iter().any(|w| w == "beta"), "{strict:?}");
        assert!(strict.iter().any(|w| w == "fort"), "{strict:?}");

        let normal = related(Variety::Normal);
        assert!(normal.iter().any(|w| w == "faible"), "{normal:?}");
        assert!(normal.iter().any(|w| w == "beta"), "{normal:?}");
    }

    /// Le cas `php flaw` : « vulnerability » définit « flaw » et son lien retour existe ;
    /// « level » n'a pas de retour et n'est pas ajouté. Le dictionnaire de `flaw` n'a pas
    /// `vulnerability` en sortie (comme la base réelle), 16 liens plus forts occupant la fiche.
    #[test]
    fn diverse_adds_signature_with_reverse_link() {
        use nknown_nlp::relations::{BuildOptions, Evidence, Origin};
        let mut r = nknown_nlp::RelationsBuilder::new();
        let entry = r.entry("flaw");
        let first = r.sense(entry);
        r.sign(first, "vulnerability");
        for _ in 0..2 {
            let sense = r.sense(entry);
            r.sign(sense, "vulnerability");
        }
        let sense = r.sense(entry);
        r.sign(sense, "level");
        r.link_from(
            "flaw",
            "vulnerability",
            Evidence::Gloss,
            1.0,
            Origin::Sense(first),
        );
        for i in 0..16 {
            let origin = if i == 0 {
                Origin::Sense(first)
            } else {
                Origin::None
            };
            r.link_from("flaw", &format!("mot{i}"), Evidence::Synonym, 1.0, origin);
        }
        let rel = r.build(BuildOptions::default(), |_| 1.0).unwrap();
        assert!(
            rel.record("flaw")
                .links
                .iter()
                .all(|l| l.word != "vulnerability"),
            "{:?}",
            rel.record("flaw").links
        );
        assert!(
            rel.record("vulnerability")
                .links
                .iter()
                .any(|l| l.word == "flaw" && l.weight >= 0.5)
        );
        let pipeline = Pipeline::default()
            .with_lexicon(Lang::FRA, lexicon(&["flaw", "vulnerability", "level"]))
            .with_relations(Lang::FRA, rel);
        let engine = nknown_engine::Engine::new(nknown_engine::MemStorage::default());

        let mut diverse =
            build_query_in_variety(&pipeline, None, "flaw", &engine, Variety::Diverse);
        add_learned(&pipeline, &engine, &mut diverse);
        let weight = |q: &Query, word: &str| {
            q.terms[0]
                .related
                .iter()
                .find(|(_, w, _)| w == word)
                .map(|(_, _, p)| *p)
        };
        let vulnerability = weight(&diverse, "vulnerability").expect("signature ajoutée");
        assert!((vulnerability - 0.28).abs() < 1e-6, "{vulnerability}");
        assert_eq!(weight(&diverse, "level"), None);

        let mut normal = build_query_in_variety(&pipeline, None, "flaw", &engine, Variety::Normal);
        add_learned(&pipeline, &engine, &mut normal);
        assert_eq!(weight(&normal, "vulnerability"), None);
    }

    #[test]
    fn diverse_accepts_topical_learned_link() {
        use nknown_engine::LinkKind;
        let diverse = Relax::for_variety(Variety::Diverse).learned_alone;
        let normal = Relax::for_variety(Variety::Normal).learned_alone;
        let strict = Relax::for_variety(Variety::Strict).learned_alone;
        let w = learned_alone_weight(diverse, false, LinkKind::Topical, 0.46, 27);
        assert!((w.unwrap() - 0.276).abs() < 1e-6);
        assert_eq!(
            learned_alone_weight(normal, false, LinkKind::Topical, 0.46, 27),
            None
        );
        assert_eq!(
            learned_alone_weight(strict, false, LinkKind::Topical, 0.46, 27),
            None
        );
        // Seuils renforcés du Diversifié.
        assert_eq!(
            learned_alone_weight(diverse, false, LinkKind::Topical, 0.44, 27),
            None
        );
        assert_eq!(
            learned_alone_weight(diverse, false, LinkKind::Topical, 0.46, 14),
            None
        );
        assert_eq!(
            learned_alone_weight(diverse, false, LinkKind::Syntactic, 0.50, 20),
            None
        );
        assert!(learned_alone_weight(diverse, false, LinkKind::Syntactic, 0.56, 12).is_some());
    }

    #[test]
    fn related_caps_follow_variety() {
        use nknown_nlp::relations::{BuildOptions, Evidence};
        let mut r = nknown_nlp::RelationsBuilder::new();
        for i in 0..60 {
            r.link("alpha", &format!("mot{i}"), Evidence::Synonym, 1.0);
        }
        let opts = BuildOptions {
            per_lemma: 64,
            min_weight: 0.1,
        };
        let rel = r.build(opts, |_| 1.0).unwrap();
        let pipeline = Pipeline::default()
            .with_lexicon(Lang::FRA, lexicon(&["alpha"]))
            .with_relations(Lang::FRA, rel);
        for (variety, expected) in [
            (Variety::Strict, STRICT_MAX_RELATED),
            (Variety::Normal, MAX_RELATED),
            (Variety::Diverse, DIVERSE_MAX_RELATED),
        ] {
            let q = build_query_with_variety(&pipeline, None, "alpha", variety);
            assert_eq!(q.terms[0].related.len(), expected, "{variety:?}");
        }
    }
}
