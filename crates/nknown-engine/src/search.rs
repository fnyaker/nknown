//! Recherche : « actu paris », « info paris », « rues calmes »...
//!
//! Score d'un passage =
//!   pertinence textuelle (BM25 sur les lemmes ; IDF du mot tapé, toutes lectures réunies ;
//!   pour un mot ambigu, chaque lecture pondérée par sa part d'usage dans l'index)
//!   × couverture (part des mots de la requête trouvés, au carré)
//!   × bonus de qualification (un adjectif de la requête qualifie un nom de la requête
//!     dans le passage : « paris dangereux » ≠ « paris … dangereux »)
//!   × titre (part des mots de la requête présents dans le titre du document)
//!   × adéquation à l'intention (nature de la source) et fraîcheur, si la requête en a une
//!   × emballement, soutien des résultats voisins (graphe), autorité du document.
//!
//! Plusieurs seuils sont dynamiques (exigence de couverture, demi-vie, coupure, fusion des
//! doublons) : ils dépendent des candidats de la requête. [`SearchResult::thresholds`] les
//! donne, [`Hit::parts`] détaille chaque score.

use std::collections::{HashMap, HashSet};

use nknown_core::analyze::Pos;
use nknown_core::lang::Lang;
use nknown_core::nature::{Nature, NatureProfile};
use nknown_core::source::SourceKind;
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
    /// (langue, nature du terme, lemme) ; les termes de code sont sous [`Lang::NONE`].
    pub readings: Vec<(Lang, TermKind, String)>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Query {
    pub terms: Vec<QueryTerm>,
    pub intent: Option<Intent>,
    /// `@new` : un peu de priorité aux contenus récents (demi-vie de base, en jours), sans
    /// changer le type de contenu cherché.
    pub recency: Option<f64>,
    /// Familles de sources retenues (codes de `DocRecord::kind`) ; vide : toutes.
    pub kinds: Vec<u8>,
}

/// Modificateurs `@…` de la requête (sans l'arobase).
#[derive(Debug, Clone, PartialEq)]
pub enum Modifier {
    Intent(Intent),
    /// Priorité aux contenus récents.
    Recent,
    /// Seulement ces familles de sources (codes de `DocRecord::kind`).
    Kinds(Vec<u8>),
}

/// `code`, `new`, `wiki`… → le modificateur ; `None` si le mot n'en est pas un.
pub fn modifier_for(word: &str) -> Option<Modifier> {
    let w = word.to_lowercase();
    Some(match w.as_str() {
        "new" | "récent" | "recent" | "neuf" | "frais" => Modifier::Recent,
        "rss" | "flux" | "feed" => Modifier::Kinds(vec![0]),
        "wiki" | "wikipedia" | "wikipédia" => Modifier::Kinds(vec![1]),
        "web" | "site" | "sites" => Modifier::Kinds(vec![2]),
        "fichier" | "fichiers" | "file" | "files" => Modifier::Kinds(vec![3]),
        _ => Modifier::Intent(intent_for(&w)?),
    })
}

/// Poids d'une variante (même racine) face à une lecture directe du mot.
const VARIANT_SHARE: f64 = 0.8;

/// Demi-vie de base de `@new`, en jours (allongée selon l'âge des candidats).
pub const RECENT_HALF_LIFE_DAYS: f64 = 14.0;

/// Document voisin d'un résultat (passage similaire dans un autre document).
#[derive(Debug, Clone, PartialEq)]
pub struct Related {
    pub doc_id: NodeId,
    pub title: Option<String>,
    pub source: String,
    pub url: Option<String>,
    /// Identifiant dans la source (chemin d'un fichier, guid d'un article).
    pub external_id: Option<String>,
    pub date: i64,
    pub similarity: f32,
}

