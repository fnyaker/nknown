//! Façade du cœur de NKnown, partagée par le démon local, le serveur et l'app native.
//!
//! Un [`Service`] possède le pipeline d'analyse et la base. Il est `Sync` : les recherches se
//! font en parallèle (verrou en lecture) et la collecte ne prend le verrou en écriture que le
//! temps d'enregistrer un document, l'analyse se faisant hors verrou.

pub mod catalog;
pub mod config;
pub mod fetch;
pub mod graph;
pub mod models;
pub mod query;
pub mod sources;
pub mod translation;
pub mod watch;

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use nknown_api as api;
use nknown_core::analyze::Analyzer;
use nknown_core::item::RawItem;
use nknown_core::lang::Lang;
use nknown_core::nature::Nature;
use nknown_core::source::{SourceError, SourceId, SourceKind};
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

/// Apprentissage des liens entre mots : base minimale, et croissance qui le relance.
const LEARN_MIN_PASSAGES: u64 = 2_000;
const LEARN_GROWTH: f64 = 0.2;

pub struct Service {
    /// Rechargé quand les modèles arrivent (téléchargement du premier lancement).
    pipeline: RwLock<Arc<Pipeline>>,
    detector: LangDetector,
    engine: RwLock<Engine<RedbStorage>>,
    data_dir: PathBuf,
    config: Mutex<Config>,
    config_path: Option<PathBuf>,
    /// État de collecte par `spec`.
    states: Mutex<HashMap<String, api::SourceState>>,
    collecting: AtomicUsize,
    /// Analyseur chargé : 0 aucun, 1 CPU, 2 GPU.
    parser: AtomicU8,
    models: Mutex<api::ModelsState>,
    translators: translation::Translators,
    /// Traduction en cours des documents déjà indexés d'une source.
    translating: Mutex<Option<api::TranslationProgress>>,
    /// Langues des documents par source (nombre de documents de la base, quand, décompte).
    source_langs: Mutex<Option<(u64, Instant, Arc<SourceLangs>)>>,
}

/// Langues des documents (hors copies traduites), par identifiant de source.
type SourceLangs = HashMap<String, Vec<Lang>>;

/// Décompte des langues par source : recalculé au plus aussi souvent (la base entière est lue).
const SOURCE_LANGS_EVERY: Duration = Duration::from_secs(60);

const PARSER_NONE: u8 = 0;
const PARSER_CPU: u8 = 1;
const PARSER_GPU: u8 = 2;

