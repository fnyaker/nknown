//! Liens appris du corpus : deux lemmes qui s'emploient dans les mêmes contextes.
//!
//! Le dictionnaire ignore le jargon d'un domaine et l'usage réel des mots. Les documents de
//! l'utilisateur le connaissent. On mesure la ressemblance des contextes de deux mots
//! (cosinus de vecteurs PPMI), pas leur co-occurrence directe : « élection » et « dimanche »
//! apparaissent ensemble sans être interchangeables.
//!
//! Deux sortes de contextes, de fiabilité différente :
//! - **syntaxiques** ([`LinkKind::Syntactic`]) : les adjectifs qui qualifient un nom, les noms
//!   qu'un adjectif qualifie. Deux noms qualifiés par les mêmes adjectifs se remplacent
//!   (« élection » et « scrutin » : présidentiel, municipal, anticipé) ; « candidat », du même
//!   sujet, a d'autres adjectifs. Ces liens peuvent élargir une requête à eux seuls ;
//! - **thématiques** ([`LinkKind::Topical`]) : les autres mots du passage. Ils rapprochent
//!   aussi les mots d'un même sujet (« élection », « candidat », « voix »), donc ne servent qu'à
//!   confirmer un lien du dictionnaire, jamais à en créer un.
//!
//! Garde-fous contre les faux positifs :
//! - ni noms propres (deux personnalités ont des contextes semblables sans être la même
//!   personne), ni nombres, ni termes de code ; liens seulement entre mots de même catégorie
//!   et de même langue ;
//! - mots assez fréquents pour avoir des contextes fiables, mais pas omniprésents ;
//! - un document compte une fois par paire de mots (un long texte répétitif ne fabrique pas
//!   de lien), et les passages répétés d'un document à l'autre (pieds de page, mentions
//!   légales) sont écartés ;
//! - assez de contextes communs, et une similarité minimale ;
//! - la co-occurrence directe des deux mots ne compte pas dans leur similarité.
//!
//! Les antonymes (« hausse » / « baisse » : mêmes contextes, sens contraires) sont écartés à la
//! requête, par le dictionnaire, et un lien appris seul pèse peu (voir `nknown-service`).
//!
//! Stockage : `R | id du lemme` → liste de (id du lemme lié u64, cosinus f32, contextes
//! communs u16, sorte u8) ; `M learned_passages` : nombre de passages au dernier calcul.

use std::collections::HashMap;

use nknown_core::analyze::Pos;
use nknown_core::lang::Lang;

use crate::graph::{Edge, EdgeKind, NodeId};
use crate::schema::{self, LemmaRecord, TermKind};
use crate::storage::{Op, Storage};
use crate::{Engine, Result, decode};

/// Préfixe des liens appris.
pub const LEARNED: u8 = b'R';
/// Compteur : passages de la base au dernier apprentissage.
pub const LEARNED_PASSAGES: &str = "learned_passages";
/// Date (secondes Unix) du dernier apprentissage.
pub const LEARNED_AT: &str = "learned_at";

/// Ce que la base contient de liens appris.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LearnedSummary {
    /// Mots qui ont au moins un lien appris.
    pub words: u64,
    pub syntactic: u64,
    pub topical: u64,
    /// Passages de la base au dernier apprentissage (0 : jamais).
    pub at_passages: u64,
    /// Date du dernier apprentissage (secondes Unix ; 0 : inconnue).
    pub at: u64,
}

/// Réglages de l'apprentissage.
#[derive(Debug, Clone, Copy)]
pub struct LearnOptions {
    /// Passages (dans l'échantillon) où un mot doit apparaître, au moins.
    pub min_df: u64,
    /// Part des passages au-delà de laquelle un mot est trop banal.
    pub max_share: f64,
    /// Mots étudiés, au plus (les plus fréquents).
    pub max_vocab: usize,
    /// Passages lus, au plus (échantillon régulier au-delà).
    pub max_passages: usize,
    /// Mots gardés par passage (les plus spécifiques).
    pub terms_per_passage: usize,
    /// Contextes gardés par mot (PPMI les plus forts).
    pub features: usize,
    /// Contextes communs minimaux et cosinus minimal d'un lien.
    pub min_shared: usize,
    pub min_cosine: f32,
    /// Liens gardés par mot.
    pub per_lemma: usize,
}

