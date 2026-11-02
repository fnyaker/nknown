//! Moteur NKnown : graphe pondéré, index et recherche, au-dessus d'un [`Storage`] abstrait
//! (en mémoire pour les tests, redb sur disque).
//!
//! Le moteur reçoit des documents déjà analysés ([`nknown_core::analyze::Analysis`]) : il ne
//! dépend pas du traitement de la langue, seulement de son résultat.

pub mod graph;
pub mod ingest;
pub mod schema;
pub mod search;
pub mod storage;

use std::fmt;

use nknown_core::source::SourceKind;
use serde::Serialize;
use serde::de::DeserializeOwned;

pub use graph::{Edge, EdgeKind, NodeId, NodeKind};
pub use ingest::{Document, Ingested};
pub use search::{Hit, Intent, Query, QueryTerm, intent_for};
pub use storage::{MemStorage, Op, RedbStorage, Storage};

#[derive(Debug)]
pub enum Error {
    Storage(Box<dyn std::error::Error + Send + Sync>),
    Codec(postcard::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Storage(e) => write!(f, "stockage : {e}"),
            Error::Codec(e) => write!(f, "enregistrement illisible : {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Storage(e) => Some(e.as_ref()),
            Error::Codec(e) => Some(e),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn encode<T: Serialize>(v: &T) -> Result<Vec<u8>> {
    postcard::to_allocvec(v).map_err(Error::Codec)
}

pub(crate) fn decode<T: DeserializeOwned>(b: &[u8]) -> Result<T> {
    postcard::from_bytes(b).map_err(Error::Codec)
}

pub fn source_kind_code(kind: SourceKind) -> u8 {
    match kind {
        SourceKind::Feed => 0,
        SourceKind::Encyclopedia => 1,
        SourceKind::Web => 2,
        SourceKind::LocalFile => 3,
        SourceKind::Other => 4,
    }
}

/// Statistiques globales de la base.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub docs: u64,
    pub passages: u64,
    pub indexed_words: u64,
}

#[derive(Debug)]
pub struct Engine<S: Storage> {
    storage: S,
}

impl<S: Storage> Engine<S> {
    pub fn new(storage: S) -> Self {
        Self { storage }
    }

    pub fn stats(&self) -> Result<Stats> {
        let get =
            |name| -> Result<u64> { Ok(schema::u64_value(self.get(&schema::meta_key(name))?)) };
        Ok(Stats {
            docs: get(schema::DOCS)?,
            passages: get(schema::PASSAGES)?,
            indexed_words: get(schema::TOTAL_LEN)?,
        })
    }

    pub(crate) fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        self.storage
            .get(key)
            .map_err(|e| Error::Storage(Box::new(e)))
    }

    pub(crate) fn scan(&self, prefix: &[u8]) -> Result<Vec<storage::KvPair>> {
        self.storage
            .scan_prefix(prefix)
            .map_err(|e| Error::Storage(Box::new(e)))
    }

    pub(crate) fn apply(&mut self, ops: Vec<Op>) -> Result<()> {
        self.storage
            .apply(ops)
            .map_err(|e| Error::Storage(Box::new(e)))
    }
}

#[cfg(test)]
mod tests {
    use nknown_core::analyze::{Analysis, Feats, Pos, Relation, Token};
    use nknown_core::item::RawItem;
    use nknown_core::lang::Lang;
    use nknown_core::source::{SourceId, SourceKind};
    use nknown_core::time::{DateConfidence, Dated, Timestamp};

    use super::*;

    const DAY: i64 = 86_400;
    const NOW: Timestamp = Timestamp(1_800_000_000);

    /// Analyse jouet : chaque mot est « forme/CAT » ou « forme/CAT/lemme ».
    fn doc(text_spec: &str, qualifies: &[(usize, usize)]) -> (String, Analysis) {
        let mut text = String::new();
        let mut tokens = Vec::new();
        for w in text_spec.split(' ') {
            let parts: Vec<&str> = w.split('/').collect();
            let pos = match parts[1] {
                "N" => Pos::Noun,
                "NP" => Pos::Propn,
                "A" => Pos::Adj,
                "V" => Pos::Verb,
                "P" => Pos::Punct,
                _ => Pos::Det,
            };
            if !text.is_empty() {
                text.push(' ');
            }
            let start = text.len();
            text.push_str(parts[0]);
            tokens.push(Token {
                span: start..text.len(),
                lemma: parts.get(2).unwrap_or(&parts[0]).to_string(),
                pos,
                feats: Feats::default(),
            });
        }
        let relations = qualifies
            .iter()
            .map(|&(adj, noun)| Relation::Qualifies { adj, noun })
            .collect();
        (text, Analysis { tokens, relations })
    }

    fn add(
        e: &mut Engine<MemStorage>,
        kind: SourceKind,
        age_days: i64,
        spec: &str,
        q: &[(usize, usize)],
    ) -> Ingested {
        let (text, analysis) = doc(spec, q);
        let mut item = RawItem::new(SourceId(format!("{kind:?}")), text, NOW);
        item.title = Some(spec.chars().take(20).collect());
        item.published_at = Some(Dated {
            at: Timestamp(NOW.0 - age_days * DAY),
            confidence: DateConfidence::Declared,
        });
        e.ingest(Document {
            item: &item,
            kind,
            lang: Lang::FRA,
            analysis: &analysis,
        })
        .unwrap()
    }