/// Facteurs du score d'un résultat : le score est leur produit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScoreParts {
    /// Pertinence textuelle (BM25 des mots trouvés).
    pub text: f64,
    /// Part des mots trouvés, à l'exposant dynamique.
    pub coverage: f64,
    /// Adjectifs de la requête qualifiant des noms de la requête dans le passage.
    pub qualified: f64,
    /// Mots de la requête dans le titre.
    pub title: f64,
    /// Adéquation de la nature du document (et du type de passage) à l'intention.
    pub nature: f64,
    pub freshness: f64,
    /// Reprise du sujet par d'autres sources récentes.
    pub burst: f64,
    /// Soutien des résultats voisins (propagation sur les liens de similarité).
    pub graph: f64,
    /// Centralité du document dans le graphe (PageRank, `Engine::compute_authority`).
    pub authority: f64,
}

impl Default for ScoreParts {
    fn default() -> Self {
        Self {
            text: 0.0,
            coverage: 1.0,
            qualified: 1.0,
            title: 1.0,
            nature: 1.0,
            freshness: 1.0,
            burst: 1.0,
            graph: 1.0,
            authority: 1.0,
        }
    }
}

impl ScoreParts {
    pub fn total(&self) -> f64 {
        self.text
            * self.coverage
            * self.qualified
            * self.title
            * self.nature
            * self.freshness
            * self.burst
            * self.graph
            * self.authority
    }
}

/// Seuils effectivement appliqués à une recherche (certains dépendent des candidats).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Thresholds {
    pub coverage_exponent: i32,
    pub half_life_days: Option<f64>,
    pub burst_window_days: Option<f64>,
    /// Part du meilleur score en dessous de laquelle un résultat est écarté.
    pub cutoff: f64,
    /// Similarité au-delà de laquelle deux résultats sont regroupés.
    pub duplicate: f32,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SearchResult {
    pub hits: Vec<Hit>,
    /// Passages candidats avant classement.
    pub candidates: usize,
    pub thresholds: Thresholds,
}

