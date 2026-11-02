//! Recherche : « actu paris », « info paris », « rues calmes »...
//!
//! Score d'un passage =
//!   pertinence textuelle (BM25 sur les lemmes ; IDF du mot tapé, toutes lectures réunies ;
//!   pour un mot ambigu, chaque lecture pondérée par sa part d'usage dans l'index)
//!   × couverture (part des mots de la requête trouvés, au carré)
//!   × bonus de qualification (un adjectif de la requête qualifie un nom de la requête
//!     dans le passage : « paris dangereux » ≠ « paris … dangereux »)
//!   × titre (part des mots de la requête présents dans le titre du document)
//!   × adéquation à l'intention (nature de la source) et fraîcheur, si la requête en a une.

use std::collections::{HashMap, HashSet};

use nknown_core::analyze::Pos;
use nknown_core::lang::Lang;
use nknown_core::nature::{Nature, NatureProfile};
use nknown_core::time::Timestamp;

use crate::graph::{Edge, EdgeKind, NodeId};
use crate::schema::{self, DocRecord, PassageRecord, TermKind};
use crate::storage::Storage;
use crate::{Engine, Result, decode};

/// Ce que l'utilisateur cherche, au-delà des mots : un type de contenu et une fraîcheur.
#[derive(Debug, Clone, PartialEq)]
pub struct Intent {
    pub name: &'static str,
    pub nature: NatureProfile,
    /// Demi-vie de la fraîcheur, en jours ; `None` = la date ne compte pas.
    pub half_life_days: Option<f64>,
    /// `Some(true)` : préférer les passages de code ; `Some(false)` : la prose technique.
    pub prefer_code: Option<bool>,
}

/// Opérateurs reconnus dans la requête.
pub fn intent_for(word: &str) -> Option<Intent> {
    let (name, nature, half_life_days, prefer_code) = match word.to_lowercase().as_str() {
        "actu" | "actus" | "news" => ("actu", Nature::Event, Some(3.0), None),
        "info" | "def" | "définition" => ("info", Nature::Reference, None, None),
        "avis" | "opinion" => ("avis", Nature::Opinion, Some(30.0), None),
        "tuto" | "comment" => ("tuto", Nature::Tutorial, None, None),
        "perso" | "notes" => ("perso", Nature::Personal, None, None),
        "code" => ("code", Nature::Technical, None, Some(true)),
        "doc" | "docs" => ("doc", Nature::Technical, None, Some(false)),
        _ => return None,
    };
    let mut profile = NatureProfile::default();
    profile.set(nature, 1.0);
    Some(Intent {
        name,
        nature: profile,
        half_life_days,
        prefer_code,
    })
}

/// Un mot de la requête et ses lectures possibles (mot d'une catégorie, ou terme de code).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryTerm {
    pub text: String,
    pub readings: Vec<(TermKind, String)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub lang: Lang,
    pub terms: Vec<QueryTerm>,
    pub intent: Option<Intent>,
}

/// Document voisin d'un résultat (passage similaire dans un autre document).
#[derive(Debug, Clone, PartialEq)]
pub struct Related {
    pub doc_id: NodeId,
    pub title: Option<String>,
    pub source: String,
    pub url: Option<String>,
    pub date: i64,
    pub similarity: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub passage: NodeId,
    pub doc_id: NodeId,
    pub score: f64,
    pub text: String,
    pub doc: DocRecord,
    /// Mots de la requête trouvés dans le passage.
    pub matched: Vec<String>,
    /// Paires (adjectif, nom) de la requête liées dans le passage.
    pub qualified: Vec<(String, String)>,
    pub is_code: bool,
    pub code_lang: Option<String>,
    /// Documents voisins, du plus similaire au moins similaire ; un quasi-doublon d'un
    /// meilleur résultat n'est pas listé à part mais ajouté ici.
    pub related: Vec<Related>,
}