impl Default for LearnOptions {
    fn default() -> Self {
        Self {
            min_df: 15,
            max_share: 0.05,
            max_vocab: 30_000,
            max_passages: 150_000,
            terms_per_passage: 24,
            features: 64,
            min_shared: 8,
            min_cosine: 0.3,
            per_lemma: 8,
        }
    }
}

/// Sorte de contexte d'où vient un lien appris.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinkKind {
    /// Mots du même passage : confirme un lien du dictionnaire, n'en crée pas.
    Topical = 0,
    /// Adjectifs d'un nom, noms d'un adjectif : lien utilisable seul.
    Syntactic = 1,
}

impl LinkKind {
    fn from_u8(b: u8) -> Option<Self> {
        match b {
            0 => Some(LinkKind::Topical),
            1 => Some(LinkKind::Syntactic),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LearnStats {
    pub passages: usize,
    /// Passages écartés comme répétés (pieds de page…).
    pub boilerplate: usize,
    pub docs: usize,
    pub vocab: usize,
    pub links: usize,
    /// Paires (nom, adjectif) lues, et liens syntaxiques.
    pub qualified: usize,
    pub syntactic: usize,
}

/// Similarité d'un passage avec un autre au-delà de laquelle il est tenu pour répété.
const REPEATED: f32 = 0.95;
/// Lissage de la distribution des contextes (PPMI, Levy et al. 2015).
const CONTEXT_SMOOTHING: f64 = 0.75;
/// Un contexte partagé par trop de mots ne départage rien (et coûte cher).
const MAX_POSTING: usize = 4000;
/// Co-occurrences minimales pour qu'un contexte compte.
const MIN_PAIR: u32 = 2;
/// Contextes syntaxiques distincts minimaux d'un mot, et contextes communs d'un lien.
const MIN_SYNTACTIC_CONTEXTS: usize = 4;
const MIN_SYNTACTIC_SHARED: usize = 3;

/// Octets d'un lien enregistré.
const RECORD: usize = 15;

/// Lien appris, lu à la requête.
#[derive(Debug, Clone, PartialEq)]
pub struct LearnedLink {
    pub lemma: String,
    pub pos: Pos,
    pub cosine: f32,
    /// Contextes communs.
    pub shared: u16,
    pub kind: LinkKind,
}

/// Liens appris, prêts à être écrits.
#[derive(Debug, Default)]
pub struct Learned {
    pub stats: LearnStats,
    links: HashMap<u64, Vec<(u64, f32, u16, LinkKind)>>,
    passages: u64,
}

fn id_at(key: &[u8], at: usize) -> Option<u64> {
    key.get(at..at + 8)?.try_into().ok().map(u64::from_be_bytes)
}

/// Catégories apprises : noms communs, verbes, adjectifs.
fn learnable(pos: u8) -> Option<Pos> {
    match Pos::from_u8(pos)? {
        p @ (Pos::Noun | Pos::Verb | Pos::Adj) => Some(p),
        _ => None,
    }
}

/// Vecteurs PPMI normalisés, triés par contexte. `counts` : (ligne, contexte, nombre) ;
/// `row_total` et `ctx_total` : marges.
fn ppmi_rows(
    rows: usize,
    counts: impl Iterator<Item = (usize, usize, u32)>,
    row_total: &[u32],
    ctx_total: &[u32],
    features: usize,
) -> Vec<Vec<(u32, f32)>> {
    let total_ctx: f64 = ctx_total
        .iter()
        .map(|&c| f64::from(c).powf(CONTEXT_SMOOTHING))
        .sum();
    let mut out: Vec<Vec<(u32, f32)>> = vec![Vec::new(); rows];
    for (r, c, n) in counts {
        if n < MIN_PAIR || total_ctx <= 0.0 {
            continue;
        }
        let p_cr = f64::from(n) / f64::from(row_total[r].max(1));
        let p_c = f64::from(ctx_total[c]).powf(CONTEXT_SMOOTHING) / total_ctx;
        let x = (p_cr / p_c).ln();
        if x > 0.0 {
            out[r].push((c as u32, x as f32));
        }
    }
    for row in &mut out {
        row.sort_unstable_by(|x, y| y.1.total_cmp(&x.1));
        row.truncate(features);
        let norm = row.iter().map(|(_, w)| w * w).sum::<f32>().sqrt();
        if norm > 0.0 {
            for (_, w) in row.iter_mut() {
                *w /= norm;
            }
        }
        row.sort_unstable_by_key(|&(c, _)| c);
    }
    out
}

/// Pour chaque ligne, les lignes du même groupe (langue, catégorie) aux contextes les plus
/// semblables : (ligne, cosinus, contextes communs).
fn similar<G: PartialEq>(
    rows: &[Vec<(u32, f32)>],
    contexts: usize,
    group: &[G],
    min_shared: usize,
    min_cosine: f32,
    per_row: usize,
) -> Vec<Vec<(u32, f32, u16)>> {
    let mut postings: Vec<Vec<(u32, f32)>> = vec![Vec::new(); contexts];
    for (w, row) in rows.iter().enumerate() {
        for &(c, x) in row {
            postings[c as usize].push((w as u32, x));
        }
    }
    let mut acc: HashMap<u32, (f32, u16)> = HashMap::new();
    rows.iter()
        .enumerate()
        .map(|(w, row)| {
            if row.len() < min_shared {
                return Vec::new();
            }
            acc.clear();
            for &(c, x) in row {
                let posting = &postings[c as usize];
                if posting.len() > MAX_POSTING {
                    continue;
                }
                for &(o, y) in posting {
                    if o as usize != w {
                        let e = acc.entry(o).or_insert((0.0, 0));
                        e.0 += x * y;
                        e.1 += 1;
                    }
                }
            }
            let mut list: Vec<(u32, f32, u16)> = acc
                .iter()
                .filter(|(o, (cos, shared))| {
                    *cos >= min_cosine
                        && usize::from(*shared) >= min_shared
                        && group[**o as usize] == group[w]
                })
                .map(|(&o, &(cos, shared))| (o, cos.min(1.0), shared))
                .collect();
            list.sort_by(|x, y| y.1.total_cmp(&x.1).then(x.0.cmp(&y.0)));
            list.truncate(per_row);
            list
        })
        .collect()
}

/// Lemme apprenable : catégorie, langue, forme (ni nombre, ni identifiant, au moins 3 lettres).
fn lemma_info(rec: &LemmaRecord) -> Option<(Lang, Pos)> {
    let pos = learnable(rec.pos)?;
    let lang = Lang::from_code(&rec.lang)?;
    let chars = rec.lemma.chars().count();
    // Ni nombre, ni identifiant, ni lemme de plusieurs mots ou à apostrophe (artefacts de
    // lemmatisation : « node' », « a long time »).
    let odd = |c: char| c.is_numeric() || c.is_whitespace() || matches!(c, '_' | '\'' | '’');
    if chars < 3 || rec.lemma.chars().any(odd) {
        return None;
    }
    Some((lang, pos))
}

impl<S: Storage> Engine<S> {
    /// Calcule les liens appris (lecture seule : peut tourner pendant les recherches).
    pub fn compute_learned(&self, opts: LearnOptions) -> Result<Learned> {
        let total_passages = schema::u64_value(self.get(&schema::meta_key(schema::PASSAGES))?);
        // Documents, triés : le document d'un passage est le plus grand id de document
        // inférieur au sien (les passages suivent leur document).
        let mut docs: Vec<u64> = Vec::new();
        self.visit(&[schema::DOC], |k, _| {
            docs.extend(id_at(k, 1));
            Ok(true)
        })?;
        let doc_of = |p: u64| -> Option<usize> { docs.partition_point(|&d| d < p).checked_sub(1) };
        let mut repeated_cache: HashMap<u64, bool> = HashMap::new();
        let mut repeated = |p: u64| -> Result<bool> {
            if let Some(&r) = repeated_cache.get(&p) {
                return Ok(r);
            }
            let r = self
                .scan(&Edge::prefix(NodeId(p), EdgeKind::Similar))?
                .iter()
                .any(|(_, w)| schema::f32_value(w) >= REPEATED);
            repeated_cache.insert(p, r);
            Ok(r)
        };
        let mut stats = LearnStats::default();
        let mut links: HashMap<u64, Vec<(u64, f32, u16, LinkKind)>> = HashMap::new();

        // ---- Contextes syntaxiques : paires (nom, adjectif), une fois par document.
        let mut info: HashMap<u64, Option<(Lang, Pos)>> = HashMap::new();
        let mut triples: Vec<(u64, u64, usize)> = Vec::new();
        self.visit(&[schema::QUALIFIED], |k, _| {
            let (Some(noun), Some(adj), Some(pid)) = (id_at(k, 1), id_at(k, 9), id_at(k, 17))
            else {
                return Ok(true);
            };
            stats.qualified += 1;
            for id in [noun, adj] {
                if let std::collections::hash_map::Entry::Vacant(e) = info.entry(id) {
                    let rec = self.get(&schema::id_key(schema::NODE, NodeId(id)))?;
                    e.insert(
                        rec.and_then(|r| decode::<LemmaRecord>(&r).ok())
                            .and_then(|r| lemma_info(&r)),
                    );
                }
            }
            let ok = matches!(info[&noun], Some((_, Pos::Noun)))
                && matches!(info[&adj], Some((_, Pos::Adj)));
            if ok
                && !repeated(pid)?
                && let Some(d) = doc_of(pid)
            {
                triples.push((noun, adj, d));
            }
            Ok(true)
        })?;
        triples.sort_unstable();
        triples.dedup();
        let mut index: HashMap<u64, usize> = HashMap::new();
        let mut ids: Vec<u64> = Vec::new();
        let mut slot = |id: u64| -> usize {
            *index.entry(id).or_insert_with(|| {
                ids.push(id);
                ids.len() - 1
            })
        };
        let mut pair_counts: HashMap<(usize, usize), u32> = HashMap::new();
        for &(n, a, _) in &triples {
            let (n, a) = (slot(n), slot(a));
            *pair_counts.entry((n, a)).or_default() += 1;
        }
        drop(triples);
        let m = ids.len();
        let mut totals = vec![0u32; m];
        let mut distinct = vec![0usize; m];
        for (&(n, a), &c) in &pair_counts {
            totals[n] += c;
            totals[a] += c;
            distinct[n] += 1;
            distinct[a] += 1;
        }
        // Un mot n'a de vecteur que s'il a assez de contextes distincts ; les deux sens
        // (adjectifs d'un nom, noms d'un adjectif) dans le même espace d'ids.
        let counts = pair_counts.iter().flat_map(|(&(n, a), &c)| {
            let mut v = Vec::with_capacity(2);
            if distinct[n] >= MIN_SYNTACTIC_CONTEXTS {
                v.push((n, a, c));
            }
            if distinct[a] >= MIN_SYNTACTIC_CONTEXTS {
                v.push((a, n, c));
            }
            v
        });
        let rows = ppmi_rows(m, counts, &totals, &totals, opts.features);
        let group: Vec<Option<(Lang, Pos)>> = ids.iter().map(|id| info[id]).collect();
        let near = similar(
            &rows,
            m,
            &group,
            MIN_SYNTACTIC_SHARED,
            opts.min_cosine,
            opts.per_lemma,
        );
        // Liens syntaxiques réciproques seulement : chacun des deux mots est parmi les plus
        // proches de l'autre.
        let mutual = |w: usize, o: u32| near[o as usize].iter().any(|&(x, ..)| x as usize == w);
        let kept: Vec<Vec<(u32, f32, u16)>> = near
            .iter()
            .enumerate()
            .map(|(w, list)| {
                list.iter()
                    .copied()
                    .filter(|&(o, ..)| mutual(w, o))
                    .collect()
            })
            .collect();
        for (w, list) in kept.into_iter().enumerate() {
            for (o, cos, shared) in list {
                stats.syntactic += 1;
                links.entry(ids[w]).or_default().push((
                    ids[o as usize],
                    cos,
                    shared,
                    LinkKind::Syntactic,
                ));
            }
        }
        drop(rows);

        // ---- Contextes thématiques : mots du même passage.
        // Échantillon régulier de passages : un sur `stride` (par id).
        let stride = (total_passages as usize / opts.max_passages.max(1)).max(1) as u64;
        let sampled = |p: u64| p.wrapping_mul(0x9E37_79B9_7F4A_7C15) % stride == 0;
        let mut df_all: HashMap<u64, u64> = HashMap::new();
        self.visit(&[schema::STATS], |k, v| {
            if let Some(id) = id_at(k, 1) {
                df_all.insert(id, schema::u64_value(Some(v.to_vec())));
            }
            Ok(true)
        })?;
        let sample_size = (total_passages / stride).max(1) as f64;
        let mut vocab: Vec<(u64, Lang, Pos, u64)> = Vec::new();
        self.visit(&[schema::NODE], |k, v| {
            let (Some(id), Ok(rec)) = (id_at(k, 1), decode::<LemmaRecord>(v)) else {
                return Ok(true);
            };
            let Some((lang, pos)) = lemma_info(&rec) else {
                return Ok(true);
            };
            let df = df_all.get(&id).copied().unwrap_or(0);
            let expected = df as f64 / stride as f64;
            if expected >= opts.min_df as f64 && expected <= opts.max_share * sample_size {
                vocab.push((id, lang, pos, df));
            }
            Ok(true)
        })?;
        drop(df_all);
        vocab.sort_by_key(|v| std::cmp::Reverse(v.3));
        vocab.truncate(opts.max_vocab);
        let v = vocab.len();
        let mut passage_terms: HashMap<u64, Vec<u32>> = HashMap::new();
        for (i, &(id, ..)) in vocab.iter().enumerate() {
            self.visit(&Edge::prefix(NodeId(id), EdgeKind::Occurs), |key, _| {
                if let Some(p) = Edge::target(key).map(|p| p.0)
                    && sampled(p)
                {
                    passage_terms.entry(p).or_default().push(i as u32);
                }
                Ok(true)
            })?;
        }
        let mut by_doc: HashMap<usize, Vec<u64>> = HashMap::new();
        let candidates: Vec<u64> = passage_terms.keys().copied().collect();
        for p in candidates {
            if repeated(p)? {
                stats.boilerplate += 1;
                continue;
            }
            if let Some(d) = doc_of(p) {
                by_doc.entry(d).or_default().push(p);
            }
        }
        stats.passages = by_doc.values().map(Vec::len).sum();
        stats.docs = by_doc.len();
        // Co-occurrences, une fois par document ; dans un passage, les mots les plus
        // spécifiques (df faible) d'abord.
        let mut pairs: Vec<u64> = Vec::new();
        let mut doc_pairs: Vec<u64> = Vec::new();
        let mut term_docs = vec![0u32; v];
        let mut doc_terms: Vec<u32> = Vec::new();
        for ps in by_doc.values() {
            doc_pairs.clear();
            doc_terms.clear();
            for p in ps {
                let mut terms = passage_terms[p].clone();
                terms.sort_by_key(|&i| vocab[i as usize].3);
                terms.truncate(opts.terms_per_passage);
                terms.sort_unstable();
                for (x, &a) in terms.iter().enumerate() {
                    doc_terms.push(a);
                    for &b in &terms[x + 1..] {
                        doc_pairs.push(u64::from(a) << 32 | u64::from(b));
                    }
                }
            }
            doc_pairs.sort_unstable();
            doc_pairs.dedup();
            pairs.extend_from_slice(&doc_pairs);
            doc_terms.sort_unstable();
            doc_terms.dedup();
            for &t in &doc_terms {
                term_docs[t as usize] += 1;
            }
        }
        drop(passage_terms);
        pairs.sort_unstable();
        let mut grouped: Vec<(usize, usize, u32)> = Vec::new();
        let mut i = 0;
        while i < pairs.len() {
            let key = pairs[i];
            let mut j = i;
            while j < pairs.len() && pairs[j] == key {
                j += 1;
            }
            let (a, b) = ((key >> 32) as usize, (key & 0xFFFF_FFFF) as usize);
            let n = (j - i) as u32;
            grouped.push((a, b, n));
            grouped.push((b, a, n));
            i = j;
        }
        drop(pairs);
        let rows = ppmi_rows(
            v,
            grouped.into_iter(),
            &term_docs,
            &term_docs,
            opts.features,
        );
        let group: Vec<(Lang, Pos)> = vocab.iter().map(|&(_, l, p, _)| (l, p)).collect();
        let near = similar(
            &rows,
            v,
            &group,
            opts.min_shared,
            opts.min_cosine,
            opts.per_lemma,
        );
        for (w, list) in near.into_iter().enumerate() {
            for (o, cos, shared) in list {
                stats.links += 1;
                links.entry(vocab[w].0).or_default().push((
                    vocab[o as usize].0,
                    cos,
                    shared,
                    LinkKind::Topical,
                ));
            }
        }
        stats.vocab = v;
        Ok(Learned {
            stats,
            links,
            passages: total_passages,
        })
    }

    /// Remplace les liens appris par `learned`.
    pub fn store_learned(&mut self, learned: Learned) -> Result<LearnStats> {
        let mut ops: Vec<Op> = Vec::new();
        self.visit(&[LEARNED], |k, _| {
            ops.push(Op::Delete(k.to_vec()));
            Ok(true)
        })?;
        for (id, list) in &learned.links {
            let mut v = Vec::with_capacity(list.len() * RECORD);
            for &(o, cos, shared, kind) in list {
                v.extend_from_slice(&o.to_be_bytes());
                v.extend_from_slice(&cos.to_le_bytes());
                v.extend_from_slice(&shared.to_le_bytes());
                v.push(kind as u8);
            }
            ops.push(Op::Put(schema::id_key(LEARNED, NodeId(*id)), v));
            if ops.len() >= 50_000 {
                self.apply(std::mem::take(&mut ops))?;
            }
        }
        ops.push(Op::Put(
            schema::meta_key(LEARNED_PASSAGES),
            learned.passages.to_be_bytes().to_vec(),
        ));
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        ops.push(Op::Put(
            schema::meta_key(LEARNED_AT),
            now.to_be_bytes().to_vec(),
        ));
        self.apply(ops)?;
        Ok(learned.stats)
    }

    /// Calcule et enregistre les liens appris.
    pub fn learn(&mut self, opts: LearnOptions) -> Result<LearnStats> {
        let learned = self.compute_learned(opts)?;
        self.store_learned(learned)
    }

    /// Compte les liens appris enregistrés.
    pub fn learned_summary(&self) -> Result<LearnedSummary> {
        let mut out = LearnedSummary {
            at_passages: schema::u64_value(self.get(&schema::meta_key(LEARNED_PASSAGES))?),
            at: schema::u64_value(self.get(&schema::meta_key(LEARNED_AT))?),
            ..LearnedSummary::default()
        };
        self.visit(&[LEARNED], |k, v| {
            if id_at(k, 1).is_none() {
                return Ok(true);
            }
            out.words += 1;
            for chunk in v.chunks_exact(RECORD) {
                match LinkKind::from_u8(chunk[14]) {
                    Some(LinkKind::Syntactic) => out.syntactic += 1,
                    Some(LinkKind::Topical) => out.topical += 1,
                    None => {}
                }
            }
            Ok(true)
        })?;
        Ok(out)
    }

    /// Faut-il réapprendre ? Oui si la base a grossi d'au moins `growth` (0,2 : 20 %) depuis
    /// le dernier calcul et compte au moins `min_passages` passages.
    pub fn learning_due(&self, min_passages: u64, growth: f64) -> Result<bool> {
        let now = schema::u64_value(self.get(&schema::meta_key(schema::PASSAGES))?);
        let then = schema::u64_value(self.get(&schema::meta_key(LEARNED_PASSAGES))?);
        Ok(now >= min_passages && (then == 0 || now as f64 >= then as f64 * (1.0 + growth)))
    }

    /// Échantillon de liens appris : (lemme, langue, liens), les lemmes aux liens les plus
    /// soutenus d'abord.
    pub fn learned_sample(&self, limit: usize) -> Result<Vec<(String, String, Vec<LearnedLink>)>> {
        let mut ids = Vec::new();
        self.visit(&[LEARNED], |k, _| {
            ids.extend(id_at(k, 1));
            Ok(true)
        })?;
        let mut out = Vec::new();
        for id in ids {
            let Some(rec) = self.get(&schema::id_key(schema::NODE, NodeId(id)))? else {
                continue;
            };
            let Ok(rec) = decode::<LemmaRecord>(&rec) else {
                continue;
            };
            let (Some(lang), Some(pos)) = (Lang::from_code(&rec.lang), Pos::from_u8(rec.pos))
            else {
                continue;
            };
            let links = self.learned(lang, TermKind::Word(pos), &rec.lemma)?;
            out.push((rec.lemma, rec.lang, links));
        }
        // Les liens syntaxiques (ceux qui servent seuls) les plus soutenus d'abord.
        let best = |l: &[LearnedLink]| {
            l.iter()
                .map(|x| (x.kind == LinkKind::Syntactic, x.shared))
                .max()
                .unwrap_or((false, 0))
        };
        out.sort_by_key(|(_, _, l)| std::cmp::Reverse(best(l)));
        out.truncate(limit);
        Ok(out)
    }

    /// Lemmes appris pour `lemma`.
    pub fn learned(&self, lang: Lang, kind: TermKind, lemma: &str) -> Result<Vec<LearnedLink>> {
        let Some(v) = self.get(&schema::lexeme_key(lang, kind, lemma))? else {
            return Ok(Vec::new());
        };
        let id = NodeId(schema::u64_value(Some(v)));
        let Some(raw) = self.get(&schema::id_key(LEARNED, id))? else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for chunk in raw.chunks_exact(RECORD) {
            let other = u64::from_be_bytes(chunk[..8].try_into().expect("8 octets"));
            let cosine = f32::from_le_bytes(chunk[8..12].try_into().expect("4 octets"));
            let shared = u16::from_le_bytes(chunk[12..14].try_into().expect("2 octets"));
            let Some(link_kind) = LinkKind::from_u8(chunk[14]) else {
                continue;
            };
            let Some(rec) = self.get(&schema::id_key(schema::NODE, NodeId(other)))? else {
                continue;
            };
            let Ok(rec) = decode::<LemmaRecord>(&rec) else {
                continue;
            };
            if let Some(pos) = Pos::from_u8(rec.pos) {
                out.push(LearnedLink {
                    lemma: rec.lemma,
                    pos,
                    cosine,
                    shared,
                    kind: link_kind,
                });
            }
        }
        Ok(out)
    }
}
