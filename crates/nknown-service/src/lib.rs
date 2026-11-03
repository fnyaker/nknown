//! Façade du cœur de NKnown, partagée par le démon local, le serveur et l'app native.
//!
//! Un [`Service`] possède le pipeline d'analyse et la base. Il est `Sync` : les recherches se
//! font en parallèle (verrou en lecture) et la collecte ne prend le verrou en écriture que le
//! temps d'enregistrer un document, l'analyse se faisant hors verrou.

pub mod config;
pub mod graph;
pub mod models;
pub mod query;
pub mod sources;
pub mod watch;

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::Instant;

use nknown_api as api;
use nknown_core::analyze::Analyzer;
use nknown_core::item::RawItem;
use nknown_core::lang::Lang;
use nknown_core::nature::Nature;
use nknown_core::source::{SourceId, SourceKind};
use nknown_core::time::Timestamp;
use nknown_engine::{Document, Engine, Ingested, ItemStatus, NodeId, RedbStorage};
use nknown_nlp::Pipeline;
use nknown_nlp::langid::LangDetector;

pub use config::Config;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Storage(String),
    Model(String),
    /// Entrée invalide (source inconnue, configuration illisible…).
    Invalid(String),
    Unsupported(&'static str),
}

impl Error {
    pub fn model(e: impl fmt::Display) -> Self {
        Error::Model(e.to_string())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "entrée/sortie : {e}"),
            Error::Storage(e) => write!(f, "base : {e}"),
            Error::Model(e) => write!(f, "modèle : {e}"),
            Error::Invalid(e) => f.write_str(e),
            Error::Unsupported(what) => write!(f, "non disponible dans cette version : {what}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<nknown_engine::Error> for Error {
    fn from(e: nknown_engine::Error) -> Self {
        Error::Storage(e.to_string())
    }
}

impl From<nknown_nlp::LexiconError> for Error {
    fn from(e: nknown_nlp::LexiconError) -> Self {
        Error::model(e)
    }
}

/// Bilan de la collecte d'une source.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CollectStats {
    pub added: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub errors: usize,
}

/// Où trouver la base, les modèles et la configuration.
#[derive(Debug, Clone)]
pub struct Options {
    pub data_dir: PathBuf,
    pub config: Config,
    /// Fichier où enregistrer les changements de configuration (`None` : pas d'écriture).
    pub config_path: Option<PathBuf>,
}

impl Options {
    /// Emplacements XDG de l'utilisateur.
    pub fn user() -> Result<Self, Error> {
        let config_path = config::config_path();
        Ok(Self {
            data_dir: config::data_dir(),
            config: Config::load(&config_path)?,
            config_path: Some(config_path),
        })
    }
}

/// Source dont l'identifiant interne n'est lié à aucune collecte : textes ajoutés par l'API.
const API_SOURCE: &str = "api";

pub struct Service {
    pipeline: Pipeline,
    detector: LangDetector,
    engine: RwLock<Engine<RedbStorage>>,
    data_dir: PathBuf,
    config: Mutex<Config>,
    config_path: Option<PathBuf>,
    /// État de collecte par `spec`.
    states: Mutex<HashMap<String, api::SourceState>>,
    collecting: AtomicUsize,
    parser: bool,
}

impl fmt::Debug for Service {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Service")
            .field("data_dir", &self.data_dir)
            .field("pipeline", &self.pipeline)
            .finish_non_exhaustive()
    }
}

impl Service {
    /// Ouvre la base (`<données>/base.redb`, créée au besoin) et charge les modèles.
    /// Sans modèles, le service fonctionne en mode dégradé (mots sans lemmes).
    pub fn open(opts: Options) -> Result<Self, Error> {
        std::fs::create_dir_all(&opts.data_dir)?;
        let models = opts.config.models_dir(&opts.data_dir);
        let mut pipeline = if models.is_dir() {
            models::load_pipeline(&models, &[])?
        } else {
            Pipeline::default()
        };
        let parser = opts.config.parser.is_some();
        if let Some(spec) = &opts.config.parser {
            let spec = opts
                .config
                .parser_precision
                .apply(&models::resolve(spec, &models));
            pipeline = models::add_model(pipeline, &spec)
                .map_err(|e| Error::Model(format!("{spec} : {e}")))?;
        }
        let storage = RedbStorage::open(opts.data_dir.join("base.redb"))
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(Self {
            pipeline,
            detector: LangDetector::new(),
            engine: RwLock::new(Engine::new(storage)),
            data_dir: opts.data_dir,
            config: Mutex::new(opts.config),
            config_path: opts.config_path,
            states: Mutex::new(HashMap::new()),
            collecting: AtomicUsize::new(0),
            parser,
        })
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn config(&self) -> Config {
        lock(&self.config).clone()
    }

    fn engine(&self) -> std::sync::RwLockReadGuard<'_, Engine<RedbStorage>> {
        self.engine.read().unwrap_or_else(|e| e.into_inner())
    }

