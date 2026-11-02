//! Recherche : « actu paris », « info paris », « rues calmes »...
//!
//! Score d'un passage =
//!   pertinence textuelle (BM25 sur les lemmes ; IDF du mot tapé, toutes lectures réunies ;
//!   pour un mot ambigu, chaque lecture pondérée par sa part d'usage dans l'index)
//!   × couverture (part des mots de la requête trouvés, au carré)
//!   × bonus de qualification (un adjectif de la requête qualifie un nom de la requête
//!     dans le passage : « paris dangereux » ≠ « paris … dangereux »)
//!   × adéquation à l'intention (nature de la source) et fraîcheur, si la requête en a une.

use std::collections::{HashMap, HashSet};

use nknown_core::analyze::Pos;
use nknown_core::lang::Lang;
use nknown_core::nature::{Nature, NatureProfile};
use nknown_core::time::Timestamp;

use crate::graph::{Edge, EdgeKind, NodeId};
use crate::schema::{self, DocRecord, PassageRecord};
use crate::storage::Storage;
use crate::{Engine, Result, decode};

/// Ce que l'utilisateur cherche, au-delà des mots : un type de contenu et une fraîcheur.
#[derive(Debug, Clone, PartialEq)]
pub struct Intent {
    pub name: &'static str,
    pub nature: NatureProfile,
    /// Demi-vie de la fraîcheur, en jours ; `None` = la date ne compte pas.
    pub half_life_days: Option<f64>,
}

/// Opérateurs reconnus en tête ou dans la requête.
pub fn intent_for(word: &str) -> Option<Intent> {
    let (name, nature, half_life_days) = match word.to_lowercase().as_str() {
        "actu" | "actus" | "news" => ("actu", Nature::Event, Some(3.0)),
        "info" | "def" | "définition" => ("info", Nature::Reference, None),
        "avis" | "opinion" => ("avis", Nature::Opinion, Some(30.0)),
        "tuto" | "comment" => ("tuto", Nature::Tutorial, None),
        "perso" | "notes" => ("perso", Nature::Personal, None),
        _ => return None,
    };
    let mut profile = NatureProfile::default();
    profile.set(nature, 1.0);
    Some(Intent {
        name,
        nature: profile,
        half_life_days,
    })
}

/// Un mot de la requête et ses lectures possibles (catégorie, lemme).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryTerm {
    pub text: String,
    pub readings: Vec<(Pos, String)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub lang: Lang,
    pub terms: Vec<QueryTerm>,
    pub intent: Option<Intent>,
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
}

const K1: f64 = 1.2;
const B: f64 = 0.75;
const QUALIFIED_BONUS: f64 = 1.5;

/// Un lemme candidat d'un mot de la requête.
struct Reading {
    id: NodeId,
    pos: Pos,
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
            for (pos, lemma) in &term.readings {
                let Some(v) = self.get(&schema::lexeme_key(q.lang, *pos, lemma))? else {
                    continue;
                };
                let id = NodeId(schema::u64_value(Some(v)));
                if seen.insert(id) {
                    let df =
                        schema::u64_value(self.get(&schema::id_key(schema::STATS, id))?) as f64;
                    found.push(Reading {
                        id,
                        pos: *pos,
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

        // Paires (adjectif, nom) de la requête liées dans un passage.
        let mut qualified: HashMap<NodeId, Vec<(usize, usize)>> = HashMap::new();
        for (a, adj_rs) in readings.iter().enumerate() {
            for (b, noun_rs) in readings.iter().enumerate() {
                if a == b {
                    continue;
                }
                for adj in adj_rs.iter().filter(|r| r.pos == Pos::Adj) {
                    for noun in noun_rs
                        .iter()
                        .filter(|r| matches!(r.pos, Pos::Noun | Pos::Propn))
                    {
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

        let mut docs: HashMap<u64, DocRecord> = HashMap::new();
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
            if let Some(intent) = &q.intent {
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
        hits.truncate(limit);
        Ok(hits)
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
