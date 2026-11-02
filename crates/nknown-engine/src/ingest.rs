//! Ingestion d'un document analysé : passages, lemmes, arêtes, statistiques.
//!
//! Tout le document est écrit en une seule transaction.

use std::collections::HashMap;
use std::ops::Range;

use nknown_core::analyze::{Analysis, Pos, Relation, Token};
use nknown_core::item::RawItem;
use nknown_core::lang::Lang;
use nknown_core::nature::{Nature, NatureProfile};
use nknown_core::source::SourceKind;
use nknown_core::time::DateConfidence;

use crate::graph::{Edge, EdgeKind, NodeId};
use crate::schema::{self, DocRecord, LemmaRecord, PassageRecord};
use crate::storage::{Op, Storage};
use crate::{Engine, Result, encode};

/// Un document prêt à indexer.
#[derive(Debug, Clone, Copy)]
pub struct Document<'a> {
    pub item: &'a RawItem,
    pub kind: SourceKind,
    pub lang: Lang,
    pub analysis: &'a Analysis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ingested {
    Added {
        doc: NodeId,
        passages: usize,
    },
    /// Contenu déjà indexé (même source, même texte).
    Duplicate(NodeId),
}

/// Catégories indexées : les mots porteurs de sens. `X` = mot inconnu (sans couche 2/3).
pub fn is_content(pos: Pos) -> bool {
    matches!(
        pos,
        Pos::Noun | Pos::Propn | Pos::Verb | Pos::Adj | Pos::Num | Pos::X
    )
}

/// Nature a priori d'un contenu selon sa source.
pub fn nature_prior(kind: SourceKind) -> NatureProfile {
    let mut p = NatureProfile::default();
    match kind {
        SourceKind::Feed => {
            p.set(Nature::Event, 0.8);
            p.set(Nature::Opinion, 0.2);
        }
        SourceKind::Encyclopedia => p.set(Nature::Reference, 0.9),
        SourceKind::LocalFile => p.set(Nature::Personal, 0.8),
        SourceKind::Web => {
            p.set(Nature::Reference, 0.4);
            p.set(Nature::Event, 0.3);
            p.set(Nature::Opinion, 0.2);
        }
        SourceKind::Other => {}
    }
    p
}

/// Passage maximal avant découpe aux fins de phrase.
const MAX_PASSAGE_TOKENS: usize = 120;

/// Passages = paragraphes (séparés par une ligne vide), redécoupés aux fins de phrase
/// s'ils sont trop longs. Renvoie des plages d'indices de tokens.
pub fn passages(text: &str, tokens: &[Token]) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0;
    for i in 0..tokens.len() {
        let last = i + 1 == tokens.len();
        let paragraph_break = !last && {
            let gap = &text[tokens[i].span.end..tokens[i + 1].span.start];
            gap.matches('\n').count() >= 2
        };
        let too_long = i + 1 - start >= MAX_PASSAGE_TOKENS
            && tokens[i].pos == Pos::Punct
            && text[tokens[i].span.clone()].starts_with(['.', '!', '?', '…']);
        if last || paragraph_break || too_long {
            out.push(start..i + 1);
            start = i + 1;
        }
    }
    out
}