    fn engine_mut(&self) -> std::sync::RwLockWriteGuard<'_, Engine<RedbStorage>> {
        self.engine.write().unwrap_or_else(|e| e.into_inner())
    }

    pub fn status(&self) -> Result<api::Status, Error> {
        let stats = self.engine().stats()?;
        Ok(api::Status {
            version: env!("CARGO_PKG_VERSION").into(),
            docs: stats.docs,
            passages: stats.passages,
            words: stats.indexed_words,
            langs: self
                .pipeline
                .lexicon_langs()
                .iter()
                .map(|l| l.to_string())
                .collect(),
            parser: self.parser,
            collecting: self.collecting.load(Ordering::Relaxed) > 0,
        })
    }

    // ------------------------------------------------------------ recherche

    /// Recherche ; `on_event` reçoit la progression, puis le résultat (aussi renvoyé).
    pub fn search(
        &self,
        req: &api::SearchRequest,
        mut on_event: impl FnMut(api::SearchEvent),
    ) -> Result<api::SearchResponse, Error> {
        let start = Instant::now();
        on_event(api::SearchEvent::Phase {
            phase: api::Phase::Analyze,
        });
        let q = query::build_query(&self.pipeline, &req.query);
        let terms: Vec<api::QueryTermView> = q
            .terms
            .iter()
            .map(|t| api::QueryTermView {
                text: t.text.clone(),
                code: query::is_code_term(t),
            })
            .collect();
        let intent = q.intent.as_ref().map(|i| i.name.to_owned());
        on_event(api::SearchEvent::Terms {
            intent: intent.clone(),
            terms: terms.clone(),
        });

        on_event(api::SearchEvent::Phase {
            phase: api::Phase::Search,
        });
        let result =
            self.engine()
                .search_explained(&q, Timestamp::now(), req.limit.clamp(1, 100))?;

        on_event(api::SearchEvent::Phase {
            phase: api::Phase::Graph,
        });
        let best = result
            .hits
            .first()
            .map_or(1.0, |h| h.score.max(f64::MIN_POSITIVE));
        let term_index = |text: &str| q.terms.iter().position(|t| t.text == text);
        let mut hits = Vec::with_capacity(result.hits.len());
        let mut links = Vec::with_capacity(result.hits.len());
        for h in result.hits {
            links.push(graph::HitLinks {
                term_scores: h.term_scores.clone(),
                qualified: h
                    .qualified
                    .iter()
                    .filter_map(|(a, n)| Some((term_index(a)?, term_index(n)?)))
                    .collect(),
                related: h
                    .related
                    .iter()
                    .map(|r| {
                        let title = sources::display_title(
                            r.title.as_deref(),
                            r.external_id.as_deref(),
                            &r.source,
                        );
                        (r.doc_id.0, title, r.similarity)
                    })
                    .collect(),
                source: h.doc.source.clone(),
            });
            let p = h.parts;
            hits.push(api::Hit {
                doc: h.doc_id.0,
                passage: h.passage.0,
                title: sources::display_title(
                    h.doc.title.as_deref(),
                    h.doc.external_id.as_deref(),
                    &h.doc.source,
                ),
                source: sources::label(&h.doc.source),
                source_kind: sources::api_kind(h.doc.kind),
                url: sources::open_url(
                    h.doc.url.as_deref(),
                    h.doc.external_id.as_deref(),
                    &h.doc.source,
                ),
                date: h.doc.date,
                score: h.score,
                relative: (h.score / best) as f32,
                snippet: query::snippet(&h.text, &h.matched, 240),
                matched: h.matched,
                is_code: h.is_code,
                code_lang: h.code_lang,
                nature: dominant_nature(&h.doc.nature),
                parts: api::ScoreParts {
                    text: p.text,
                    coverage: p.coverage,
                    qualified: p.qualified,
                    title: p.title,
                    nature: p.nature,
                    freshness: p.freshness,
                    burst: p.burst,
                    graph: p.graph,
                    authority: p.authority,
                },
            });
        }
        let graph = graph::build(&terms, &hits, &links);
        let response = api::SearchResponse {
            query: req.query.clone(),
            intent,
            terms,
            candidates: result.candidates,
            took_ms: (start.elapsed().as_secs_f64() * 1e4).round() / 10.0,
            hits,
            graph,
        };
        on_event(api::SearchEvent::Done(Box::new(response.clone())));
        Ok(response)
    }

    // ------------------------------------------------------------ documents

    pub fn document(&self, id: u64) -> Result<Option<api::DocumentView>, Error> {
        let engine = self.engine();
        let Some(doc) = engine.doc(NodeId(id))? else {
            return Ok(None);
        };
        let mut passages = Vec::with_capacity(doc.passages as usize);
        for i in 0..u64::from(doc.passages) {
            let pid = NodeId(doc.first_passage + i);
            if let Some(p) = engine.passage(pid)? {
                passages.push(api::PassageView {
                    id: pid.0,
                    text: p.text,
                    is_code: p.is_code,
                    code_lang: p.code_lang,
                });
            }
        }
        Ok(Some(api::DocumentView {
            id,
            title: sources::display_title(
                doc.title.as_deref(),
                doc.external_id.as_deref(),
                &doc.source,
            ),
            source: sources::label(&doc.source),
            source_kind: sources::api_kind(doc.kind),
            url: sources::open_url(doc.url.as_deref(), doc.external_id.as_deref(), &doc.source),
            date: doc.date,
            lang: doc.lang,
            nature: dominant_nature(&doc.nature),
            passages,
        }))
    }

    /// Ajoute un texte fourni directement (API, futur serveur MCP). Un même `url` remplace
    /// la version précédente ; sans `url`, le texte est identifié par son contenu.
    pub fn add_document(&self, d: api::AddDocument) -> Result<api::Added, Error> {
        if d.text.trim().is_empty() {
            return Err(Error::Invalid("texte vide".into()));
        }
        let now = Timestamp::now();
        let mut item = RawItem::new(SourceId(API_SOURCE.into()), d.text, now);
        item.external_id = Some(match &d.url {
            Some(url) => url.clone(),
            None => format!("text:{}", sources::id_of(&item.text)),
        });
        item.url = d.url;
        item.title = d.title;
        if let Some(f) = d.format {
            let hint = match f.as_str() {
                "markdown" | "md" | "plain" | "code" => f,
                lang => format!("code:{lang}"),
            };
            item.meta.push(("format".into(), hint));
        }
        let (doc, outcome) = self.ingest_item(&item, SourceKind::Other)?;
        Ok(api::Added { id: doc.0, outcome })
    }

    /// Analyse hors verrou, puis enregistre.
    fn ingest_item(
        &self,
        item: &RawItem,
        kind: SourceKind,
    ) -> Result<(NodeId, api::AddOutcome), Error> {
        if let ItemStatus::Unchanged(doc) = self.engine().status(item)? {
            return Ok((doc, api::AddOutcome::Unchanged));
        }
        let lang = item.lang_hint.unwrap_or(Lang::FRA);
        let format = item
            .meta
            .iter()
            .find(|(k, _)| k == "format")
            .map_or(nknown_nlp::Format::Plain, |(_, v)| {
                nknown_nlp::Format::from_hint(v)
            });
        let analysis = nknown_nlp::analyze_document(
            &self.pipeline,
            lang,
            &item.text,
            &format,
            Some(&self.detector),
        );
        let lang = query::main_lang(&analysis).unwrap_or(lang);
        let title = item
            .title
            .as_deref()
            .map(|t| self.pipeline.analyze(lang, t));
        let ingested = self.engine_mut().ingest(Document {
            item,
            kind,
            lang,
            analysis: &analysis,
            title: title.as_ref(),
        })?;
        Ok(match ingested {
            Ingested::Added { doc, .. } => (doc, api::AddOutcome::Added),
            Ingested::Updated { doc, .. } => (doc, api::AddOutcome::Updated),
            Ingested::Duplicate(doc) => (doc, api::AddOutcome::Unchanged),
        })
    }

    // ------------------------------------------------------------ sources

    pub fn sources(&self) -> Vec<api::SourceView> {
        let specs = lock(&self.config).sources.clone();
        let states = lock(&self.states);
        specs
            .into_iter()
            .map(|spec| {
                let last_run = sources::open(&spec)
                    .ok()
                    .and_then(|s| self.engine().checkpoint(&s.checkpoint_key()).ok().flatten())
                    .map(|t| t.0);
                api::SourceView {
                    id: sources::id_of(&spec),
                    kind: sources::kind_of(&spec),
                    state: states.get(&spec).cloned().unwrap_or(api::SourceState::Idle),
                    last_run,
                    spec,
                }
            })
            .collect()
    }

    /// Ajoute une source à la configuration (sans la collecter : voir [`Self::collect`]).
    pub fn add_source(&self, spec: &str) -> Result<api::SourceView, Error> {
        let spec = spec.trim();
        sources::open(spec)?;
        {
            let mut config = lock(&self.config);
            if !config.sources.iter().any(|s| s == spec) {
                config.sources.push(spec.to_owned());
                self.save_config(&config)?;
            }
        }
        let id = sources::id_of(spec);
        Ok(self
            .sources()
            .into_iter()
            .find(|s| s.id == id)
            .expect("source ajoutée"))
    }

    /// Retire une source de la configuration. Ses documents restent dans la base.
    pub fn remove_source(&self, id: &str) -> Result<bool, Error> {
        let mut config = lock(&self.config);
        let before = config.sources.len();
        config.sources.retain(|s| sources::id_of(s) != id);
        let removed = config.sources.len() != before;
        if removed {
            self.save_config(&config)?;
        }
        Ok(removed)
    }

    fn save_config(&self, config: &Config) -> Result<(), Error> {
        match &self.config_path {
            Some(path) => config.save(path),
            None => Ok(()),
        }
    }

    // ------------------------------------------------------------ collecte

    /// Collecte toutes les sources configurées, puis recalcule l'autorité des documents.
    pub fn collect_all(&self) -> Vec<(String, Result<CollectStats, Error>)> {
        let specs = lock(&self.config).sources.clone();
        let out: Vec<_> = specs
            .into_iter()
            .map(|spec| {
                let r = self.collect(&spec);
                (spec, r)
            })
            .collect();
        if let Err(e) = self.engine_mut().compute_authority() {
            eprintln!("autorité : {e}");
        }
        out
    }

    /// Collecte incrémentale d'une source : seuls les contenus nouveaux ou modifiés sont
    /// analysés ; une version modifiée remplace l'ancienne.
    pub fn collect(&self, spec: &str) -> Result<CollectStats, Error> {
        self.collecting.fetch_add(1, Ordering::Relaxed);
        let set_state = |s: api::SourceState| {
            lock(&self.states).insert(spec.to_owned(), s);
        };
        set_state(api::SourceState::Collecting { done: 0 });
        let result = self.collect_inner(spec, &set_state);
        set_state(match &result {
            Ok(_) => api::SourceState::Idle,
            Err(e) => api::SourceState::Failed {
                message: e.to_string(),
            },
        });
        self.collecting.fetch_sub(1, Ordering::Relaxed);
        result
    }

    fn collect_inner(
        &self,
        spec: &str,
        set_state: &dyn Fn(api::SourceState),
    ) -> Result<CollectStats, Error> {
        let mut source = sources::open(spec)?;
        let (kind, name, checkpoint) = (
            source.kind(),
            source.id().0.clone(),
            source.checkpoint_key(),
        );
        // Marge de 2 jours : un flux peut publier en retard ; les doublons sont reconnus.
        let since = self
            .engine()
            .checkpoint(&checkpoint)?
            .map(|t| Timestamp(t.0 - 2 * 86_400));
        let fetched_at = Timestamp::now();
        let known = |ext: &str| self.engine().known(&name, ext).ok().flatten();
        let items = source
            .fetch_new(since, &known)
            .map_err(|e| Error::Invalid(format!("{name} : {e}")))?;
        let mut stats = CollectStats::default();
        for (i, item) in items.enumerate() {
            match item {
                Ok(item) => match self.ingest_item(&item, kind)?.1 {
                    api::AddOutcome::Added => stats.added += 1,
                    api::AddOutcome::Updated => stats.updated += 1,
                    api::AddOutcome::Unchanged => stats.unchanged += 1,
                },
                Err(e) => {
                    eprintln!("{name} : {e}");
                    stats.errors += 1;
                }
            }
            set_state(api::SourceState::Collecting { done: i + 1 });
        }
        if stats.errors == 0 {
            self.engine_mut().set_checkpoint(&checkpoint, fetched_at)?;
        }
        Ok(stats)
    }
}