const K1: f64 = 1.2;
const B: f64 = 0.75;
const QUALIFIED_BONUS: f64 = 1.5;
/// Score multiplié jusqu'à (1 + TITLE_BOOST) quand tous les mots sont dans le titre.
const TITLE_BOOST: f64 = 0.6;
/// Au-delà, deux passages disent la même chose : le moins bien classé est regroupé.
const DUPLICATE_SIMILARITY: f32 = 0.5;
/// Emballement : voisins d'autres sources assez proches et assez récents.
const BURST_SIMILARITY: f32 = 0.35;
const BURST_WINDOW_SECS: i64 = 3 * 86_400;
const BURST_BONUS: f64 = 0.25;

/// Un lemme candidat d'un mot de la requête.
struct Reading {
    id: NodeId,
    kind: TermKind,
    idf: f64,
    /// Part de cette lecture parmi celles du mot, selon l'usage dans l'index : pour « paris »,
    /// la ville (fréquente) l'emporte sur « pari » (rare), que l'IDF seul favoriserait.
    share: f64,
}

impl<S: Storage> Engine<S> {
    pub fn search(&self, q: &Query, now: Timestamp, limit: usize) -> Result<Vec<Hit>> {
        let n = schema::u64_value(self.get(&schema::meta_key(schema::PASSAGES))?) as f64;
        if n == 0.0 || q.terms.is_empty() {
            return Ok(Vec::new());
        }
        let avg_len = (schema::u64_value(self.get(&schema::meta_key(schema::TOTAL_LEN))?) as f64
            / n)
            .max(1.0);

        // Lectures existantes dans l'index pour chaque mot.
        let mut readings: Vec<Vec<Reading>> = Vec::new();
        for term in &q.terms {
            let mut found = Vec::new();
            let mut seen = HashSet::new();
            for (kind, lemma) in &term.readings {
                let Some(v) = self.get(&schema::lexeme_key(q.lang, *kind, lemma))? else {
                    continue;
                };
                let id = NodeId(schema::u64_value(Some(v)));
                if seen.insert(id) {
                    let df =
                        schema::u64_value(self.get(&schema::id_key(schema::STATS, id))?) as f64;
                    found.push(Reading {
                        id,
                        kind: *kind,
                        idf: 0.0,
                        share: df,
                    });
                }
            }
            // La spécificité (IDF) est celle du mot tapé, toutes lectures réunies ;
            // la part lissée de chaque lecture départage ensuite les sens.
            let total: f64 = found.iter().map(|r| r.share).sum();
            let idf = (1.0 + (n - total.min(n) + 0.5) / (total.min(n) + 0.5)).ln();
            let k = found.len() as f64;
            for r in &mut found {
                r.share = (r.share + 1.0) / (total + k);
                r.idf = idf;
            }
            readings.push(found);
        }

        // Score BM25 par passage et par mot (meilleure lecture du mot).
        let mut per_passage: HashMap<NodeId, Vec<f64>> = HashMap::new();
        let mut lengths: HashMap<NodeId, f64> = HashMap::new();
        for (t, rs) in readings.iter().enumerate() {
            for r in rs {
                for (key, value) in self.scan(&Edge::prefix(r.id, EdgeKind::Occurs))? {
                    let Some(pid) = Edge::target(&key) else {
                        continue;
                    };
                    let tf = f64::from(schema::f32_value(&value));
                    let len = match lengths.get(&pid) {
                        Some(&l) => l,
                        None => {
                            let l = self.passage(pid)?.map_or(avg_len, |p| f64::from(p.len));
                            lengths.insert(pid, l);
                            l
                        }
                    };
                    let s = r.share * r.idf * tf * (K1 + 1.0)
                        / (tf + K1 * (1.0 - B + B * len / avg_len));
                    let scores = per_passage
                        .entry(pid)
                        .or_insert_with(|| vec![0.0; q.terms.len()]);
                    scores[t] = scores[t].max(s);
                }
            }
        }

        // Documents dont le titre contient un mot de la requête : candidats même si le mot
        // n'apparaît pas dans leur texte (fichier « mst.rs », article « Seine »). Leur premier
        // passage les représente, compté comme une occurrence.
        let mut title_docs: HashMap<u64, DocRecord> = HashMap::new();
        for (t, rs) in readings.iter().enumerate() {
            for r in rs {
                for (key, _) in self.scan(&Edge::prefix(r.id, EdgeKind::InTitle))? {
                    let Some(doc_id) = Edge::target(&key) else {
                        continue;
                    };
                    let doc = match title_docs.get(&doc_id.0) {
                        Some(d) => d.clone(),
                        None => {
                            let Some(d) = self.doc(doc_id)? else { continue };
                            title_docs.insert(doc_id.0, d.clone());
                            d
                        }
                    };
                    if doc.passages == 0 {
                        continue;
                    }
                    let pid = NodeId(doc.first_passage);
                    // Le titre atteste cette lecture du mot : pas de pondération par l'usage.
                    let s = r.idf;
                    let scores = per_passage
                        .entry(pid)
                        .or_insert_with(|| vec![0.0; q.terms.len()]);
                    scores[t] = scores[t].max(s);
                }
            }
        }

        // Paires (adjectif, nom) de la requête liées dans un passage.
        let mut qualified: HashMap<NodeId, Vec<(usize, usize)>> = HashMap::new();
        for (a, adj_rs) in readings.iter().enumerate() {
            for (b, noun_rs) in readings.iter().enumerate() {
                if a == b {
                    continue;
                }
                for adj in adj_rs.iter().filter(|r| r.kind == TermKind::Word(Pos::Adj)) {
                    for noun in noun_rs.iter().filter(|r| {
                        matches!(
                            r.kind,
                            TermKind::Word(Pos::Noun) | TermKind::Word(Pos::Propn)
                        )
                    }) {
                        for (key, _) in self.scan(&schema::qualified_prefix(noun.id, adj.id))? {
                            if let Some(pid) = key.get(17..25).and_then(|b| b.try_into().ok()) {
                                let pairs = qualified
                                    .entry(NodeId(u64::from_be_bytes(pid)))
                                    .or_default();
                                if !pairs.contains(&(a, b)) {
                                    pairs.push((a, b));
                                }
                            }
                        }
                    }
                }
            }
        }

        let mut docs: HashMap<u64, DocRecord> = title_docs;
        let mut hits = Vec::new();
        for (pid, scores) in per_passage {
            let matched: Vec<usize> = (0..scores.len()).filter(|&t| scores[t] > 0.0).collect();
            let coverage = matched.len() as f64 / q.terms.len() as f64;
            let pairs = qualified.get(&pid).cloned().unwrap_or_default();
            let mut score = scores.iter().sum::<f64>()
                * coverage.powi(2)
                * QUALIFIED_BONUS.powi(pairs.len() as i32);

            let Some(passage) = self.passage(pid)? else {
                continue;
            };
            let doc = match docs.get(&passage.doc) {
                Some(d) => d.clone(),
                None => {
                    let Some(d) = self.doc(NodeId(passage.doc))? else {
                        continue;
                    };
                    docs.insert(passage.doc, d.clone());
                    d
                }
            };
            // Mots de la requête présents dans le titre du document : le sujet du document,
            // pas une mention en passant.
            let in_title = readings
                .iter()
                .filter(|rs| rs.iter().any(|r| doc.title_lemmas.contains(&r.id.0)))
                .count();
            score *= 1.0 + TITLE_BOOST * in_title as f64 / q.terms.len() as f64;
            if let Some(intent) = &q.intent {
                if let Some(prefer_code) = intent.prefer_code {
                    score *= if passage.is_code == prefer_code {
                        1.5
                    } else {
                        0.6
                    };
                }
                let fit = NatureProfile::from_scores(doc.nature).weigh(&intent.nature);
                score *= 0.25 + f64::from(fit);
                if let Some(half_life) = intent.half_life_days {
                    let age_days = ((now.0 - doc.date) as f64 / 86_400.0).max(0.0);
                    let mut fresh = 0.5f64.powf(age_days / half_life);
                    if doc.date_confidence == 2 {
                        fresh *= 0.8; // date de collecte seulement : fraîcheur incertaine
                    }
                    score *= 0.2 + 0.8 * fresh;
                }
            }
            hits.push(Hit {
                passage: pid,
                doc_id: NodeId(passage.doc),
                score,
                is_code: passage.is_code,
                code_lang: passage.code_lang.clone(),
                related: Vec::new(),
                text: passage.text,
                doc,
                matched: matched.iter().map(|&t| q.terms[t].text.clone()).collect(),
                qualified: pairs
                    .iter()
                    .map(|&(a, b)| (q.terms[a].text.clone(), q.terms[b].text.clone()))
                    .collect(),
            });
        }
        hits.sort_by(|a, b| b.score.total_cmp(&a.score));
        // Un résultat par document : son meilleur passage.
        let mut seen = HashSet::new();
        hits.retain(|h| seen.insert(h.doc_id));

        // Voisins des meilleurs résultats ; pour « actu », un sujet repris par plusieurs
        // sources récentes remonte (effet d'emballement).
        hits.truncate(limit * 4 + 20);
        for h in &mut hits {
            h.related = self.related(h.passage, h.doc_id, &mut docs)?;
            if q.intent
                .as_ref()
                .is_some_and(|i| i.half_life_days.is_some())
            {
                let echoes = h
                    .related
                    .iter()
                    .filter(|r| {
                        r.source != h.doc.source
                            && r.similarity >= BURST_SIMILARITY
                            && now.0 - r.date <= BURST_WINDOW_SECS
                    })
                    .count()
                    .min(5);
                h.score *= 1.0 + BURST_BONUS * echoes as f64;
            }
        }
        hits.sort_by(|a, b| b.score.total_cmp(&a.score));

        // Quasi-doublons (même information reprise ailleurs) : regroupés sous le meilleur.
        let mut kept: Vec<Hit> = Vec::new();
        for h in hits {
            let twin = kept.iter().position(|k| {
                k.related
                    .iter()
                    .any(|r| r.doc_id == h.doc_id && r.similarity >= DUPLICATE_SIMILARITY)
                    || h.related
                        .iter()
                        .any(|r| r.doc_id == k.doc_id && r.similarity >= DUPLICATE_SIMILARITY)
            });
            match twin {
                Some(i) => {
                    let k = &mut kept[i];
                    if !k.related.iter().any(|r| r.doc_id == h.doc_id) {
                        k.related.push(Related {
                            doc_id: h.doc_id,
                            title: h.doc.title.clone(),
                            source: h.doc.source.clone(),
                            url: h.doc.url.clone(),
                            date: h.doc.date,
                            similarity: DUPLICATE_SIMILARITY,
                        });
                    }
                }
                None => kept.push(h),
            }
        }
        kept.truncate(limit);
        Ok(kept)
    }