fn fnv(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

impl<S: Storage> Engine<S> {
    pub fn ingest(&mut self, d: Document<'_>) -> Result<Ingested> {
        let item = d.item;
        let hash = fnv(format!("{}\0{}", item.source.0, item.text).as_bytes());
        if let Some(id) = self.get(&schema::hash_key(hash))? {
            return Ok(Ingested::Duplicate(NodeId(schema::u64_value(Some(id)))));
        }

        let mut tx = Tx::new(self)?;
        let doc = tx.next();
        let tokens = &d.analysis.tokens;
        let ranges = passages(&item.text, tokens);
        // Passages contigus : le document ne retient que le premier et leur nombre.
        let pids: Vec<NodeId> = ranges.iter().map(|_| tx.next()).collect();
        let first_passage = pids.first().map_or(0, |p| p.0);

        for (ordinal, (range, &pid)) in ranges.iter().zip(&pids).enumerate() {
            let mut tf: HashMap<NodeId, u32> = HashMap::new();
            let mut lemma_of: HashMap<usize, NodeId> = HashMap::new();
            for i in range.clone() {
                let t = &tokens[i];
                if !is_content(t.pos) {
                    continue;
                }
                let id = tx.lemma(d.lang, t.pos, &t.lemma)?;
                *tf.entry(id).or_default() += 1;
                lemma_of.insert(i, id);
            }
            let len: u32 = tf.values().sum();
            for (&lemma, &count) in &tf {
                let w = (count as f32).to_le_bytes().to_vec();
                tx.put(
                    Edge::key_of(lemma, EdgeKind::Occurs, pid).to_vec(),
                    w.clone(),
                );
                tx.put(Edge::key_of(pid, EdgeKind::Contains, lemma).to_vec(), w);
                tx.add(schema::id_key(schema::STATS, lemma), 1)?;
            }
            for r in &d.analysis.relations {
                if let Relation::Qualifies { adj, noun } = *r
                    && let (Some(&a), Some(&n)) = (lemma_of.get(&adj), lemma_of.get(&noun))
                {
                    tx.put(schema::qualified_key(n, a, pid), Vec::new());
                    tx.add_f32(Edge::key_of(a, EdgeKind::Qualifies, n).to_vec(), 1.0)?;
                }
            }
            let (start, end) = (
                tokens[range.start].span.start,
                tokens[range.end - 1].span.end,
            );
            tx.put(
                schema::id_key(schema::PASSAGE, pid),
                encode(&PassageRecord {
                    doc: doc.0,
                    ordinal: ordinal as u32,
                    text: item.text[start..end].to_owned(),
                    len,
                })?,
            );
            tx.add(schema::meta_key(schema::TOTAL_LEN), u64::from(len))?;
        }

        let date = item.date();
        tx.put(
            schema::id_key(schema::DOC, doc),
            encode(&DocRecord {
                source: item.source.0.clone(),
                kind: crate::source_kind_code(d.kind),
                url: item.url.clone(),
                title: item.title.clone(),
                date: date.at.0,
                date_confidence: match date.confidence {
                    DateConfidence::Declared => 0,
                    DateConfidence::Inferred => 1,
                    DateConfidence::FetchTimeOnly => 2,
                },
                fetched_at: item.fetched_at.0,
                lang: d.lang.as_str().to_owned(),
                nature: nature_prior(d.kind).scores(),
                first_passage,
                passages: ranges.len() as u32,
            })?,
        );
        tx.put(schema::hash_key(hash), doc.0.to_be_bytes().to_vec());
        tx.add(schema::meta_key(schema::PASSAGES), ranges.len() as u64)?;
        tx.add(schema::meta_key(schema::DOCS), 1)?;
        tx.commit()?;
        Ok(Ingested::Added {
            doc,
            passages: ranges.len(),
        })
    }
}

/// Écritures d'une ingestion, avec les compteurs lus une fois puis cumulés en mémoire.
struct Tx<'e, S: Storage> {
    engine: &'e mut Engine<S>,
    ops: Vec<Op>,
    counter: u64,
    counters: HashMap<Vec<u8>, u64>,
    weights: HashMap<Vec<u8>, f32>,
    lemmas: HashMap<(Pos, String), NodeId>,
}

impl<'e, S: Storage> Tx<'e, S> {
    fn new(engine: &'e mut Engine<S>) -> Result<Self> {
        let counter = schema::u64_value(engine.get(&schema::meta_key(schema::NEXT_ID))?).max(1);
        Ok(Self {
            engine,
            ops: Vec::new(),
            counter,
            counters: HashMap::new(),
            weights: HashMap::new(),
            lemmas: HashMap::new(),
        })
    }

    fn next(&mut self) -> NodeId {
        let id = NodeId(self.counter);
        self.counter += 1;
        id
    }

    fn put(&mut self, k: Vec<u8>, v: Vec<u8>) {
        self.ops.push(Op::Put(k, v));
    }

    fn add(&mut self, key: Vec<u8>, delta: u64) -> Result<()> {
        if !self.counters.contains_key(&key) {
            let current = schema::u64_value(self.engine.get(&key)?);
            self.counters.insert(key.clone(), current);
        }
        *self.counters.get_mut(&key).expect("inséré ci-dessus") += delta;
        Ok(())
    }

    fn add_f32(&mut self, key: Vec<u8>, delta: f32) -> Result<()> {
        if !self.weights.contains_key(&key) {
            let current = self
                .engine
                .get(&key)?
                .map_or(0.0, |v| schema::f32_value(&v));
            self.weights.insert(key.clone(), current);
        }
        *self.weights.get_mut(&key).expect("inséré ci-dessus") += delta;
        Ok(())
    }

    /// Identifiant du lemme, créé s'il n'existe pas.
    fn lemma(&mut self, lang: Lang, pos: Pos, lemma: &str) -> Result<NodeId> {
        let norm = schema::normalize_lemma(lemma);
        if let Some(&id) = self.lemmas.get(&(pos, norm.clone())) {
            return Ok(id);
        }
        let key = schema::lexeme_key(lang, pos, &norm);
        let id = match self.engine.get(&key)? {
            Some(v) => NodeId(schema::u64_value(Some(v))),
            None => {
                let id = self.next();
                self.put(key, id.0.to_be_bytes().to_vec());
                self.put(
                    schema::id_key(schema::NODE, id),
                    encode(&LemmaRecord {
                        lang: lang.as_str().to_owned(),
                        pos: pos as u8,
                        lemma: lemma.to_owned(),
                    })?,
                );
                id
            }
        };
        self.lemmas.insert((pos, norm), id);
        Ok(id)
    }

    fn commit(mut self) -> Result<()> {
        let next = self.counter;
        self.put(
            schema::meta_key(schema::NEXT_ID),
            next.to_be_bytes().to_vec(),
        );
        let counters: Vec<_> = self.counters.drain().collect();
        for (k, v) in counters {
            self.ops.push(Op::Put(k, v.to_be_bytes().to_vec()));
        }
        let weights: Vec<_> = self.weights.drain().collect();
        for (k, v) in weights {
            self.ops.push(Op::Put(k, v.to_le_bytes().to_vec()));
        }
        let ops = std::mem::take(&mut self.ops);
        self.engine.apply(ops)
    }
}
