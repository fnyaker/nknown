//! Moteur NKnown : graphe pondéré, index et recherche, au-dessus d'un [`Storage`] abstrait
//! (en mémoire pour les tests, redb sur disque).
//!
//! Le moteur reçoit des documents déjà analysés ([`nknown_core::analyze::Analysis`]) : il ne
//! dépend pas du traitement de la langue, seulement de son résultat.

pub mod graph;
pub mod ingest;
pub mod learn;
pub mod rank;
pub mod schema;
pub mod search;
pub mod storage;

use std::fmt;

use nknown_core::analyze::Pos;
use nknown_core::lang::Lang;
use nknown_core::source::SourceKind;
use serde::Serialize;
use serde::de::DeserializeOwned;

pub use graph::{Edge, EdgeKind, NodeId, NodeKind};
pub use ingest::{Document, Ingested, ItemStatus};
pub use learn::{LearnOptions, LearnStats, Learned, LearnedLink, LearnedSummary, LinkKind};
pub use rank::RankStats;
pub use search::{
    Hit, Intent, Modifier, Query, QueryTerm, RECENT_HALF_LIFE_DAYS, Related, ScoreParts,
    SearchResult, Stage, Thresholds, Variety, intent_for, modifier_for,
};
pub use storage::{MemStorage, Op, RedbStorage, Storage, StorageReport};

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

/// Racine d'un mot dans une langue (Snowball), si la langue en a une.
pub type Stemmer = fn(Lang, &str) -> Option<String>;

#[derive(Debug)]
pub struct Engine<S: Storage> {
    storage: S,
    stemmer: Option<Stemmer>,
}

impl<S: Storage> Engine<S> {
    pub fn new(storage: S) -> Self {
        Self {
            storage,
            stemmer: None,
        }
    }

    /// Racines des lemmes : une requête retrouve les variantes d'un mot (« midterm »,
    /// « midterms »), y compris celles indexées sous une autre catégorie.
    pub fn with_stemmer(mut self, stemmer: Stemmer) -> Self {
        self.stemmer = Some(stemmer);
        self
    }

    /// Racine d'un lemme (normalisé) ; le lemme lui-même pour une langue sans racinisation.
    pub(crate) fn stem_of(&self, lang: Lang, lemma: &str) -> String {
        let norm = schema::normalize_lemma(lemma);
        self.stemmer
            .and_then(|f| f(lang, &norm))
            .filter(|s| !s.is_empty())
            .unwrap_or(norm)
    }

    /// Lemmes indexés de racine `stem` (normalisée), avec leur catégorie.
    pub(crate) fn stem_lemmas(&self, lang: Lang, stem: &str) -> Result<Vec<(NodeId, Pos)>> {
        Ok(self
            .scan(&schema::stem_prefix(lang, stem))?
            .into_iter()
            .filter_map(|(key, value)| {
                let id = key
                    .get(key.len().saturating_sub(8)..)
                    .and_then(|b| b.try_into().ok())
                    .map(|b| NodeId(u64::from_be_bytes(b)))?;
                Some((id, value.first().and_then(|&b| Pos::from_u8(b))?))
            })
            .collect())
    }