/// Étape d'une recherche en cours.
#[derive(Debug)]
pub enum Stage<'a> {
    /// Classement provisoire, au score du texte, un passage par document : ni propagation
    /// dans le graphe, ni autorité, ni seuils, ni regroupement des doublons.
    Candidates { hits: &'a [Hit], candidates: usize },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub passage: NodeId,
    pub doc_id: NodeId,
    pub score: f64,
    pub parts: ScoreParts,
    pub text: String,
    pub doc: DocRecord,
    /// Mots de la requête trouvés dans le passage.
    pub matched: Vec<String>,
    /// Contribution textuelle (BM25 pondéré) de chaque mot de la requête, dans l'ordre de
    /// `Query::terms` ; 0 pour un mot absent du passage.
    pub term_scores: Vec<f64>,
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
/// Au-delà, deux passages de sources différentes disent la même chose (une reprise) : le
/// moins bien classé est regroupé.
const DUPLICATE_SIMILARITY: f32 = 0.5;
/// Ailleurs, des documents proches restent distincts (deux fichiers d'un même projet, deux
/// articles d'un même flux, deux articles d'encyclopédie) : seule une copie est regroupée.
const COPY_SIMILARITY: f32 = 0.95;

/// Similarité à partir de laquelle deux résultats sont regroupés. La reprise d'une même
/// information n'existe qu'entre sources différentes de flux ou du web.
fn fold_similarity(a: &DocRecord, b: &DocRecord, duplicate: f32) -> f32 {
    let republishes = |d: &DocRecord| {
        d.kind == crate::source_kind_code(SourceKind::Feed)
            || d.kind == crate::source_kind_code(SourceKind::Web)
    };
    if a.source != b.source && republishes(a) && republishes(b) {
        duplicate
    } else {
        COPY_SIMILARITY
    }
}
/// Emballement : voisins d'autres sources assez proches et assez récents.
const BURST_SIMILARITY: f32 = 0.35;
/// Fenêtre minimale de l'emballement (élargie avec la demi-vie dynamique).
const BURST_WINDOW_DAYS: f64 = 3.0;
const BURST_BONUS: f64 = 0.25;
/// Propagation : bonus maximal reçu des résultats voisins.
const GRAPH_WEIGHT: f64 = 0.3;
/// Autorité : poids du logarithme de la centralité (moyenne 1).
const AUTHORITY_WEIGHT: f64 = 0.15;

/// Un lemme candidat d'un mot de la requête.
struct Reading {
    id: NodeId,
    kind: TermKind,
    idf: f64,
    /// Part de cette lecture parmi celles du mot, selon l'usage dans l'index : pour « paris »,
    /// la ville (fréquente) l'emporte sur « pari » (rare), que l'IDF seul favoriserait.
    share: f64,
}

fn age_days(now: Timestamp, date: i64) -> f64 {
    ((now.0 - date) as f64 / 86_400.0).max(0.0)
}

/// Un passage candidat et ses facteurs propres, avant les seuils dynamiques.
struct Candidate {
    pid: NodeId,
    text: f64,
    matched: Vec<usize>,
    term_scores: Vec<f64>,
    pairs: Vec<(usize, usize)>,
    in_title: usize,
    passage: PassageRecord,
    doc: DocRecord,
}

impl<S: Storage> Engine<S> {
    pub fn search(&self, q: &Query, now: Timestamp, limit: usize) -> Result<Vec<Hit>> {
        Ok(self.search_explained(q, now, limit)?.hits)
    }

    /// Recherche avec le détail : seuils effectivement appliqués, nombre de candidats.
    pub fn search_explained(
        &self,
        q: &Query,
        now: Timestamp,
        limit: usize,
    ) -> Result<SearchResult> {
        self.search_observed(q, now, limit, &mut |_| {})
    }

    /// Comme [`Self::search_explained`], en montrant les étapes à `observe` au fil du calcul
    /// (pour qu'une interface construise la carte en direct).
    pub fn search_observed(
        &self,
        q: &Query,
        now: Timestamp,
        limit: usize,
        observe: &mut dyn FnMut(Stage<'_>),
    ) -> Result<SearchResult> {
        let n = schema::u64_value(self.get(&schema::meta_key(schema::PASSAGES))?) as f64;
        if n == 0.0 || q.terms.is_empty() {
            return Ok(SearchResult::default());
        }
        let avg_len = (schema::u64_value(self.get(&schema::meta_key(schema::TOTAL_LEN))?) as f64
            / n)
            .max(1.0);

        // Lectures existantes dans l'index pour chaque mot.
        let mut readings: Vec<Vec<Reading>> = Vec::new();
        for term in &q.terms {
            let mut found = Vec::new();
            let mut seen = HashSet::new();
            for (lang, kind, lemma) in &term.readings {
                let Some(v) = self.get(&schema::lexeme_key(*lang, *kind, lemma))? else {
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
            // Variantes : lemmes de même racine (« midterms » pour « midterm », un nom propre
            // pour un nom commun), un peu moins sûres que les lectures directes.
            let stems: HashSet<(Lang, String)> = term
                .readings
                .iter()
                .filter(|(_, kind, _)| *kind != TermKind::Code)
                .map(|(lang, _, lemma)| (*lang, self.stem_of(*lang, lemma)))
                .collect();
            for (lang, stem) in stems {
                for (key, value) in self.scan(&schema::stem_prefix(lang, &stem))? {
                    let Some(id) = key
                        .get(key.len().saturating_sub(8)..)
                        .and_then(|b| b.try_into().ok())
                        .map(|b| NodeId(u64::from_be_bytes(b)))
                    else {
                        continue;
                    };
                    let Some(pos) = value.first().and_then(|&b| Pos::from_u8(b)) else {
                        continue;
                    };
                    if seen.insert(id) {
                        let df =
                            schema::u64_value(self.get(&schema::id_key(schema::STATS, id))?) as f64;
                        found.push(Reading {
                            id,
                            kind: TermKind::Word(pos),
                            idf: 0.0,
                            share: df * VARIANT_SHARE,
                        });
                    }
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

        // ---- Facteurs propres à chaque passage.
        let mut docs: HashMap<u64, DocRecord> = title_docs;
        let mut cands: Vec<Candidate> = Vec::new();
        for (pid, scores) in per_passage {
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
            if !q.kinds.is_empty() && !q.kinds.contains(&doc.kind) {
                continue;
            }
            // Mots de la requête présents dans le titre : le sujet du document.
            let in_title = readings
                .iter()
                .filter(|rs| rs.iter().any(|r| doc.title_lemmas.contains(&r.id.0)))
                .count();
            cands.push(Candidate {
                pid,
                text: scores.iter().sum(),
                matched: (0..scores.len()).filter(|&t| scores[t] > 0.0).collect(),
                term_scores: scores.clone(),
                pairs: qualified.get(&pid).cloned().unwrap_or_default(),
                in_title,
                passage,
                doc,
            });
        }
        let candidates = cands.len();
        if candidates == 0 {
            return Ok(SearchResult::default());
        }

        // ---- Seuils dynamiques, déduits des candidats.
        let terms = q.terms.len();
        let full = cands.iter().filter(|c| c.matched.len() == terms).count();
        // Couverture : stricte s'il y a de quoi remplir la page avec des passages qui
        // contiennent tous les mots, indulgente sinon.
        let coverage_exp = match () {
            _ if terms <= 1 => 1,
            _ if full >= limit => 3,
            _ if full > 0 => 2,
            _ => 1,
        };
        // Fraîcheur : demi-vie au moins celle de l'intention, allongée jusqu'à l'âge médian
        // des candidats les plus pertinents (corpus lent : on ne pénalise pas tout pareil).
        let half_life = q
            .intent
            .as_ref()
            .and_then(|i| i.half_life_days)
            .or(q.recency)
            .map(|base| {
                let mut best: Vec<&Candidate> = cands.iter().collect();
                best.sort_by(|a, b| b.text.total_cmp(&a.text));
                let mut ages: Vec<f64> = best
                    .iter()
                    .take(20)
                    .map(|c| age_days(now, c.doc.date))
                    .collect();
                ages.sort_by(f64::total_cmp);
                ages[ages.len() / 2].clamp(base, base * 30.0)
            });

        // ---- Score de chaque passage.
        let mut hits = Vec::new();
        for c in cands {
            let mut parts = ScoreParts {
                text: c.text,
                coverage: (c.matched.len() as f64 / terms as f64).powi(coverage_exp),
                qualified: QUALIFIED_BONUS.powi(c.pairs.len() as i32),
                title: 1.0 + TITLE_BOOST * c.in_title as f64 / terms as f64,
                ..ScoreParts::default()
            };
            if let Some(intent) = &q.intent {
                let fit = NatureProfile::from_scores(c.doc.nature).weigh(&intent.nature);
                parts.nature = 0.25 + f64::from(fit);
                if let Some(prefer_code) = intent.prefer_code {
                    parts.nature *= if c.passage.is_code == prefer_code {
                        1.5
                    } else {
                        0.6
                    };
                }
            }
            if let Some(hl) = half_life {
                let mut fresh = 0.5f64.powf(age_days(now, c.doc.date) / hl);
                if c.doc.date_confidence == 2 {
                    fresh *= 0.8; // date de collecte seulement : fraîcheur incertaine
                }
                // L'intention qui porte une date (actu) tranche ; `@new` ne fait que pencher.
                let floor = if q
                    .intent
                    .as_ref()
                    .is_some_and(|i| i.half_life_days.is_some())
                {
                    0.2
                } else {
                    0.5
                };
                parts.freshness = floor + (1.0 - floor) * fresh;
            }
            hits.push(Hit {
                passage: c.pid,
                doc_id: NodeId(c.passage.doc),
                score: parts.total(),
                parts,
                is_code: c.passage.is_code,
                code_lang: c.passage.code_lang.clone(),
                related: Vec::new(),
                text: c.passage.text,
                doc: c.doc,
                matched: c.matched.iter().map(|&t| q.terms[t].text.clone()).collect(),
                term_scores: c.term_scores,
                qualified: c
                    .pairs
                    .iter()
                    .map(|&(a, b)| (q.terms[a].text.clone(), q.terms[b].text.clone()))
                    .collect(),
            });
        }
        hits.sort_by(|a, b| b.score.total_cmp(&a.score));
        // Un résultat par document : son meilleur passage.
        let mut seen = HashSet::new();
        hits.retain(|h| seen.insert(h.doc_id));
        hits.truncate(limit * 4 + 20);
        observe(Stage::Candidates {
            hits: &hits[..limit.min(hits.len())],
            candidates,
        });

        // ---- Graphe : voisins, emballement, propagation, autorité.
        let burst_window = half_life.map_or(BURST_WINDOW_DAYS, |h| h.max(BURST_WINDOW_DAYS));
        let base: HashMap<NodeId, f64> = hits.iter().map(|h| (h.doc_id, h.score)).collect();
        let top_base = hits.first().map_or(1.0, |h| h.score).max(f64::EPSILON);
        for h in &mut hits {
            h.related = self.related(h.passage, h.doc_id, &mut docs)?;
            if half_life.is_some() {
                let echoes = h
                    .related
                    .iter()
                    .filter(|r| {
                        r.source != h.doc.source
                            && r.similarity >= BURST_SIMILARITY
                            && age_days(now, r.date) <= burst_window
                    })
                    .count()
                    .min(5);
                h.parts.burst = 1.0 + BURST_BONUS * echoes as f64;
            }
            // Propagation : un passage relié à d'autres résultats pertinents en reçoit une part.
            let support: f64 = h
                .related
                .iter()
                .filter_map(|r| base.get(&r.doc_id).map(|s| f64::from(r.similarity) * s))
                .sum();
            h.parts.graph = 1.0 + GRAPH_WEIGHT * (support / top_base).min(1.0);
            h.parts.authority =
                (1.0 + AUTHORITY_WEIGHT * self.authority(h.doc_id)?.ln()).clamp(0.85, 1.3);
            h.score = h.parts.total();
        }
        hits.sort_by(|a, b| b.score.total_cmp(&a.score));

        // ---- Coupure relative : plus il y a de candidats, plus on écarte la traîne faible.
        let cutoff = (0.05 + 0.05 * (candidates as f64).log10()).clamp(0.05, 0.3);
        let top = hits.first().map_or(0.0, |h| h.score);
        hits.retain(|h| h.score >= cutoff * top);

        // ---- Quasi-doublons : seuil abaissé quand les résultats sont redondants entre eux
        // (beaucoup de reprises), pour diversifier la page.
        let in_results: HashSet<NodeId> = hits.iter().map(|h| h.doc_id).collect();
        let redundant = hits
            .iter()
            .filter(|h| {
                h.related
                    .iter()
                    .any(|r| r.similarity >= 0.4 && in_results.contains(&r.doc_id))
            })
            .count() as f32
            / hits.len().max(1) as f32;
        let duplicate = (DUPLICATE_SIMILARITY - 0.1 * redundant).max(0.4);
        let mut kept: Vec<Hit> = Vec::new();
        for h in hits {
            let twin = kept.iter().position(|k| {
                let min = fold_similarity(&k.doc, &h.doc, duplicate);
                k.related
                    .iter()
                    .any(|r| r.doc_id == h.doc_id && r.similarity >= min)
                    || h.related
                        .iter()
                        .any(|r| r.doc_id == k.doc_id && r.similarity >= min)
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
                            external_id: h.doc.external_id.clone(),
                            date: h.doc.date,
                            similarity: fold_similarity(&k.doc, &h.doc, duplicate),
                        });
                    }
                }
                None => kept.push(h),
            }
        }
        kept.truncate(limit);
        Ok(SearchResult {
            hits: kept,
            candidates,
            thresholds: Thresholds {
                coverage_exponent: coverage_exp,
                half_life_days: half_life,
                burst_window_days: half_life.map(|_| burst_window),
                cutoff,
                duplicate,
            },
        })
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
                    external_id: doc.external_id,
                    date: doc.date,
                    similarity,
                }),
            }
        }
        out.sort_by(|a, b| b.similarity.total_cmp(&a.similarity));
        Ok(out)
    }

    pub fn passage(&self, id: NodeId) -> Result<Option<PassageRecord>> {
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