/// Fils de calcul du processus (rayon, utilisé par l'analyseur sur CPU) : nombre limité, priorité
/// basse. Réglé une fois, avant le premier calcul.
fn tune_compute(threads: usize) {
    static DONE: std::sync::Once = std::sync::Once::new();
    DONE.call_once(|| {
        let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
        let n = if threads == 0 {
            (cores / 4).max(2)
        } else {
            threads
        };
        let _ = rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .thread_name(|i| format!("nknown-calcul-{i}"))
            .start_handler(|_| {
                // Priorité basse pour ce seul fil (setpriority sur l'identifiant du fil).
                let tid = rustix::thread::gettid();
                let _ = rustix::process::setpriority_process(Some(tid), 15);
            })
            .build_global();
    });
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
        tune_compute(opts.config.compute_threads);
        let (pipeline, parser) = load_models(&opts.config, &opts.data_dir)?;
        // Migration automatique à la mise à jour : base réécrite si elle a beaucoup de place
        // perdue, puis mise au format courant.
        let (storage, tidied) = RedbStorage::open_tidy(opts.data_dir.join("base.redb"), false)
            .map_err(|e| Error::Storage(e.to_string()))?;
        if let Some(note) = tidied {
            eprintln!("{note}");
        }
        let mut engine = Engine::new(storage).with_stemmer(nknown_nlp::langid::stem);
        if let Some(note) = engine.migrate()? {
            eprintln!("base mise à jour : {note}");
        }
        Ok(Self {
            detector: LangDetector::new(),
            engine: RwLock::new(engine),
            data_dir: opts.data_dir,
            config: Mutex::new(opts.config),
            config_path: opts.config_path,
            states: Mutex::new(HashMap::new()),
            collecting: AtomicUsize::new(0),
            parser: AtomicU8::new(parser),
            models: Mutex::new(api::ModelsState::Missing),
            translators: translation::Translators::default(),
            translating: Mutex::new(None),
            source_langs: Mutex::new(None),
            pipeline: RwLock::new(Arc::new(pipeline)),
        })
    }

    fn pipeline(&self) -> Arc<Pipeline> {
        self.pipeline
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Télécharge les modèles manquants (premier lancement), puis les recharge. Sans effet
    /// s'ils sont déjà là. La progression se lit dans [`Self::status`].
    pub fn ensure_models(&self) -> Result<(), Error> {
        let config = self.config();
        let dir = config.models_dir(&self.data_dir);
        let manifest = fetch::manifest();
        let mut missing = fetch::missing(&manifest, &dir, &fetch::groups(&config));
        missing.extend(fetch::missing_pairs(
            &manifest,
            &dir,
            &fetch::wanted_pairs(&config),
        ));
        if !missing.is_empty() {
            let total = missing.iter().map(|f| f.size).sum();
            *lock(&self.models) = api::ModelsState::Downloading { done: 0, total };
            eprintln!(
                "téléchargement des modèles : {} fichiers, {} Mo",
                missing.len(),
                total / 1_000_000
            );
            let fetched = fetch::fetch(&manifest, &dir, &missing, &mut |done, total| {
                *lock(&self.models) = api::ModelsState::Downloading { done, total };
            });
            if let Err(e) = fetched {
                // Ce qui est arrivé (les lexiques, souvent) sert déjà aux requêtes.
                if let Err(e) = self.reload_models() {
                    eprintln!("modèles : {e}");
                }
                *lock(&self.models) = api::ModelsState::Failed {
                    message: e.to_string(),
                };
                return Err(e);
            }
            self.reload_models()?;
            eprintln!("modèles prêts");
        } else if !matches!(*lock(&self.models), api::ModelsState::Ready) {
            self.reload_models()?;
        }
        *lock(&self.models) = api::ModelsState::Ready;
        Ok(())
    }

    /// Recharge lexiques, tagueurs et analyseur selon la configuration.
    pub fn reload_models(&self) -> Result<(), Error> {
        let (pipeline, parser) = load_models(&self.config(), &self.data_dir)?;
        *self
            .pipeline
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(pipeline);
        self.parser.store(parser, Ordering::Relaxed);
        Ok(())
    }

    /// Catalogue de sources proposées à l'import (réseau au plus une fois par jour).
    pub fn catalog(&self) -> api::Catalog {
        catalog::load(&self.data_dir, &self.config().sources)
    }

    /// Réglages d'analyse en vigueur.
    pub fn settings(&self) -> api::EngineSettings {
        let c = self.config();
        api::EngineSettings {
            parser: c.parser.as_deref().is_some_and(|p| !p.trim().is_empty()),
            precision: match c.parser_precision {
                config::ParserPrecision::Standard => api::ParserPrecision::Standard,
                config::ParserPrecision::Light => api::ParserPrecision::Light,
            },
            device: c.parser_device,
        }
    }

    /// Change les réglages d'analyse, les enregistre et recharge les modèles (téléchargés au
    /// besoin : la précision `light` a ses propres poids).
    pub fn update_settings(&self, s: api::EngineSettings) -> Result<api::EngineSettings, Error> {
        {
            let mut c = lock(&self.config);
            let custom = c
                .parser
                .as_deref()
                .filter(|p| !p.trim().is_empty() && *p != config::DEFAULT_PARSER)
                .map(str::to_owned);
            c.parser = Some(if s.parser {
                custom.unwrap_or_else(|| config::DEFAULT_PARSER.into())
            } else {
                String::new()
            });
            c.parser_precision = match s.precision {
                api::ParserPrecision::Standard => config::ParserPrecision::Standard,
                api::ParserPrecision::Light => config::ParserPrecision::Light,
            };
            c.parser_device = s.device;
            self.save_config(&c)?;
        }
        self.ensure_models()?;
        self.reload_models()?;
        Ok(self.settings())
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
                .pipeline()
                .lexicon_langs()
                .iter()
                .map(|l| l.to_string())
                .collect(),
            parser: self.parser.load(Ordering::Relaxed) != PARSER_NONE,
            parser_device: match self.parser.load(Ordering::Relaxed) {
                PARSER_GPU => Some(api::ComputeDevice::Gpu),
                PARSER_CPU => Some(api::ComputeDevice::Cpu),
                _ => None,
            },
            collecting: self.collecting.load(Ordering::Relaxed) > 0,
            models: lock(&self.models).clone(),
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
        let pipeline = self.pipeline();
        let q = {
            let engine = self.engine();
            let mut q = query::build_query_in(&pipeline, Some(&self.detector), &req.query, &engine);
            query::add_learned(&pipeline, &engine, &mut q);
            q
        };
        let terms: Vec<api::QueryTermView> = q
            .terms
            .iter()
            .map(|t| api::QueryTermView {
                text: t.text.clone(),
                code: query::is_code_term(t),
            })
            .collect();
        let intent = query::modifiers_label(&q);
        on_event(api::SearchEvent::Terms {
            intent: intent.clone(),
            terms: terms.clone(),
        });

        // Carte en direct : d'abord la requête et ses mots.
        let mut shown = api::Graph::default();
        let mut show = |next: api::Graph, on_event: &mut dyn FnMut(api::SearchEvent)| {
            let patch = graph::diff(&shown, &next);
            if !patch.is_empty() {
                on_event(api::SearchEvent::GraphPatch(patch));
            }
            shown = next;
        };
        show(graph::build(&terms, &[], &[]), &mut on_event);

        on_event(api::SearchEvent::Phase {
            phase: api::Phase::Search,
        });
        let term_index = |text: &str| q.terms.iter().position(|t| t.text == text);
        let mut provisional = None;
        let result = self.engine().search_observed(
            &q,
            Timestamp::now(),
            req.limit.clamp(1, 100),
            &mut |stage| match stage {
                nknown_engine::Stage::Candidates { hits, .. } => {
                    // Résultats provisoires : la carte s'étoffe avant les scores définitifs.
                    let (hits, links) = convert_hits(hits.to_vec(), &term_index, false);
                    let mut g = graph::build(&terms, &hits, &links);
                    for n in &mut g.nodes {
                        n.hit = None;
                    }
                    provisional = Some(g);
                }
            },
        )?;
        if let Some(g) = provisional {
            show(g, &mut on_event);
        }

        on_event(api::SearchEvent::Phase {
            phase: api::Phase::Graph,
        });
        let (hits, links) = convert_hits(result.hits, &term_index, true);
        let graph = graph::build(&terms, &hits, &links);
        show(graph.clone(), &mut on_event);
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
            path: sources::local_path(doc.external_id.as_deref(), &doc.source),
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
        let (doc, outcome) = self.ingest_item(&item, SourceKind::Other, false)?;
        Ok(api::Added { id: doc.0, outcome })
    }

    /// Analyse hors verrou, puis enregistre.
    /// Analyse et enregistre `item` ; avec `translate`, en indexe aussi une copie traduite en
    /// français s'il est dans une autre langue (voir [`translation`]).
    fn ingest_item(
        &self,
        item: &RawItem,
        kind: SourceKind,
        translate: bool,
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
        let pipeline = self.pipeline();
        // Gros volumes (paliers de Wikipédia) : sans l'analyseur neuronal.
        let light = item
            .meta
            .iter()
            .any(|(k, v)| k == "analysis" && v == "light");
        let analysis = nknown_nlp::document::analyze_document_with(
            &pipeline,
            lang,
            &item.text,
            &format,
            Some(&self.detector),
            light,
        );
        let lang = query::main_lang(&analysis).unwrap_or(lang);
        let title = item.title.as_deref().map(|t| pipeline.analyze(lang, t));
        let ingested = self.engine_mut().ingest(Document {
            item,
            kind,
            lang,
            analysis: &analysis,
            title: title.as_ref(),
        })?;
        if translate && let Err(e) = self.translate_copy(item, kind, lang) {
            eprintln!("traduction : {e}");
        }
        Ok(match ingested {
            Ingested::Added { doc, .. } => (doc, api::AddOutcome::Added),
            Ingested::Updated { doc, .. } => (doc, api::AddOutcome::Updated),
            Ingested::Duplicate(doc) => (doc, api::AddOutcome::Unchanged),
        })
    }

    // ------------------------------------------------------------ traduction

    /// Indexe la copie traduite en français de `item` (langue `lang`), si elle manque ou si
    /// l'original a changé. Sans modèle pour cette langue, rien.
    fn translate_copy(&self, item: &RawItem, kind: SourceKind, lang: Lang) -> Result<(), Error> {
        if lang == translation::TARGET || translation::chain(lang).is_none() {
            return Ok(());
        }
        let config = self.config();
        let dir = config.models_dir(&self.data_dir);
        if !translation::models_present(&dir, lang) {
            return Ok(());
        }
        let original = item
            .external_id
            .clone()
            .or_else(|| item.url.clone())
            .unwrap_or_else(|| format!("text:{}", sources::id_of(&item.text)));
        if translation::translated_from(&original).is_some() {
            return Ok(());
        }
        let copy_id = translation::copy_id(&original, lang);
        let mut copy = item.clone();
        copy.external_id = Some(copy_id);
        copy.lang_hint = Some(translation::TARGET);
        // Inchangée depuis la dernière traduction : l'original n'a pas bougé non plus.
        let text = self
            .translators
            .translate(&dir, config.translate_device, lang, &item.text)?;
        copy.text = text;
        copy.title = match &item.title {
            Some(t) => Some(
                self.translators
                    .translate(&dir, config.translate_device, lang, t)?,
            ),
            None => None,
        };
        copy.meta.retain(|(k, _)| k != "format");
        copy.meta
            .push(("translated_from".into(), lang.as_str().to_owned()));
        self.ingest_item(&copy, kind, false)?;
        Ok(())
    }

    /// Identifiant de source (`SourceId`) de chaque spécification configurée.
    fn source_ids(&self, specs: &[String]) -> Vec<(String, String)> {
        specs
            .iter()
            .filter_map(|spec| {
                let s = sources::open_in(spec, &self.data_dir).ok()?;
                Some((spec.clone(), s.id().0.clone()))
            })
            .collect()
    }

    /// Traduction : sources proposées ou décidées, périphérique, travail en cours.
    pub fn translation(&self) -> Result<api::TranslationView, Error> {
        let config = self.config();
        let dir = config.models_dir(&self.data_dir);
        let langs = self.source_langs()?;
        let mut out = Vec::new();
        for (spec, id) in self.source_ids(&config.sources) {
            let state = if config.translate.contains(&spec) {
                api::TranslationState::Accepted
            } else if config.translate_declined.contains(&spec) {
                api::TranslationState::Declined
            } else {
                api::TranslationState::Suggested
            };
            let found = langs.get(&id).map(Vec::as_slice).unwrap_or_default();
            let Some(sugg) = translation::suggest(found) else {
                continue;
            };
            out.push(api::TranslationSource {
                spec,
                lang: sugg.lang.as_str().to_owned(),
                docs: sugg.docs,
                share: sugg.share,
                state,
                model_ready: translation::models_present(&dir, sugg.lang),
            });
        }
        Ok(api::TranslationView {
            available: cfg!(feature = "neural"),
            device: config.translate_device,
            sources: out,
            busy: lock(&self.translating).clone(),
        })
    }

    /// Langues des documents par source, en cache (voir [`SOURCE_LANGS_EVERY`]).
    fn source_langs(&self) -> Result<Arc<SourceLangs>, Error> {
        let docs = self.engine().stats()?.docs;
        if let Some((n, at, langs)) = lock(&self.source_langs).as_ref()
            && (*n == docs || at.elapsed() < SOURCE_LANGS_EVERY)
        {
            return Ok(langs.clone());
        }
        let mut langs: SourceLangs = HashMap::new();
        for (_, d) in self.engine().documents()? {
            let copy = d
                .external_id
                .as_deref()
                .is_some_and(|e| translation::translated_from(e).is_some());
            if copy {
                continue;
            }
            if let Some(l) = Lang::from_code(&d.lang) {
                langs.entry(d.source).or_default().push(l);
            }
        }
        let langs = Arc::new(langs);
        *lock(&self.source_langs) = Some((docs, Instant::now(), langs.clone()));
        Ok(langs)
    }

    /// Réponse à une proposition de traduction, ou changement de périphérique. Acceptée : les
    /// modèles de la langue sont téléchargés, puis les documents déjà indexés traduits en
    /// tâche de fond.
    pub fn answer_translation(
        self: &Arc<Self>,
        req: api::TranslationRequest,
    ) -> Result<api::TranslationView, Error> {
        if let Some(device) = req.device {
            let mut c = lock(&self.config);
            if c.translate_device != device {
                c.translate_device = device;
                self.save_config(&c)?;
                self.translators.clear();
            }
        }
        if let (Some(spec), Some(accept)) = (req.spec, req.accept) {
            let view = self.translation()?;
            let lang = view
                .sources
                .iter()
                .find(|s| s.spec == spec)
                .map(|s| s.lang.clone());
            {
                let mut c = lock(&self.config);
                c.translate.retain(|s| *s != spec);
                c.translate_declined.retain(|s| *s != spec);
                if accept {
                    c.translate.push(spec.clone());
                    if let Some(l) = &lang
                        && !c.translate_langs.contains(l)
                    {
                        c.translate_langs.push(l.clone());
                    }
                } else {
                    c.translate_declined.push(spec.clone());
                }
                self.save_config(&c)?;
            }
            if accept {
                let svc = self.clone();
                std::thread::spawn(move || {
                    if let Err(e) = svc.ensure_models() {
                        eprintln!("traduction : modèles : {e}");
                        return;
                    }
                    if let Err(e) = svc.translate_existing(&spec) {
                        eprintln!("traduction de {spec} : {e}");
                    }
                    *lock(&svc.translating) = None;
                });
            }
        }
        self.translation()
    }

    /// Traduit les documents déjà indexés de la source `spec` qui n'ont pas encore de copie.
    fn translate_existing(&self, spec: &str) -> Result<(), Error> {
        let source = sources::open_in(spec, &self.data_dir)?;
        let (id, kind) = (source.id().clone(), source.kind());
        let docs: Vec<_> = self
            .engine()
            .documents()?
            .into_iter()
            .filter(|(_, d)| d.source == id.0)
            .filter(|(_, d)| {
                Lang::from_code(&d.lang)
                    .is_some_and(|l| l != translation::TARGET && translation::chain(l).is_some())
                    && d.external_id
                        .as_deref()
                        .is_none_or(|e| translation::translated_from(e).is_none())
            })
            .collect();
        let total = docs.len();
        for (done, (_, d)) in docs.into_iter().enumerate() {
            *lock(&self.translating) = Some(api::TranslationProgress {
                spec: spec.to_owned(),
                done,
                total,
            });
            // La source a été retirée de la traduction entre-temps : on s'arrête.
            if !self.config().translate.iter().any(|s| s == spec) {
                break;
            }
            let Some(lang) = Lang::from_code(&d.lang) else {
                continue;
            };
            let Some(ext) = d.external_id.clone() else {
                continue;
            };
            if self
                .engine()
                .known(&id.0, &translation::copy_id(&ext, lang))?
                .is_some()
            {
                continue;
            }
            let text = self.engine().doc_text(&d)?;
            let mut item = RawItem::new(id.clone(), text, Timestamp(d.fetched_at));
            item.external_id = Some(ext);
            item.url = d.url.clone();
            item.title = d.title.clone();
            item.published_at = Some(nknown_core::time::Dated {
                at: Timestamp(d.date),
                confidence: match d.date_confidence {
                    0 => nknown_core::time::DateConfidence::Declared,
                    1 => nknown_core::time::DateConfidence::Inferred,
                    _ => nknown_core::time::DateConfidence::FetchTimeOnly,
                },
            });
            if let Err(e) = self.translate_copy(&item, kind, lang) {
                eprintln!("traduction : {e}");
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------ sources

    pub fn sources(&self) -> Vec<api::SourceView> {
        let (specs, paused) = {
            let c = lock(&self.config);
            (c.sources.clone(), c.paused.clone())
        };
        let states = lock(&self.states);
        specs
            .into_iter()
            .map(|spec| {
                let last_run = sources::open_in(&spec, &self.data_dir)
                    .ok()
                    .and_then(|s| self.engine().checkpoint(&s.checkpoint_key()).ok().flatten())
                    .map(|t| t.0);
                api::SourceView {
                    id: sources::id_of(&spec),
                    kind: sources::kind_of(&spec),
                    state: states.get(&spec).cloned().unwrap_or(api::SourceState::Idle),
                    last_run,
                    paused: paused.contains(&spec),
                    spec,
                }
            })
            .collect()
    }

    /// Ajoute une source à la configuration (sans la collecter : voir [`Self::collect`]).
    pub fn add_source(&self, spec: &str) -> Result<api::SourceView, Error> {
        let spec = spec.trim();
        sources::open_in(spec, &self.data_dir)?;
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

    // ------------------------------------------------------------ flux d'info

    /// Les flux d'info et leurs derniers contenus.
    pub fn feeds(&self) -> Result<Vec<api::FeedView>, Error> {
        let feeds = self.config().feeds;
        feeds.iter().map(|f| self.feed_view(f)).collect()
    }

    /// Contenus d'un flux : chaque mot-clé est cherché (avec priorité au récent), les résultats
    /// sont fusionnés par document puis triés du plus récent au plus ancien.
    fn feed_view(&self, feed: &config::Feed) -> Result<api::FeedView, Error> {
        let mut by_doc: std::collections::HashMap<u64, api::Hit> = Default::default();
        for kw in feed.keywords.iter().filter(|k| !k.trim().is_empty()) {
            let pipeline = self.pipeline();
            let mut q = {
                let engine = self.engine();
                let mut q = query::build_query_in(&pipeline, Some(&self.detector), kw, &engine);
                query::add_learned(&pipeline, &engine, &mut q);
                q
            };
            if q.terms.is_empty() {
                continue;
            }
            q.recency = Some(nknown_engine::RECENT_HALF_LIFE_DAYS);
            let found = self
                .engine()
                .search(&q, Timestamp::now(), FEED_PER_KEYWORD)?;
            let term_index = |text: &str| q.terms.iter().position(|t| t.text == text);
            for h in convert_hits(found, &term_index, true).0 {
                let keep = by_doc.get(&h.doc).is_none_or(|old| old.score < h.score);
                if keep {
                    by_doc.insert(h.doc, h);
                }
            }
        }
        let mut items: Vec<api::Hit> = by_doc.into_values().collect();
        items.sort_by(|a, b| b.date.cmp(&a.date).then(b.score.total_cmp(&a.score)));
        items.truncate(FEED_ITEMS);
        Ok(api::FeedView {
            id: feed.id.clone(),
            title: feed.title.clone(),
            keywords: feed.keywords.clone(),
            fresh: items.iter().filter(|h| h.date > feed.seen).count(),
            items,
        })
    }

    fn feed_from(req: api::FeedRequest) -> Result<(String, Vec<String>), Error> {
        let keywords: Vec<String> = req
            .keywords
            .iter()
            .map(|k| k.trim().to_owned())
            .filter(|k| !k.is_empty())
            .collect();
        if keywords.is_empty() {
            return Err(Error::Invalid("au moins un mot-clé".into()));
        }
        let title = match req.title.trim() {
            "" => keywords.join(", "),
            t => t.to_owned(),
        };
        Ok((title, keywords))
    }

    pub fn add_feed(&self, req: api::FeedRequest) -> Result<api::FeedView, Error> {
        let (title, keywords) = Self::feed_from(req)?;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let feed = config::Feed {
            id: sources::id_of(&format!("{title}\u{1f}{nanos}")),
            title,
            keywords,
            seen: Timestamp::now().0,
        };
        {
            let mut c = lock(&self.config);
            c.feeds.push(feed.clone());
            self.save_config(&c)?;
        }
        self.feed_view(&feed)
    }

    pub fn update_feed(
        &self,
        id: &str,
        req: api::FeedRequest,
    ) -> Result<Option<api::FeedView>, Error> {
        let (title, keywords) = Self::feed_from(req)?;
        let feed = {
            let mut c = lock(&self.config);
            let Some(f) = c.feeds.iter_mut().find(|f| f.id == id) else {
                return Ok(None);
            };
            f.title = title;
            f.keywords = keywords;
            let feed = f.clone();
            self.save_config(&c)?;
            feed
        };
        self.feed_view(&feed).map(Some)
    }

    pub fn remove_feed(&self, id: &str) -> Result<bool, Error> {
        let mut c = lock(&self.config);
        let before = c.feeds.len();
        c.feeds.retain(|f| f.id != id);
        let removed = c.feeds.len() != before;
        if removed {
            self.save_config(&c)?;
        }
        Ok(removed)
    }

    /// Le flux vient d'être consulté : ses contenus ne sont plus « nouveaux ».
    pub fn mark_feed_seen(&self, id: &str) -> Result<bool, Error> {
        let mut c = lock(&self.config);
        let Some(f) = c.feeds.iter_mut().find(|f| f.id == id) else {
            return Ok(false);
        };
        f.seen = Timestamp::now().0;
        self.save_config(&c)?;
        Ok(true)
    }

    /// Met une source en pause ou la reprend ; `None` si elle est inconnue.
    pub fn set_paused(&self, id: &str, paused: bool) -> Result<Option<api::SourceView>, Error> {
        {
            let mut c = lock(&self.config);
            let Some(spec) = c.sources.iter().find(|s| sources::id_of(s) == id).cloned() else {
                return Ok(None);
            };
            c.paused.retain(|p| *p != spec);
            if paused {
                c.paused.push(spec);
            }
            self.save_config(&c)?;
        }
        Ok(self.sources().into_iter().find(|s| s.id == id))
    }

    /// Ordre de grandeur d'un import de Wikipédia (réseau : taille du dump, nombre d'articles).
    pub fn wikipedia_estimate(
        &self,
        req: &api::WikiEstimateRequest,
    ) -> Result<api::WikiEstimate, Error> {
        if req.edition.is_empty()
            || !req
                .edition
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b == b'-')
        {
            return Err(Error::Invalid(format!(
                "édition invalide : {}",
                req.edition
            )));
        }
        Ok(match req.tier {
            api::WikiTier::TopRead { limit } => api::WikiEstimate {
                articles: limit,
                download_bytes: limit * WIKI_API_BYTES,
                disk_bytes: limit * WIKI_DISK_BYTES,
                // Cinq articles par seconde au plus côté API ; l'indexation suit.
                seconds: limit / 5 + limit / WIKI_INDEX_RATE,
            },
            api::WikiTier::Dump { limit } => {
                let http = nknown_sources::http::Http::new();
                let (size, total) = nknown_sources::wikidump::estimate(&http, &req.edition)
                    .map_err(|e| Error::Invalid(e.to_string()))?;
                let total = total.max(1);
                let articles = limit.map_or(total, |n| n.min(total));
                // Le dump contient aussi les redirections et autres pages : au prorata.
                let download = (size as f64 * articles as f64 / total as f64) as u64 + size / 40;
                api::WikiEstimate {
                    articles,
                    download_bytes: download,
                    disk_bytes: articles * WIKI_DISK_BYTES,
                    seconds: download / WIKI_DOWNLOAD_RATE + articles / WIKI_INDEX_RATE,
                }
            }
        })
    }

    /// Retire une source de la configuration. Ses documents restent dans la base.
    pub fn remove_source(&self, id: &str) -> Result<bool, Error> {
        let mut config = lock(&self.config);
        let before = config.sources.len();
        config.sources.retain(|s| sources::id_of(s) != id);
        config.paused.retain(|s| sources::id_of(s) != id);
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
        let specs: Vec<String> = {
            let c = lock(&self.config);
            c.sources
                .iter()
                .filter(|s| !c.paused.contains(s))
                .cloned()
                .collect()
        };
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
        self.learn_if_due();
        out
    }

    /// Réapprend les liens entre mots si la base a assez grossi depuis la dernière fois
    /// (voir `nknown_engine::learn`). Le calcul se fait sous verrou de lecture : les
    /// recherches continuent pendant ce temps.
    pub fn learn_if_due(&self) {
        let due = self
            .engine()
            .learning_due(LEARN_MIN_PASSAGES, LEARN_GROWTH)
            .unwrap_or(false);
        if !due {
            return;
        }
        let start = Instant::now();
        let learned = self
            .engine()
            .compute_learned(nknown_engine::LearnOptions::default());
        let stored = learned.and_then(|l| self.engine_mut().store_learned(l));
        match stored {
            Ok(s) => eprintln!(
                "liens appris : {} syntaxiques, {} thématiques ({} mots, {} documents) [{:.1?}]",
                s.syntactic,
                s.links,
                s.vocab,
                s.docs,
                start.elapsed()
            ),
            Err(e) => eprintln!("liens appris : {e}"),
        }
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
        let mut source = sources::open_in(spec, &self.data_dir)?;
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
        let translate = lock(&self.config).translate.iter().any(|s| s == spec);
        for (i, item) in items.enumerate() {
            // Mise en pause pendant la collecte : on s'arrête là (un dump reprendra au bloc
            // suivant).
            if lock(&self.config).paused.iter().any(|p| p == spec) {
                return Ok(stats);
            }
            match item {
                Ok(item) => match self.ingest_item(&item, kind, translate)?.1 {
                    api::AddOutcome::Added => stats.added += 1,
                    api::AddOutcome::Updated => stats.updated += 1,
                    api::AddOutcome::Unchanged => stats.unchanged += 1,
                },
                // Protection anti-robot : on s'arrête là (les pages déjà lues restent).
                Err(e @ SourceError::Blocked(_)) => return Err(Error::Invalid(e.to_string())),
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

/// Ordres de grandeur des imports de Wikipédia : réponse de l'API par article, place dans la
/// base par article, articles indexés par seconde (analyse légère), débit des dumps (o/s).
const WIKI_API_BYTES: u64 = 30_000;
const WIKI_DISK_BYTES: u64 = 20_000;
const WIKI_INDEX_RATE: u64 = 25;
const WIKI_DOWNLOAD_RATE: u64 = 4_000_000;

/// Contenus cherchés par mot-clé d'un flux, et gardés par flux.
const FEED_PER_KEYWORD: usize = 20;
const FEED_ITEMS: usize = 12;

/// Résultats du moteur → résultats de l'API et entrées du graphe. Sans `with_snippets`
/// (carte provisoire), pas d'extrait : seule la carte s'en sert.
fn convert_hits(
    found: Vec<nknown_engine::Hit>,
    term_index: &dyn Fn(&str) -> Option<usize>,
    with_snippets: bool,
) -> (Vec<api::Hit>, Vec<graph::HitLinks>) {
    let best = found
        .first()
        .map_or(1.0, |h| h.score.max(f64::MIN_POSITIVE));
    let mut hits = Vec::with_capacity(found.len());
    let mut links = Vec::with_capacity(found.len());
    for h in found {
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
        // Copie traduite : titre, lien et chemin de l'original.
        let ext = h.doc.external_id.as_deref();
        let original = ext.map(translation::original_id);
        let translated_from = ext
            .and_then(translation::translated_from)
            .map(|l| l.as_str().to_owned());
        hits.push(api::Hit {
            doc: h.doc_id.0,
            passage: h.passage.0,
            title: sources::display_title(h.doc.title.as_deref(), original, &h.doc.source),
            source: sources::label(&h.doc.source),
            source_kind: sources::api_kind(h.doc.kind),
            url: sources::open_url(h.doc.url.as_deref(), original, &h.doc.source),
            path: sources::local_path(original, &h.doc.source),
            translated_from,
            date: h.doc.date,
            score: h.score,
            relative: (h.score / best) as f32,
            snippet: if with_snippets {
                query::snippet(&h.text, &h.matched, 240)
            } else {
                String::new()
            },
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
    (hits, links)
}

/// Lexiques et tagueurs du dossier des modèles, et l'analyseur configuré s'il est là (sinon,
/// en attendant son téléchargement, on s'en passe). L'analyseur va sur le GPU (Vulkan) si le
/// réglage le permet et qu'un GPU répond, sinon sur le CPU.
fn load_models(config: &Config, data_dir: &Path) -> Result<(Pipeline, u8), Error> {
    let dir = config.models_dir(data_dir);
    let pipeline = if dir.is_dir() {
        models::load_pipeline(&dir, &[])?
    } else {
        Pipeline::default()
    };
    let Some(spec) = config.parser.as_deref().filter(|s| !s.trim().is_empty()) else {
        return Ok((pipeline, PARSER_NONE));
    };
    let ready = if spec == config::DEFAULT_PARSER {
        let groups = fetch::groups(config);
        fetch::missing(&fetch::manifest(), &dir, &groups[1..]).is_empty()
    } else {
        let resolved = models::resolve(spec, &dir);
        let path = resolved
            .split_once(':')
            .map_or(resolved.as_str(), |(_, p)| p);
        Path::new(path).exists()
    };
    if !ready {
        return Ok((pipeline, PARSER_NONE));
    }
    let resolved = models::resolve(spec, &dir);
    let explicit_gpu = resolved.starts_with("vulkan:");
    let want_gpu = explicit_gpu
        || match config.parser_device {
            api::DeviceChoice::Cpu => false,
            api::DeviceChoice::Gpu => true,
            api::DeviceChoice::Auto => models::gpu_available(),
        };
    if want_gpu {
        let gpu_spec = if explicit_gpu {
            resolved.clone()
        } else {
            format!("vulkan:{resolved}")
        };
        match models::load_parser(&gpu_spec) {
            Ok((parser, langs)) => {
                return Ok((models::share_parser(pipeline, langs, parser), PARSER_GPU));
            }
            // Pas de GPU utilisable : le CPU prend le relais.
            Err(e) => eprintln!("analyseur sur GPU indisponible ({e}) : CPU"),
        }
    }
    let spec = config.parser_precision.apply(&resolved);
    let (parser, langs) =
        models::load_parser(&spec).map_err(|e| Error::Model(format!("{spec} : {e}")))?;
    Ok((models::share_parser(pipeline, langs, parser), PARSER_CPU))
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
    fn paused_sources_are_skipped_and_wiki_tiers_parse() {
        let (dir, svc) = service();
        let folder = dir.path().join("notes");
        std::fs::create_dir_all(&folder).unwrap();
        let view = svc.add_source(folder.to_str().unwrap()).unwrap();
        assert!(!view.paused);
        let paused = svc.set_paused(&view.id, true).unwrap().unwrap();
        assert!(paused.paused);
        assert!(svc.collect_all().is_empty());
        assert!(!svc.set_paused(&view.id, false).unwrap().unwrap().paused);
        assert_eq!(svc.collect_all().len(), 1);
        assert!(svc.set_paused("inconnue", true).unwrap().is_none());
        // Paliers de Wikipédia.
        for ok in ["wiki-top:fr:1000", "wiki-dump:en", "wiki-dump:ru:100000"] {
            assert!(sources::open_in(ok, dir.path()).is_ok(), "{ok}");
        }
        for bad in ["wiki-top:fr:beaucoup", "wiki-dump:", "wiki-dump:FR/../x"] {
            assert!(sources::open_in(bad, dir.path()).is_err(), "{bad}");
        }
    }

    #[test]
    fn foreign_sources_are_suggested_for_translation() {
        let (dir, svc) = service();
        let svc = Arc::new(svc);
        let folder = dir.path().join("english");
        std::fs::create_dir_all(&folder).unwrap();
        for (i, text) in [
            "The election results will be announced on Sunday evening by the commission.",
            "Researchers found that the new drug reduced symptoms in most of the patients.",
            "The city council voted to build a new bridge across the river next spring.",
            "Heavy rain is expected across the region during the whole of next week.",
        ]
        .iter()
        .enumerate()
        {
            std::fs::write(folder.join(format!("{i}.txt")), text).unwrap();
        }
        let spec = folder.to_str().unwrap().to_owned();
        svc.add_source(&spec).unwrap();
        svc.collect_all();
        let view = svc.translation().unwrap();
        let s = view
            .sources
            .iter()
            .find(|s| s.spec == spec)
            .expect("proposée");
        assert_eq!(s.lang, "eng");
        assert_eq!(s.state, api::TranslationState::Suggested);
        assert!(!s.model_ready);
        // Refus : retenu, plus proposé comme tel.
        let view = svc
            .answer_translation(api::TranslationRequest {
                spec: Some(spec.clone()),
                accept: Some(false),
                device: Some(api::DeviceChoice::Cpu),
            })
            .unwrap();
        let s = view.sources.iter().find(|s| s.spec == spec).unwrap();
        assert_eq!(s.state, api::TranslationState::Declined);
        assert_eq!(view.device, api::DeviceChoice::Cpu);
        assert!(svc.config().translate_declined.contains(&spec));
    }

    #[test]
    fn feeds_follow_keywords() {
        let (_dir, svc) = service();
        for (title, text) in [
            (
                "Vulkan",
                "Le pilote Vulkan accélère le calcul sur la carte graphique.",
            ),
            ("Jardin", "Les tomates du jardin mûrissent en août."),
        ] {
            svc.add_document(api::AddDocument {
                text: text.into(),
                title: Some(title.into()),
                url: None,
                format: None,
            })
            .unwrap();
        }
        assert!(
            svc.add_feed(api::FeedRequest {
                title: " ".into(),
                keywords: vec![" ".into()],
            })
            .is_err()
        );
        let feed = svc
            .add_feed(api::FeedRequest {
                title: "".into(),
                keywords: vec!["vulkan".into(), "introuvable".into()],
            })
            .unwrap();
        assert_eq!(feed.title, "vulkan, introuvable");
        assert_eq!(feed.items.len(), 1);
        assert_eq!(feed.items[0].title.as_deref(), Some("Vulkan"));
        let updated = svc
            .update_feed(
                &feed.id,
                api::FeedRequest {
                    title: "Potager".into(),
                    keywords: vec!["tomates".into()],
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(updated.items[0].title.as_deref(), Some("Jardin"));
        assert!(svc.mark_feed_seen(&feed.id).unwrap());
        assert_eq!(svc.feeds().unwrap()[0].fresh, 0);
        // Enregistrés dans la configuration.
        assert_eq!(svc.config().feeds[0].title, "Potager");
        assert!(svc.remove_feed(&feed.id).unwrap());
        assert!(svc.feeds().unwrap().is_empty());
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

    /// Les patchs du flux, appliqués dans l'ordre, donnent la carte finale ; la carte se
    /// construit par étapes (mots d'abord, résultats ensuite).
    #[test]
    fn live_graph_patches_rebuild_the_final_graph() {
        let (_dir, svc) = service();
        for (title, text) in [
            (
                "Rust",
                "Le compilateur Rust vérifie les emprunts et la mémoire.",
            ),
            (
                "Mémoire",
                "La mémoire vive du compilateur sature sur les gros projets.",
            ),
            (
                "Cuisine",
                "Une recette de tarte aux pommes, sans compilateur.",
            ),
        ] {
            svc.add_document(api::AddDocument {
                text: text.into(),
                title: Some(title.into()),
                url: None,
                format: None,
            })
            .unwrap();
        }
        let mut events = Vec::new();
        let r = svc
            .search(
                &api::SearchRequest {
                    query: "compilateur mémoire".into(),
                    limit: 10,
                },
                |e| events.push(e),
            )
            .unwrap();
        let patches: Vec<&api::GraphPatch> = events
            .iter()
            .filter_map(|e| match e {
                api::SearchEvent::GraphPatch(p) => Some(p),
                _ => None,
            })
            .collect();
        assert!(patches.len() >= 2, "{} patchs", patches.len());
        // Premier patch : la requête et ses mots, sans résultat.
        assert!(
            patches[0]
                .nodes
                .iter()
                .all(|n| matches!(n.kind, api::NodeKind::Query | api::NodeKind::Term))
        );
        // Chaque patch arrive avant le résultat final.
        let done = events
            .iter()
            .position(|e| matches!(e, api::SearchEvent::Done(_)))
            .unwrap();
        assert!(
            events[..done]
                .iter()
                .filter(|e| matches!(e, api::SearchEvent::GraphPatch(_)))
                .count()
                == patches.len()
        );
        let mut live = api::Graph::default();
        for p in &patches {
            live.apply(p);
        }
        let sorted = |g: &api::Graph| {
            let mut nodes = g.nodes.clone();
            nodes.sort_by(|a, b| a.id.cmp(&b.id));
            let mut edges = g.edges.clone();
            edges.sort_by(|a, b| (&a.from, &a.to).cmp(&(&b.from, &b.to)));
            (nodes, edges)
        };
        assert_eq!(sorted(&live), sorted(&r.graph));
    }

    #[test]
    fn graph_diff_removes_dropped_nodes_and_edges() {
        let node = |id: &str, w: f32| api::Node {
            id: id.into(),
            kind: api::NodeKind::Prose,
            label: String::new(),
            weight: w,
            hit: None,
            doc: None,
            source_kind: None,
        };
        let edge = |from: &str, to: &str| api::Edge {
            from: from.into(),
            to: to.into(),
            kind: api::EdgeKind::TermHit,
            weight: 1.0,
        };
        let old = api::Graph {
            nodes: vec![node("a", 1.0), node("b", 1.0), node("c", 1.0)],
            edges: vec![edge("a", "b"), edge("a", "c"), edge("b", "c")],
        };
        let new = api::Graph {
            nodes: vec![node("a", 0.5), node("b", 1.0), node("d", 1.0)],
            edges: vec![edge("a", "d")],
        };
        let p = graph::diff(&old, &new);
        assert_eq!(p.nodes, vec![node("a", 0.5), node("d", 1.0)]);
        assert_eq!(p.removed_nodes, vec!["c".to_owned()]);
        // a → b disparaît ; les liens de c partent avec lui.
        assert_eq!(p.removed_edges, vec![edge("a", "b").key()]);
        let mut g = old.clone();
        g.apply(&p);
        g.nodes.sort_by(|a, b| a.id.cmp(&b.id));
        assert_eq!(g, new);
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