    /// Met la base au format courant ; renvoie ce qui a été fait (journal), rien si elle l'était.
    pub fn migrate(&mut self) -> Result<Option<String>> {
        let version = schema::u64_value(self.get(&schema::meta_key(schema::SCHEMA))?);
        if version >= schema::SCHEMA_VERSION {
            return Ok(None);
        }
        // 1 → 2 : index des racines, depuis les lemmes existants.
        let mut ops = Vec::new();
        let mut count = 0usize;
        for (key, value) in self.scan(&[schema::NODE])? {
            let Ok(rec) = decode::<schema::LemmaRecord>(&value) else {
                continue;
            };
            let (Some(lang), Some(id)) = (
                Lang::from_code(&rec.lang),
                key.get(1..9)
                    .and_then(|b| b.try_into().ok())
                    .map(u64::from_be_bytes),
            ) else {
                continue;
            };
            if rec.pos == schema::CODE_TERM {
                continue;
            }
            ops.push(Op::Put(
                schema::stem_key(lang, &self.stem_of(lang, &rec.lemma), NodeId(id)),
                vec![rec.pos],
            ));
            count += 1;
            if ops.len() >= 50_000 {
                self.apply(std::mem::take(&mut ops))?;
            }
        }
        ops.push(Op::Put(
            schema::meta_key(schema::SCHEMA),
            schema::SCHEMA_VERSION.to_be_bytes().to_vec(),
        ));
        self.apply(ops)?;
        Ok(Some(format!("index des racines : {count} lemmes")))
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

    /// Parcourt les paires de préfixe `prefix` sans les charger toutes ; `f` renvoie
    /// `Ok(false)` pour arrêter. Une erreur de `f` arrête le parcours et est renvoyée.
    pub(crate) fn visit(
        &self,
        prefix: &[u8],
        mut f: impl FnMut(&[u8], &[u8]) -> Result<bool>,
    ) -> Result<()> {
        let mut failed = None;
        self.storage
            .visit_prefix(prefix, &mut |k, v| match f(k, v) {
                Ok(go) => go,
                Err(e) => {
                    failed = Some(e);
                    false
                }
            })
            .map_err(|e| Error::Storage(Box::new(e)))?;
        failed.map_or(Ok(()), Err)
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

    use nknown_core::nature::{Nature, NatureProfile};

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
                "X" => Pos::X,
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
                domain: Default::default(),
            });
        }
        let relations = qualifies
            .iter()
            .map(|&(adj, noun)| Relation::Qualifies { adj, noun })
            .collect();
        (
            text,
            Analysis {
                tokens,
                relations,
                segments: Vec::new(),
            },
        )
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
            title: None,
        })
        .unwrap()
    }

    fn query(
        e: &Engine<MemStorage>,
        words: &[(&str, &[(Pos, &str)])],
        intent: Option<&str>,
    ) -> Vec<Hit> {
        let q = Query {
            terms: words
                .iter()
                .map(|(w, rs)| QueryTerm {
                    text: w.to_string(),
                    readings: rs
                        .iter()
                        .map(|(p, l)| (Lang::FRA, schema::TermKind::Word(*p), l.to_string()))
                        .collect(),
                    ..Default::default()
                })
                .collect(),
            intent: intent.and_then(intent_for),
            ..Query::default()
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

    /// Racinisation jouet : le pluriel en -s.
    fn plural_stem(_: Lang, w: &str) -> Option<String> {
        Some(w.trim_end_matches('s').to_owned())
    }

    #[test]
    fn variants_of_a_word_are_found() {
        let midterm: (&str, &[(Pos, &str)]) = ("midterm", &[(Pos::Noun, "midterm")]);
        // « Midterms » indexé comme nom propre, tel quel (titre d'actualité).
        let mut plain = Engine::new(MemStorage::default());
        add(
            &mut plain,
            SourceKind::Feed,
            1,
            "Midterms/NP approchent/V",
            &[],
        );
        assert!(query(&plain, &[midterm], None).is_empty());

        let mut e = Engine::new(MemStorage::default()).with_stemmer(plural_stem);
        add(&mut e, SourceKind::Feed, 1, "Midterms/NP approchent/V", &[]);
        add(&mut e, SourceKind::Feed, 1, "le/D midterm/N décisif/A", &[]);
        assert_eq!(query(&e, &[midterm], None).len(), 2);

        // Base d'avant l'index des racines : la migration le construit.
        let mut migrated = Engine {
            storage: plain.storage,
            stemmer: Some(plural_stem),
        };
        assert!(migrated.migrate().unwrap().is_some());
        assert!(migrated.migrate().unwrap().is_none());
        assert_eq!(query(&migrated, &[midterm], None).len(), 1);
    }

    #[test]
    fn related_words_are_found_below_the_word_itself() {
        let mut e = Engine::new(MemStorage::default()).with_stemmer(plural_stem);
        add(
            &mut e,
            SourceKind::Feed,
            1,
            "le/D scrutin/N de/D dimanche/N",
            &[],
        );
        add(
            &mut e,
            SourceKind::Feed,
            1,
            "une/D élection/N disputée/A",
            &[],
        );
        add(
            &mut e,
            SourceKind::Feed,
            1,
            "le/D marché/N du/D dimanche/N",
            &[],
        );
        let term = |related: Vec<(Lang, String, f32)>| QueryTerm {
            text: "élection".into(),
            readings: vec![(
                Lang::FRA,
                schema::TermKind::Word(Pos::Noun),
                "élection".into(),
            )],
            related,
        };
        let search = |terms: Vec<QueryTerm>| {
            e.search(
                &Query {
                    terms,
                    ..Query::default()
                },
                NOW,
                10,
            )
            .unwrap()
        };
        assert_eq!(search(vec![term(Vec::new())]).len(), 1);

        let linked = vec![(Lang::FRA, "scrutin".to_owned(), 0.6)];
        let hits = search(vec![term(linked.clone())]);
        assert_eq!(hits.len(), 2, "{hits:#?}");
        assert!(hits[0].text.contains("élection"));
        // Le mot trouvé à la place est montré (extrait, surlignage).
        assert_eq!(hits[1].matched, vec!["scrutin".to_owned()]);
        assert!(hits[1].score < hits[0].score);

        // Avec un autre mot de la requête, le passage qui a les deux gagne.
        let dimanche = QueryTerm {
            text: "dimanche".into(),
            readings: vec![(
                Lang::FRA,
                schema::TermKind::Word(Pos::Noun),
                "dimanche".into(),
            )],
            ..Default::default()
        };
        let hits = search(vec![term(linked), dimanche]);
        assert!(hits[0].text.contains("scrutin"), "{hits:#?}");
    }

    #[test]
    fn links_are_learned_from_shared_contexts() {
        let mut e = Engine::new(MemStorage::default());
        // Mots de remplissage uniques par document (sans chiffres : ils seraient écartés).
        let filler = |k: usize| -> String {
            let a = (b'a' + (k % 26) as u8) as char;
            let b = (b'a' + (k / 26 % 26) as u8) as char;
            format!("z{a}{b}q/N y{b}{a}w/N")
        };
        let vote_adjs = [
            "présidentiel",
            "municipal",
            "anticipé",
            "législatif",
            "régional",
        ];
        let person_adjs = [
            "républicain",
            "démocrate",
            "malheureux",
            "sortant",
            "favori",
        ];
        let market_adjs = [
            "couvert",
            "hebdomadaire",
            "artisanal",
            "nocturne",
            "estival",
        ];
        let politics = ["voix", "tour", "résultat", "campagne", "bureau"];
        let market = ["légume", "fruit", "étal", "panier", "primeur"];
        let mut k = 0;
        for round in 0..12 {
            for (topic, adjs, person, ctx) in [
                ("élection", &vote_adjs, Some("candidat"), &politics),
                ("scrutin", &vote_adjs, Some("candidat"), &politics),
                ("marché", &market_adjs, None, &market),
                ("foire", &market_adjs, None, &market),
            ] {
                let a1 = adjs[round % adjs.len()];
                let a2 = adjs[(round + 2) % adjs.len()];
                let mut spec = format!("{topic}/N {a1}/A {a2}/A");
                let mut q = vec![(1, 0), (2, 0)];
                if let Some(p) = person {
                    let pa = person_adjs[(round + k) % person_adjs.len()];
                    spec.push_str(&format!(" {p}/N {pa}/A"));
                    q.push((4, 3));
                }
                for i in 0..3 {
                    spec.push_str(&format!(" {}/N", ctx[(round + i) % ctx.len()]));
                }
                spec.push_str(&format!(" dimanche/N {} Dupont/NP", filler(k)));
                add(&mut e, SourceKind::Feed, 1, &spec, &q);
                k += 1;
            }
        }
        let opts = LearnOptions {
            min_df: 5,
            max_share: 0.9,
            min_shared: 3,
            ..LearnOptions::default()
        };
        let stats = e.learn(opts).unwrap();
        assert!(stats.syntactic > 0 && stats.qualified > 0, "{stats:?}");
        let of = |w: &str, kind: LinkKind| -> Vec<String> {
            e.learned(Lang::FRA, schema::TermKind::Word(Pos::Noun), w)
                .unwrap()
                .into_iter()
                .filter(|l| l.kind == kind)
                .map(|l| l.lemma)
                .collect()
        };
        let election = of("élection", LinkKind::Syntactic);
        assert!(election.contains(&"scrutin".to_owned()), "{election:?}");
        // Même sujet, autres adjectifs : pas interchangeable.
        assert!(!election.contains(&"candidat".to_owned()), "{election:?}");
        assert!(!election.contains(&"marché".to_owned()), "{election:?}");
        assert!(of("marché", LinkKind::Syntactic).contains(&"foire".to_owned()));
        // Les contextes de passage rapprochent aussi le sujet (« candidat ») : ces liens-là
        // ne servent qu'à confirmer.
        let topical = of("élection", LinkKind::Topical);
        assert!(!topical.contains(&"marché".to_owned()), "{topical:?}");
        // Noms propres : jamais appris.
        assert!(
            e.learned(Lang::FRA, schema::TermKind::Word(Pos::Propn), "Dupont")
                .unwrap()
                .is_empty()
        );
        // Base inchangée depuis : rien à réapprendre.
        assert!(!e.learning_due(1, 0.2).unwrap());
        // Résumé pour les statistiques : les mêmes comptes que le calcul.
        let summary = e.learned_summary().unwrap();
        assert_eq!(summary.syntactic as usize, stats.syntactic, "{summary:?}");
        assert_eq!(summary.topical as usize, stats.links, "{summary:?}");
        assert!(summary.words > 0 && summary.at > 0 && summary.at_passages > 0);
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
        assert_ne!(hits[0].doc_id, hits[1].doc_id);
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
                title: None,
            })
            .unwrap();
        assert!(matches!(r, Ingested::Added { passages: 2, .. }));
    }

    fn item(ext: &str, spec: &str) -> (RawItem, Analysis) {
        let (text, analysis) = doc(spec, &[(1, 0)]);
        let mut item = RawItem::new(SourceId("wiki".into()), text, NOW);
        item.external_id = Some(ext.into());
        (item, analysis)
    }

    fn ingest(e: &mut Engine<MemStorage>, item: &RawItem, analysis: &Analysis) -> Ingested {
        e.ingest(Document {
            item,
            kind: SourceKind::Encyclopedia,
            lang: Lang::FRA,
            analysis,
            title: None,
        })
        .unwrap()
    }

    /// Tout le contenu de la base sauf le compteur d'identifiants (qui avance).
    fn dump(e: &Engine<MemStorage>) -> Vec<(Vec<u8>, Vec<u8>)> {
        e.scan(b"")
            .unwrap()
            .into_iter()
            .filter(|(k, _)| k[0] != b'M' || !k.ends_with(b"next_id"))
            .collect()
    }

    #[test]
    fn updates_replace_the_previous_version() {
        let (v1, a1) = item("Paris", "rues/N/rue calmes/A/calme");
        let (v2, a2) = item("Paris", "places/N/place animées/A/animé");

        let mut e = Engine::new(MemStorage::default());
        assert_eq!(e.status(&v1).unwrap(), ItemStatus::New);
        assert!(matches!(ingest(&mut e, &v1, &a1), Ingested::Added { .. }));
        assert!(matches!(e.status(&v1).unwrap(), ItemStatus::Unchanged(_)));
        assert!(matches!(e.status(&v2).unwrap(), ItemStatus::Changed(_)));
        assert!(matches!(ingest(&mut e, &v2, &a2), Ingested::Updated { .. }));

        let rue = [("rues", &[(Pos::Noun, "rue")][..])];
        assert!(
            query(&e, &rue, None).is_empty(),
            "l'ancienne version reste trouvable"
        );
        assert_eq!(e.stats().unwrap().docs, 1);

        // Statistiques et liens : comme si seule la nouvelle version avait été indexée
        // (à la numérotation près).
        let mut fresh = Engine::new(MemStorage::default());
        ingest(&mut fresh, &v2, &a2);
        let keep = |d: Vec<(Vec<u8>, Vec<u8>)>| -> Vec<(u8, usize)> {
            let mut kinds: Vec<(u8, usize)> = Vec::new();
            for (k, _) in d {
                match kinds.iter_mut().find(|(c, _)| *c == k[0]) {
                    Some((_, n)) => *n += 1,
                    None => kinds.push((k[0], 1)),
                }
            }
            kinds
        };
        let stats = |e: &Engine<MemStorage>| {
            e.scan(b"S")
                .unwrap()
                .into_iter()
                .filter(|(_, v)| schema::u64_value(Some(v.clone())) > 0)
                .count()
        };
        // Les lemmes de l'ancienne version restent connus (avec df = 0) : seules les
        // entrées vivantes doivent coïncider.
        assert_eq!(stats(&e), stats(&fresh));
        let live = |d: Vec<(Vec<u8>, Vec<u8>)>| {
            d.into_iter()
                .filter(|(k, v)| {
                    !matches!(k[0], b'L' | b'N' | b'S' | b'T')
                        && !(k[0] == b'E'
                            && schema::f32_value(v) == 0.0
                            && k.len() == 18
                            && k[9] == EdgeKind::Qualifies as u8)
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(keep(live(dump(&e))), keep(live(dump(&fresh))));
        assert_eq!(e.stats().unwrap(), fresh.stats().unwrap());
    }

    #[test]
    fn removal_and_checkpoints() {
        let (v1, a1) = item("Lyon", "rues/N/rue calmes/A/calme");
        let mut e = Engine::new(MemStorage::default());
        let Ingested::Added { doc, .. } = ingest(&mut e, &v1, &a1) else {
            panic!()
        };
        assert!(e.remove(doc).unwrap());
        assert!(!e.remove(doc).unwrap());
        assert_eq!(e.stats().unwrap(), Stats::default());
        assert_eq!(e.status(&v1).unwrap(), ItemStatus::New);

        assert_eq!(e.checkpoint("rss:x").unwrap(), None);
        e.set_checkpoint("rss:x", NOW).unwrap();
        assert_eq!(e.checkpoint("rss:x").unwrap(), Some(NOW));
    }

    /// Document mêlant prose et code : `spec` comme pour `doc`, « | » sépare les blocs,
    /// un bloc préfixé par « code: » est du code (ses mots sont des termes de code).
    fn mixed(e: &mut Engine<MemStorage>, ext: &str, blocks: &[&str]) {
        use nknown_core::analyze::{Domain, Segment, SegmentKind};
        let mut text = String::new();
        let mut analysis = Analysis::default();
        for b in blocks {
            let (is_code, spec) = match b.strip_prefix("code:") {
                Some(s) => (true, s),
                None => (false, *b),
            };
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            let (t, mut a) = doc(spec, &[]);
            let offset = text.len();
            text.push_str(&t);
            let first = analysis.tokens.len();
            for mut tok in a.tokens.drain(..) {
                tok.span = tok.span.start + offset..tok.span.end + offset;
                if is_code {
                    tok.domain = Domain::Code;
                    tok.pos = Pos::X;
                }
                analysis.tokens.push(tok);
            }
            analysis.segments.push(Segment {
                tokens: first..analysis.tokens.len(),
                kind: if is_code {
                    SegmentKind::Code {
                        lang: Some("rust".into()),
                    }
                } else {
                    SegmentKind::Prose
                },
                lang: None,
            });
        }
        let mut item = RawItem::new(SourceId(ext.into()), text, NOW);
        item.external_id = Some(ext.into());
        e.ingest(Document {
            item: &item,
            kind: SourceKind::Web,
            lang: Lang::FRA,
            analysis: &analysis,
            title: None,
        })
        .unwrap();
    }

    fn search_kinds(e: &Engine<MemStorage>, word: &str, intent: Option<&str>) -> Vec<Hit> {
        let q = Query {
            terms: vec![QueryTerm {
                text: word.into(),
                readings: vec![
                    (Lang::FRA, schema::TermKind::Word(Pos::Noun), word.into()),
                    (Lang::NONE, schema::TermKind::Code, word.into()),
                ],
                ..Default::default()
            }],
            intent: intent.and_then(intent_for),
            ..Query::default()
        };
        e.search(&q, NOW, 10).unwrap()
    }

    #[test]
    fn code_and_prose_words_stay_apart() {
        let mut e = Engine::new(MemStorage::default());
        // Un récit anglais où « map » est un mot ordinaire...
        mixed(
            &mut e,
            "story",
            &["we/D read/V the/D map/N/map and/D found/V the/D road/N"],
        );
        // ...et une page technique : prose, puis code utilisant `map`.
        mixed(
            &mut e,
            "tuto",
            &[
                "transformer/V une/D liste/N",
                "code:items/X iter/X map/X collect/X",
            ],
        );
        let p = e.stats().unwrap().passages;
        assert_eq!(p, 3, "les passages ne franchissent pas les blocs");

        // Sans intention, les deux sens sont trouvés.
        let all = search_kinds(&e, "map", None);
        assert_eq!(all.len(), 2);
        // Avec « code », le passage de code passe devant le récit.
        let code = search_kinds(&e, "map", Some("code"));
        assert!(
            code[0].is_code && code[0].code_lang.as_deref() == Some("rust"),
            "{code:#?}"
        );
        assert!(!code[1].is_code);
        // Le récit n'est pas devenu technique.
        let story = all.iter().find(|h| !h.is_code).unwrap();
        assert_eq!(
            NatureProfile::from_scores(story.doc.nature).get(Nature::Technical),
            0.0
        );
    }

    fn add_from(e: &mut Engine<MemStorage>, source: &str, age_days: i64, spec: &str) -> NodeId {
        let (text, analysis) = doc(spec, &[]);
        let mut item = RawItem::new(SourceId(source.into()), text, NOW);
        item.title = Some(spec.split('/').next().unwrap_or_default().to_owned());
        item.published_at = Some(Dated {
            at: Timestamp(NOW.0 - age_days * DAY),
            confidence: DateConfidence::Declared,
        });
        match e
            .ingest(Document {
                item: &item,
                kind: SourceKind::Feed,
                lang: Lang::FRA,
                analysis: &analysis,
                title: None,
            })
            .unwrap()
        {
            Ingested::Added { doc, .. } => doc,
            other => panic!("{other:?}"),
        }
    }

    /// Bruit de fond : sans lui, tous les termes seraient « trop fréquents » pour servir
    /// de point d'entrée aux liens.
    fn background(e: &mut Engine<MemStorage>) {
        for i in 0..30 {
            add_from(
                e,
                "fond",
                400,
                &format!("sujet{i}/N divers{i}/N thème{i}/N rubrique{i}/N chronique{i}/N"),
            );
        }
    }

    const BHV: &str = "BHV/NP Marais/NP redressement/N judiciaire/A magasin/N salariés/N";

    fn similar_edges(e: &Engine<MemStorage>) -> usize {
        e.scan(b"E")
            .unwrap()
            .iter()
            .filter(|(k, _)| k.len() == 18 && k[9] == EdgeKind::Similar as u8)
            .count()
    }

    #[test]
    fn similar_passages_are_linked() {
        let mut e = Engine::new(MemStorage::default());
        background(&mut e);
        let before = similar_edges(&e);
        let a = add_from(&mut e, "lemonde", 0, BHV);
        let b = add_from(
            &mut e,
            "franceinfo",
            0,
            "BHV/NP Marais/NP placé/V/placer en/D redressement/N judiciaire/A magasin/N",
        );
        assert_eq!(similar_edges(&e) - before, 2, "un lien dans chaque sens");
        let hits = query(&e, &[("bhv", &[(Pos::Propn, "BHV")])], None);
        // Quasi-doublons : un seul résultat, l'autre en « aussi ».
        assert_eq!(hits.len(), 1, "{hits:#?}");
        let other = if hits[0].doc_id == a { b } else { a };
        assert_eq!(hits[0].related[0].doc_id, other);
        // Retirer un document retire ses liens.
        e.remove(other).unwrap();
        assert_eq!(similar_edges(&e), before);
        assert!(
            query(&e, &[("bhv", &[(Pos::Propn, "BHV")])], None)[0]
                .related
                .is_empty()
        );
    }

    #[test]
    fn unrelated_passages_are_not_linked() {
        let mut e = Engine::new(MemStorage::default());
        background(&mut e);
        let before = similar_edges(&e);
        add_from(&mut e, "a", 0, BHV);
        add_from(
            &mut e,
            "b",
            0,
            "tournoi/N tennis/N Bercy/NP finale/N joueur/N blessure/N",
        );
        assert_eq!(similar_edges(&e), before);
    }

    #[test]
    fn widely_covered_news_gets_a_burst_bonus() {
        let mut e = Engine::new(MemStorage::default());
        background(&mut e);
        // Sujet isolé, plus « pertinent » textuellement (Paris deux fois)...
        add_from(
            &mut e,
            "blog",
            0,
            "Paris/NP Paris/NP exposition/N peinture/N galerie/N vernissage/N",
        );
        // ...et sujet repris par trois sources, mais avec des formulations différentes.
        add_from(
            &mut e,
            "s1",
            0,
            "Paris/NP grève/N métro/N RATP/NP lignes/N/ligne fermées/A/fermé",
        );
        add_from(
            &mut e,
            "s2",
            0,
            "grève/N RATP/NP Paris/NP métro/N perturbé/A trafic/N",
        );
        add_from(
            &mut e,
            "s3",
            1,
            "RATP/NP grève/N métro/N Paris/NP usagers/N/usager trafic/N",
        );
        let paris = [PARIS];
        let plain = query(&e, &paris, None);
        assert!(
            plain[0].text.contains("exposition"),
            "sans intention : {plain:#?}"
        );
        let actu = query(&e, &paris, Some("actu"));
        assert!(actu[0].text.contains("grève"), "avec actu : {actu:#?}");
    }

    #[test]
    fn title_words_weigh_more() {
        let mut e = Engine::new(MemStorage::default());
        background(&mut e);
        let ingest_titled =
            |e: &mut Engine<MemStorage>, src: &str, title: Option<&str>, body: &str| {
                let (text, analysis) = doc(body, &[]);
                let title_analysis = title.map(|t| doc(t, &[]).1);
                let mut item = RawItem::new(SourceId(src.into()), text, NOW);
                item.title = title.map(|t| t.split('/').next().unwrap_or_default().to_owned());
                e.ingest(Document {
                    item: &item,
                    kind: SourceKind::Web,
                    lang: Lang::FRA,
                    analysis: &analysis,
                    title: title_analysis.as_ref(),
                })
                .unwrap();
            };
        // Corps aussi pertinents l'un que l'autre ; un seul a « transports » dans son titre.
        ingest_titled(
            &mut e,
            "sans",
            Some("Urbanisme/N"),
            "réseau/N bus/N transports/N/transport ville/N",
        );
        ingest_titled(
            &mut e,
            "avec",
            Some("Transports/N/transport"),
            "trafic/N tram/N transports/N/transport quartier/N",
        );
        let hits = query(&e, &[("transports", &[(Pos::Noun, "transport")])], None);
        assert_eq!(hits[0].doc.source, "avec", "{hits:#?}");
        assert!(hits[0].score > hits[1].score * 1.5);
    }

    #[test]
    fn titles_alone_make_candidates() {
        let mut e = Engine::new(MemStorage::default());
        let (text, analysis) = doc("fn/X decode/X scores/X tree/X", &[]);
        let title = doc("mst/X", &[]).1;
        let mut item = RawItem::new(SourceId("files".into()), text, NOW);
        item.title = Some("mst".into());
        let Ingested::Added { doc: id, .. } = e
            .ingest(Document {
                item: &item,
                kind: SourceKind::LocalFile,
                lang: Lang::FRA,
                analysis: &analysis,
                title: Some(&title),
            })
            .unwrap()
        else {
            panic!()
        };
        let hits = query(&e, &[("mst", &[(Pos::X, "mst")])], None);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].doc_id, id);
        // Le lien titre disparaît avec le document.
        e.remove(id).unwrap();
        assert!(query(&e, &[("mst", &[(Pos::X, "mst")])], None).is_empty());
        assert!(e.scan(b"E").unwrap().is_empty());
    }

    #[test]
    fn blocks_are_indexed_in_their_language() {
        use nknown_core::analyze::{Domain, Segment, SegmentKind};
        let mut e = Engine::new(MemStorage::default());
        // Bloc anglais (racines), puis code : le terme `return` du code est sans langue.
        let (text, mut analysis) = doc("reads/X/read config/X/config return/X/return x/X", &[]);
        analysis.tokens[3].domain = Domain::Code;
        analysis.segments = vec![
            Segment {
                tokens: 0..3,
                kind: SegmentKind::Prose,
                lang: Some(Lang::ENG),
            },
            Segment {
                tokens: 3..4,
                kind: SegmentKind::Code { lang: None },
                lang: None,
            },
        ];
        let item = RawItem::new(SourceId("x".into()), text, NOW);
        e.ingest(Document {
            item: &item,
            kind: SourceKind::Web,
            lang: Lang::FRA,
            analysis: &analysis,
            title: None,
        })
        .unwrap();
        let find = |lang: Lang, kind: schema::TermKind, w: &str| {
            e.search(
                &Query {
                    terms: vec![QueryTerm {
                        text: w.into(),
                        readings: vec![(lang, kind, w.into())],
                        ..Default::default()
                    }],
                    ..Query::default()
                },
                NOW,
                5,
            )
            .unwrap()
            .len()
        };
        use schema::TermKind::{Code, Word};
        assert_eq!(find(Lang::ENG, Word(Pos::X), "return"), 1);
        assert_eq!(
            find(Lang::FRA, Word(Pos::X), "return"),
            0,
            "pas sous la langue du document"
        );
        assert_eq!(find(Lang::NONE, Code, "x"), 1);
        assert_eq!(find(Lang::FRA, Code, "x"), 0);
    }

    fn explained(
        e: &Engine<MemStorage>,
        words: &[(&str, &[(Pos, &str)])],
        intent: Option<&str>,
        limit: usize,
    ) -> SearchResult {
        let q = Query {
            terms: words
                .iter()
                .map(|(w, rs)| QueryTerm {
                    text: w.to_string(),
                    readings: rs
                        .iter()
                        .map(|(p, l)| (Lang::FRA, schema::TermKind::Word(*p), l.to_string()))
                        .collect(),
                    ..Default::default()
                })
                .collect(),
            intent: intent.and_then(intent_for),
            ..Query::default()
        };
        e.search_explained(&q, NOW, limit).unwrap()
    }

    #[test]
    fn scores_are_the_product_of_their_parts() {
        let e = corpus();
        let r = explained(&e, &[PARIS], Some("actu"), 5);
        assert!(!r.hits.is_empty());
        for h in &r.hits {
            assert!((h.score - h.parts.total()).abs() < 1e-9);
            assert!(h.parts.freshness < 1.0 || h.doc.date == NOW.0);
        }
    }

    #[test]
    fn thresholds_follow_the_candidates() {
        let mut e = Engine::new(MemStorage::default());
        background(&mut e);
        // Un seul mot : la couverture ne compte pas.
        let r = explained(&e, &[("sujet1", &[(Pos::Noun, "sujet1")])], None, 5);
        assert_eq!(r.thresholds.coverage_exponent, 1);

        // Corpus ancien (60 jours) : pour « actu », la demi-vie s'allonge au lieu
        // d'écraser tous les résultats pareil.
        for i in 0..6 {
            add_from(
                &mut e,
                &format!("s{i}"),
                60,
                &format!("Paris/NP réunion{i}/N conseil/N"),
            );
        }
        let r = explained(&e, &[PARIS], Some("actu"), 5);
        let hl = r.thresholds.half_life_days.unwrap();
        assert!(hl > 50.0 && hl <= 90.0, "demi-vie {hl}");

        // Deux mots, assez de passages qui contiennent les deux : couverture stricte.
        let both = [PARIS, ("conseil", &[(Pos::Noun, "conseil")][..])];
        let r = explained(&e, &both, None, 3);
        assert_eq!(r.thresholds.coverage_exponent, 3);
        // Plus de candidats : coupure plus haute.
        let few = explained(&e, &[("sujet1", &[(Pos::Noun, "sujet1")])], None, 5);
        assert!(r.thresholds.cutoff >= few.thresholds.cutoff);
        assert!(r.candidates > few.candidates);
    }

    #[test]
    fn central_documents_gain_authority() {
        let mut e = Engine::new(MemStorage::default());
        background(&mut e);
        // Un document carrefour, proche de trois autres qui ne se ressemblent pas entre eux.
        let hub = add_from(
            &mut e,
            "hub",
            0,
            "réseau/N métro/N tramway/N vélo/N autobus/N péage/N",
        );
        let a = add_from(&mut e, "a", 0, "métro/N tramway/N ligne/N station/N quai/N");
        add_from(&mut e, "b", 0, "vélo/N autobus/N piste/N casque/N selle/N");
        add_from(
            &mut e,
            "c",
            0,
            "péage/N réseau/N autoroute/N barrière/N radar/N",
        );
        let stats = e.compute_authority().unwrap();
        assert!(stats.links >= 3, "{stats:?}");
        assert!(e.authority(hub).unwrap() > e.authority(a).unwrap());
        assert!(e.authority(hub).unwrap() > 1.0);
        // Un document isolé (bruit de fond) reste sous la moyenne.
        let r = explained(&e, &[("réseau", &[(Pos::Noun, "réseau")])], None, 5);
        let h = r.hits.iter().find(|h| h.doc_id == hub).unwrap();
        assert!(h.parts.authority > 1.0, "{:?}", h.parts);
    }

    #[test]
    fn variety_scales_expansion_constants() {
        assert_eq!(Variety::default(), Variety::Normal);
        assert_eq!(Variety::Strict.related_share(), 0.35);
        assert_eq!(Variety::Normal.related_share(), 0.5);
        assert_eq!(Variety::Diverse.related_share(), 0.60);
        assert_eq!(Variety::Strict.related_kept(), 4);
        assert_eq!(Variety::Normal.related_kept(), 8);
        assert_eq!(Variety::Diverse.related_kept(), 16);
        assert_eq!(Variety::Strict.related_coverage(), 0.5);
        assert_eq!(Variety::Normal.related_coverage(), 1.0);
        assert_eq!(Variety::Diverse.related_coverage(), 1.0);
    }

    /// Un mot lié seul compte pour un demi-mot en Strict, pour un mot entier ailleurs ;
    /// Normal garde exactement l'ancienne couverture (`matched.len() / terms`).
    #[test]
    fn variety_changes_coverage_of_related_matches() {
        let mut e = Engine::new(MemStorage::default());
        add_from(&mut e, "direct", 0, "alpha/N gamma/N");
        add_from(&mut e, "via", 0, "beta/N gamma/N");
        add_from(&mut e, "partiel", 0, "alpha/N");
        let query = |variety| Query {
            terms: vec![
                QueryTerm {
                    text: "alpha".into(),
                    readings: vec![(Lang::FRA, schema::TermKind::Word(Pos::Noun), "alpha".into())],
                    related: vec![(Lang::FRA, "beta".into(), 1.0)],
                },
                QueryTerm {
                    text: "gamma".into(),
                    readings: vec![(Lang::FRA, schema::TermKind::Word(Pos::Noun), "gamma".into())],
                    related: Vec::new(),
                },
            ],
            variety,
            ..Query::default()
        };
        let search = |variety| e.search_explained(&query(variety), NOW, 10).unwrap();
        let coverage = |r: &SearchResult, source: &str| {
            r.hits
                .iter()
                .find(|h| h.doc.source == source)
                .map(|h| h.parts.coverage)
                .unwrap_or_else(|| panic!("document {source} absent"))
        };

        let normal = search(Variety::Normal);
        let exp = normal.thresholds.coverage_exponent;
        assert_eq!(coverage(&normal, "via"), 1.0);
        assert_eq!(coverage(&normal, "partiel"), (1.0f64 / 2.0).powi(exp));
        for h in &normal.hits {
            let expected = (h.matched.len() as f64 / 2.0).powi(exp);
            assert!((h.parts.coverage - expected).abs() < 1e-12);
        }

        let strict = search(Variety::Strict);
        let exp = strict.thresholds.coverage_exponent;
        assert_eq!(coverage(&strict, "via"), (0.75f64).powi(exp));
        assert_eq!(coverage(&strict, "direct"), 1.0);

        let diverse = search(Variety::Diverse);
        assert_eq!(coverage(&diverse, "via"), 1.0);
    }
}