    fn query(
        e: &Engine<MemStorage>,
        words: &[(&str, &[(Pos, &str)])],
        intent: Option<&str>,
    ) -> Vec<Hit> {
        let q = Query {
            lang: Lang::FRA,
            terms: words
                .iter()
                .map(|(w, rs)| QueryTerm {
                    text: w.to_string(),
                    readings: rs.iter().map(|(p, l)| (*p, l.to_string())).collect(),
                })
                .collect(),
            intent: intent.and_then(intent_for),
        };
        e.search(&q, NOW, 10).unwrap()
    }

    const PARIS: (&str, &[(Pos, &str)]) = ("paris", &[(Pos::Propn, "Paris"), (Pos::Noun, "pari")]);

    fn corpus() -> Engine<MemStorage> {
        let mut e = Engine::new(MemStorage::default());
        add(
            &mut e,
            SourceKind::Feed,
            1,
            "manifestation/N à/D Paris/NP ce/D soir/N ./P",
            &[],
        );
        add(
            &mut e,
            SourceKind::Encyclopedia,
            900,
            "Paris/NP est/V/être la/D capitale/N de/D la/D France/NP ./P",
            &[],
        );
        add(
            &mut e,
            SourceKind::Feed,
            20,
            "Paris/NP accueille/V/accueillir un/D salon/N",
            &[],
        );
        add(
            &mut e,
            SourceKind::Web,
            60,
            "visiter/V Paris/NP en/D hiver/N",
            &[],
        );
        add(
            &mut e,
            SourceKind::Feed,
            2,
            "les/D paris/N/pari sportifs/A/sportif explosent/V/exploser",
            &[(2, 1)],
        );
        e
    }

    #[test]
    fn intent_changes_the_ranking() {
        let e = corpus();
        let actu = query(&e, &[PARIS], Some("actu"));
        assert!(actu[0].text.starts_with("manifestation"), "{actu:#?}");
        let info = query(&e, &[PARIS], Some("info"));
        assert!(info[0].text.starts_with("Paris est"), "{info:#?}");
        assert_eq!(e.stats().unwrap().docs, 5);
    }

    #[test]
    fn rare_readings_stay_reachable() {
        let e = corpus();
        let hits = query(&e, &[("paris", &[(Pos::Noun, "pari")])], None);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].text.contains("sportifs"));
        // Avec les deux lectures, il est trouvé aussi, mais pas en premier.
        let both = query(&e, &[PARIS], Some("actu"));
        assert!(both.iter().skip(1).any(|h| h.text.contains("sportifs")));
    }

    #[test]
    fn duplicates_are_detected() {
        let mut e = corpus();
        let again = add(
            &mut e,
            SourceKind::Feed,
            1,
            "manifestation/N à/D Paris/NP ce/D soir/N ./P",
            &[],
        );
        assert!(matches!(again, Ingested::Duplicate(_)));
        assert_eq!(e.stats().unwrap().docs, 5);
    }

    #[test]
    fn qualified_pairs_get_a_bonus() {
        let mut e = Engine::new(MemStorage::default());
        // Les deux passages contiennent « rues » et « calmes », un seul les relie.
        add(
            &mut e,
            SourceKind::Web,
            10,
            "rues/N/rue calmes/A/calme de/D Paris/NP",
            &[(1, 0)],
        );
        add(
            &mut e,
            SourceKind::Web,
            10,
            "rues/N/rue de/D Paris/NP mer/N calme/A",
            &[(4, 3)],
        );
        let hits = query(
            &e,
            &[
                ("rues", &[(Pos::Noun, "rue")]),
                ("calmes", &[(Pos::Adj, "calme")]),
            ],
            None,
        );
        assert_eq!(hits.len(), 2);
        assert!(hits[0].text.starts_with("rues calmes"));
        assert_eq!(
            hits[0].qualified,
            [("calmes".to_string(), "rues".to_string())]
        );
        assert!(hits[1].qualified.is_empty());
    }

    #[test]
    fn coverage_favours_passages_with_all_words() {
        let e = corpus();
        let hits = query(&e, &[PARIS, ("capitale", &[(Pos::Noun, "capitale")])], None);
        assert!(hits[0].text.starts_with("Paris est"));
        assert_eq!(hits[0].matched.len(), 2);
    }

    #[test]
    fn paragraphs_become_passages() {
        let mut e = Engine::new(MemStorage::default());
        let (mut text, mut analysis) = doc("Paris/NP ./P", &[]);
        let (t2, a2) = doc("Lyon/NP ./P", &[]);
        let offset = text.len() + 2;
        text.push_str("\n\n");
        text.push_str(&t2);
        analysis.tokens.extend(a2.tokens.into_iter().map(|mut t| {
            t.span = t.span.start + offset..t.span.end + offset;
            t
        }));
        let item = RawItem::new(SourceId("f".into()), text, NOW);
        let r = e
            .ingest(Document {
                item: &item,
                kind: SourceKind::LocalFile,
                lang: Lang::FRA,
                analysis: &analysis,
            })
            .unwrap();
        assert!(matches!(r, Ingested::Added { passages: 2, .. }));
    }
}