    /// Documents reliés au passage par des arêtes de similarité, du plus proche au plus loin.
    fn related(
        &self,
        passage: NodeId,
        own_doc: NodeId,
        docs: &mut HashMap<u64, DocRecord>,
    ) -> Result<Vec<Related>> {
        let mut out: Vec<Related> = Vec::new();
        for (key, value) in self.scan(&Edge::prefix(passage, EdgeKind::Similar))? {
            let Some(other) = Edge::target(&key) else {
                continue;
            };
            let Some(p) = self.passage(other)? else {
                continue;
            };
            if p.doc == own_doc.0 {
                continue;
            }
            let doc = match docs.get(&p.doc) {
                Some(d) => d.clone(),
                None => {
                    let Some(d) = self.doc(NodeId(p.doc))? else {
                        continue;
                    };
                    docs.insert(p.doc, d.clone());
                    d
                }
            };
            let similarity = schema::f32_value(&value);
            match out.iter_mut().find(|r| r.doc_id.0 == p.doc) {
                Some(r) => r.similarity = r.similarity.max(similarity),
                None => out.push(Related {
                    doc_id: NodeId(p.doc),
                    title: doc.title,
                    source: doc.source,
                    url: doc.url,
                    date: doc.date,
                    similarity,
                }),
            }
        }
        out.sort_by(|a, b| b.similarity.total_cmp(&a.similarity));
        Ok(out)
    }

    pub(crate) fn passage(&self, id: NodeId) -> Result<Option<PassageRecord>> {
        self.get(&schema::id_key(schema::PASSAGE, id))?
            .map(|v| decode(&v))
            .transpose()
    }

    pub fn doc(&self, id: NodeId) -> Result<Option<DocRecord>> {
        self.get(&schema::id_key(schema::DOC, id))?
            .map(|v| decode(&v))
            .transpose()
    }
}