fn dominant_nature(scores: &[f32; Nature::COUNT]) -> api::Nature {
    let i = (0..Nature::COUNT)
        .max_by(|&a, &b| scores[a].total_cmp(&scores[b]))
        .unwrap_or(0);
    match Nature::ALL[i] {
        Nature::Event => api::Nature::Event,
        Nature::Reference => api::Nature::Reference,
        Nature::Opinion => api::Nature::Opinion,
        Nature::Tutorial => api::Nature::Tutorial,
        Nature::Personal => api::Nature::Personal,
        Nature::Discussion => api::Nature::Discussion,
        Nature::Technical => api::Nature::Technical,
    }
}

/// Un verrou empoisonné (panique d'un autre fil) reste utilisable : l'état protégé est
/// toujours cohérent entre deux instructions.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service() -> (tempfile::TempDir, Service) {
        let dir = tempfile::tempdir().unwrap();
        let svc = Service::open(Options {
            data_dir: dir.path().to_owned(),
            config: Config {
                sources: vec![],
                ..Config::default()
            },
            config_path: Some(dir.path().join("config.toml")),
        })
        .unwrap();
        (dir, svc)
    }

    #[test]
    fn add_then_search_with_progress_and_graph() {
        let (_dir, svc) = service();
        let added = svc
            .add_document(api::AddDocument {
                text: "Le moteur de recherche indexe les fichiers locaux.".into(),
                title: Some("Moteur".into()),
                url: None,
                format: None,
            })
            .unwrap();
        assert_eq!(added.outcome, api::AddOutcome::Added);
        let again = svc
            .add_document(api::AddDocument {
                text: "Le moteur de recherche indexe les fichiers locaux.".into(),
                title: Some("Moteur".into()),
                url: None,
                format: None,
            })
            .unwrap();
        assert_eq!(again.outcome, api::AddOutcome::Unchanged);

        let mut events = Vec::new();
        let r = svc
            .search(
                &api::SearchRequest {
                    query: "moteur fichiers".into(),
                    limit: 10,
                },
                |e| events.push(e),
            )
            .unwrap();
        assert_eq!(r.hits.len(), 1);
        assert_eq!(r.hits[0].relative, 1.0);
        let phases: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                api::SearchEvent::Phase { phase } => Some(*phase),
                _ => None,
            })
            .collect();
        assert_eq!(phases, api::Phase::ALL);
        assert!(matches!(events.last(), Some(api::SearchEvent::Done(_))));
        let kinds: Vec<_> = r.graph.nodes.iter().map(|n| n.kind).collect();
        assert!(kinds.contains(&api::NodeKind::Query));
        assert!(kinds.contains(&api::NodeKind::Prose));
        assert!(kinds.contains(&api::NodeKind::Source));
        assert!(
            r.graph
                .edges
                .iter()
                .any(|e| e.kind == api::EdgeKind::TermHit && e.weight > 0.0)
        );

        let doc = svc.document(added.id).unwrap().unwrap();
        assert_eq!(doc.title.as_deref(), Some("Moteur"));
        assert_eq!(doc.passages.len(), 1);
    }

    #[test]
    fn sources_are_saved_and_removed() {
        let (dir, svc) = service();
        let folder = dir.path().join("notes");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("a.md"), "# Titre\n\nDu texte sur les graphes.").unwrap();
        let spec = folder.to_string_lossy().into_owned();
        let view = svc.add_source(&spec).unwrap();
        assert!(svc.add_source("nimporte:quoi").is_err());
        let stats = svc.collect(&spec).unwrap();
        assert_eq!(stats.added, 1);
        assert_eq!(
            svc.collect(&spec).unwrap().added,
            0,
            "collecte incrémentale"
        );
        assert!(svc.sources()[0].last_run.is_some());
        let saved = Config::load(&dir.path().join("config.toml")).unwrap();
        assert_eq!(saved.sources, vec![spec]);
        assert!(svc.remove_source(&view.id).unwrap());
        assert!(svc.sources().is_empty());
    }
}
