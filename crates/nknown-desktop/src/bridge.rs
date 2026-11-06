//! Pont Rust ↔ QML : l'objet `Backend` lance les recherches et relaie leur progression.
//!
//! Les échanges avec le QML passent par du JSON (carte disposée, résultats, état) : le QML n'a
//! qu'à afficher et animer. Une recherche relancée avant la fin de la précédente l'éclipse : les
//! évènements d'une recherche dépassée sont ignorés.

// Le pont généré par cxx déclare des blocs `unsafe extern "C++"` : seul ce fichier y a droit.
#![allow(unsafe_code)]

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use std::time::Instant;

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use nknown_api::{self as api, Graph};

use crate::client::Engine;
use crate::settings::{Mode, Settings};
use crate::view;

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(QString, status)]
        #[qproperty(QString, automation)]
        #[qproperty(QString, request)]
        #[qproperty(QString, variety)]
        #[qproperty(bool, tray_available, cxx_name = "trayAvailable")]
        type Backend = super::BackendRust;

        /// Connexion au moteur et suivi de son état ; à appeler une fois l'interface prête.
        #[qinvokable]
        fn start(self: Pin<&mut Backend>);

        #[qinvokable]
        fn search(self: Pin<&mut Backend>, query: &QString);

        /// Ajoute une source : dossier, adresse de flux, page Wikipédia…
        #[qinvokable]
        #[cxx_name = "addSource"]
        fn add_source(self: Pin<&mut Backend>, spec: &QString);

        #[qinvokable]
        #[cxx_name = "removeSource"]
        fn remove_source(self: Pin<&mut Backend>, id: &QString);

        /// Change de moteur (`auto`, `daemon`, `embedded`, `remote`) ; pour un serveur distant,
        /// son adresse et, s'il est fourni, son jeton (rangé dans KWallet).
        #[qinvokable]
        #[cxx_name = "setEngine"]
        fn set_engine(self: Pin<&mut Backend>, mode: &QString, url: &QString, token: &QString);

        /// Démarrage du démon avec la session (service systemd utilisateur).
        /// Réglages d'analyse du moteur : analyseur, précision (`standard`, `light`), calcul
        /// (`auto`, `gpu`, `cpu`).
        #[qinvokable]
        #[cxx_name = "setAnalysis"]
        fn set_analysis(
            self: Pin<&mut Backend>,
            parser: bool,
            precision: &QString,
            device: &QString,
        );

        /// Charge un document pour le lecteur (identifiants en texte : document, passage à mettre
        /// en vue) ; réponse : `document`.
        #[qinvokable]
        #[cxx_name = "openDocument"]
        fn open_document(self: Pin<&mut Backend>, doc: &QString, passage: &QString);

        /// Montre un fichier dans le gestionnaire de fichiers (sélectionné si possible).
        #[qinvokable]
        #[cxx_name = "revealFile"]
        fn reveal_file(self: Pin<&mut Backend>, path: &QString);

        /// Flux d'info : relecture (réponse : `feeds`), création (`id` vide) ou modification,
        /// suppression, marque « vu ». Mots-clés séparés par des virgules.
        /// Plusieurs sources d'un coup (une par ligne), par exemple depuis le catalogue.
        #[qinvokable]
        #[cxx_name = "addSources"]
        fn add_sources(self: Pin<&mut Backend>, specs: &QString);

        #[qinvokable]
        #[cxx_name = "pauseSource"]
        fn pause_source(self: Pin<&mut Backend>, id: &QString, paused: bool);

        /// Statistiques du moteur (base, liens entre mots, mémoire) ; `word` non vide : liens
        /// de ce mot. Réponse : `stats`.
        #[qinvokable]
        #[cxx_name = "loadStats"]
        fn load_stats(self: Pin<&mut Backend>, word: &QString);

        /// Catalogue des sources proposées ; réponse : `catalog`.
        #[qinvokable]
        #[cxx_name = "loadCatalog"]
        fn load_catalog(self: Pin<&mut Backend>);

        /// Estimation d'un palier de Wikipédia (`tier` : `top` ou `dump`, `limit` 0 = tout) ;
        /// réponse : `estimate`.
        #[qinvokable]
        #[cxx_name = "estimateWikipedia"]
        fn estimate_wikipedia(
            self: Pin<&mut Backend>,
            edition: &QString,
            tier: &QString,
            limit: f64,
        );

        #[qinvokable]
        #[cxx_name = "refreshFeeds"]
        fn refresh_feeds(self: Pin<&mut Backend>);

        #[qinvokable]
        #[cxx_name = "saveFeed"]
        fn save_feed(self: Pin<&mut Backend>, id: &QString, title: &QString, keywords: &QString);

        #[qinvokable]
        #[cxx_name = "removeFeed"]
        fn remove_feed(self: Pin<&mut Backend>, id: &QString);

        #[qinvokable]
        #[cxx_name = "feedSeen"]
        fn feed_seen(self: Pin<&mut Backend>, id: &QString);

        #[qinvokable]
        #[cxx_name = "setDaemonEnabled"]
        fn set_daemon_enabled(self: Pin<&mut Backend>, enabled: bool);

        /// Réponse à la proposition de traduire une source en français.
        #[qinvokable]
        #[cxx_name = "answerTranslation"]
        fn answer_translation(self: Pin<&mut Backend>, spec: &QString, accept: bool);

        /// Où calculer la traduction : `auto`, `gpu`, `cpu`.
        #[qinvokable]
        #[cxx_name = "setTranslateDevice"]
        fn set_translate_device(self: Pin<&mut Backend>, device: &QString);

        /// Action venue de l'icône de la zone de notification : `toggle`, `search`, `quit`.
        #[qsignal]
        #[cxx_name = "trayAction"]
        fn tray_action(self: Pin<&mut Backend>, action: QString);

        /// Message bref à montrer (source ajoutée, réglage appliqué, erreur).
        #[qsignal]
        fn notice(self: Pin<&mut Backend>, message: QString, error: bool);

        /// Étape de la recherche en cours : `analyze`, `search`, `graph`, `done`.
        #[qsignal]
        fn phase(self: Pin<&mut Backend>, name: QString);

        /// Carte disposée (JSON : nœuds et liens avec leurs coordonnées).
        #[qsignal]
        fn map(self: Pin<&mut Backend>, json: QString);

        /// Mots reconnus et intention (JSON).
        #[qsignal]
        fn terms(self: Pin<&mut Backend>, json: QString);

        /// Résultats définitifs (JSON).
        #[qsignal]
        fn results(self: Pin<&mut Backend>, json: QString, took_ms: f64);

        #[qsignal]
        fn failed(self: Pin<&mut Backend>, message: QString);

        /// Statistiques du moteur (JSON ; `error` en cas d'échec).
        #[qsignal]
        fn stats(self: Pin<&mut Backend>, json: QString);

        /// Catalogue des sources proposées (JSON).
        #[qsignal]
        fn catalog(self: Pin<&mut Backend>, json: QString);

        /// Estimation d'un import de Wikipédia (JSON ; `error` en cas d'échec).
        #[qsignal]
        fn estimate(self: Pin<&mut Backend>, json: QString);

        /// Flux d'info et leurs contenus (JSON).
        #[qsignal]
        fn feeds(self: Pin<&mut Backend>, json: QString);

        /// Document chargé pour le lecteur (JSON).
        #[qsignal]
        fn document(self: Pin<&mut Backend>, json: QString);
    }

    impl cxx_qt::Threading for Backend {}
}

/// Mise à jour de l'interface, exécutée dans le fil de Qt.
type UiUpdate = Box<dyn FnOnce(Pin<&mut qobject::Backend>) + Send>;

/// Intervalle de relecture de l'état du moteur.
const STATUS_EVERY: Duration = Duration::from_secs(2);
/// Relecture des flux d'info au plus tard après ce délai (la fraîcheur évolue).
const FEEDS_EVERY: Duration = Duration::from_secs(60);

pub struct BackendRust {
    status: QString,
    /// Pilotage pour les captures et les essais visuels (JSON ; vide sinon) :
    /// `NKNOWN_APP_QUERY` (requête à lancer), `NKNOWN_APP_SNAPSHOT` (préfixe des captures).
    automation: QString,
    /// Demande de lancement (`--query`, `--select`), la sienne puis celles des lancements
    /// suivants (JSON avec un numéro, pour que deux demandes identiques se distinguent).
    request: QString,
    /// Niveau d'élargissement envoyé avec chaque recherche : `strict`, `normal`, `diverse`.
    variety: QString,
    /// Une zone de notification accueille l'icône : fermer la fenêtre l'y range.
    tray_available: bool,
    engine: Arc<Slot>,
    generation: Arc<AtomicU64>,
    /// Flux d'info à relire au prochain tour du fil d'état.
    feeds_dirty: Arc<AtomicBool>,
}

impl Default for BackendRust {
    fn default() -> Self {
        Self {
            status: QString::default(),
            automation: QString::default(),
            request: QString::default(),
            variety: QString::from("normal"),
            tray_available: false,
            engine: Arc::default(),
            generation: Arc::default(),
            feeds_dirty: Arc::default(),
        }
    }
}

/// Niveau reçu du QML ; une valeur inconnue (ou vide) reste le comportement historique.
fn api_variety(value: &str) -> api::Variety {
    match value.trim().to_ascii_lowercase().as_str() {
        "strict" => api::Variety::Strict,
        "diverse" => api::Variety::Diverse,
        _ => api::Variety::Normal,
    }
}

/// Le moteur courant, remplaçable (changement de réglages) : vide pendant une connexion, que
/// les recherches attendent.
#[derive(Default)]
pub struct Slot {
    current: Mutex<Option<Result<Arc<Engine>, String>>>,
    ready: Condvar,
}

impl Slot {
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Result<Arc<Engine>, String>>> {
        self.current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn wait(&self) -> Result<Arc<Engine>, String> {
        let mut g = self.lock();
        while g.is_none() {
            g = self
                .ready
                .wait(g)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        g.clone()
            .unwrap_or_else(|| Err("moteur indisponible".into()))
    }

    fn get(&self) -> Option<Result<Arc<Engine>, String>> {
        self.lock().clone()
    }

    /// (Re)connexion selon les réglages, dans un fil à part.
    fn connect(self: &Arc<Self>) {
        *self.lock() = None;
        let slot = self.clone();
        std::thread::spawn(move || {
            let engine = Engine::connect(&Settings::load()).map(Arc::new);
            *slot.lock() = Some(engine);
            slot.ready.notify_all();
        });
    }
}

/// `systemctl --user <args> nknownd.service` ; sa sortie standard.
fn systemctl(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("systemctl")
        .arg("--user")
        .args(args)
        .arg("nknownd.service")
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

impl qobject::Backend {
    fn start(mut self: Pin<&mut Self>) {
        if let Ok(prefix) = std::env::var("NKNOWN_APP_SNAPSHOT") {
            let query = std::env::var("NKNOWN_APP_QUERY").unwrap_or_default();
            let source = std::env::var("NKNOWN_APP_SOURCE").unwrap_or_default();
            let json = serde_json::json!({ "snapshot": prefix, "query": query, "source": source });
            self.as_mut()
                .set_automation(QString::from(&json.to_string()));
        }
        let first = crate::instance::Request::from_args(std::env::args().skip(1));
        if !first.is_empty() {
            self.as_mut().set_request(request_json(&first, 0));
        }
        // Pas d'icône pendant les captures pilotées (elles tournent hors écran).
        if self.automation().is_empty() {
            let tray_thread = self.qt_thread();
            let available = crate::tray::spawn(move |action| {
                let name = QString::from(match action {
                    crate::tray::Action::Toggle => "toggle",
                    crate::tray::Action::Search => "search",
                    crate::tray::Action::Quit => "quit",
                });
                let _ = tray_thread.queue(move |obj| obj.tray_action(name));
            });
            self.as_mut().set_tray_available(available);
        }
        let forwarded = self.qt_thread();
        let counter = AtomicU64::new(1);
        crate::instance::listen(move |req| {
            let json = request_json(&req, counter.fetch_add(1, Ordering::SeqCst));
            let _ = forwarded.queue(move |obj| obj.set_request(json));
        });
        let thread = self.qt_thread();
        let engine = self.rust().engine.clone();
        let feeds_dirty = self.rust().feeds_dirty.clone();
        feeds_dirty.store(true, Ordering::SeqCst);
        engine.connect();
        std::thread::Builder::new()
            .name("nknown-moteur".into())
            .spawn(move || {
                // Flux relus quand la base change, à la demande, et au moins chaque minute.
                let mut feeds_docs: Option<u64> = None;
                let mut feeds_at = Instant::now();
                loop {
                    let settings = Settings::load();
                    // « enabled », « disabled »… ; vide si le service n'est pas installé.
                    let daemon = systemctl(&["is-enabled"]).unwrap_or_default();
                    let common = serde_json::json!({
                        "settings": settings,
                        "daemon": {
                            "installed": !daemon.is_empty() && daemon != "not-found",
                            "enabled": daemon == "enabled",
                        },
                    });
                    let mut status = match engine.get() {
                        None => serde_json::json!({ "engine": "connecting" }),
                        Some(Ok(e)) => match e.status() {
                            Ok(s) => {
                                let docs = Some(s.docs);
                                if feeds_dirty.swap(false, Ordering::SeqCst)
                                    || docs != feeds_docs
                                    || feeds_at.elapsed() > FEEDS_EVERY
                                {
                                    feeds_docs = docs;
                                    feeds_at = Instant::now();
                                    if let Ok(f) = e.feeds() {
                                        let json = QString::from(&view::feeds(&f).to_string());
                                        let _ = thread.queue(move |obj| obj.feeds(json));
                                    }
                                }
                                serde_json::json!({
                                "engine": e.name(),
                                "status": s,
                                "sources": e.sources().unwrap_or_default(),
                                "analysis": e.settings().ok(),
                                "translation": e.translation().ok(),
                                })
                            }
                            Err(err) => serde_json::json!({ "engine": e.name(), "error": err }),
                        },
                        Some(Err(err)) => serde_json::json!({ "engine": "none", "error": err }),
                    };
                    if let (Some(s), Some(c)) = (status.as_object_mut(), common.as_object()) {
                        s.extend(c.clone());
                    }
                    let json = QString::from(&status.to_string());
                    if thread.queue(move |obj| obj.set_status(json)).is_err() {
                        return; // interface fermée
                    }
                    std::thread::sleep(STATUS_EVERY);
                }
            })
            .expect("fil du moteur");
    }

    fn search(self: Pin<&mut Self>, query: &QString) {
        let query = query.to_string();
        let variety = api_variety(&self.variety().to_string());
        let thread = self.qt_thread();
        let engine = self.rust().engine.clone();
        let generation = self.rust().generation.clone();
        let mine = generation.fetch_add(1, Ordering::SeqCst) + 1;
        std::thread::Builder::new()
            .name("nknown-recherche".into())
            .spawn(move || {
                // Les évènements d'une recherche éclipsée ne sont plus montrés.
                let send = |f: UiUpdate| {
                    if generation.load(Ordering::SeqCst) == mine {
                        let _ = thread.queue(f);
                    }
                };
                // Lancée avec une requête, l'app peut chercher avant d'être connectée : on attend.
                let engine = match engine.wait() {
                    Ok(e) => e,
                    Err(err) => {
                        let message = QString::from(&err);
                        send(Box::new(move |obj| obj.failed(message)));
                        return;
                    }
                };
                let mut graph = Graph::default();
                let result = engine.search(&query, variety, &mut |event| match event {
                    api::SearchEvent::Phase { phase } => {
                        let name = QString::from(phase_name(phase));
                        send(Box::new(move |obj| obj.phase(name)));
                    }
                    api::SearchEvent::Terms { intent, terms } => {
                        let json = serde_json::json!({ "intent": intent, "terms": terms });
                        let json = QString::from(&json.to_string());
                        send(Box::new(move |obj| obj.terms(json)));
                    }
                    api::SearchEvent::GraphPatch(patch) => {
                        graph.apply(&patch);
                        let json = map_json(&graph);
                        send(Box::new(move |obj| obj.map(json)));
                    }
                    api::SearchEvent::Done(resp) => {
                        // La carte finale fait foi (index des résultats compris).
                        graph = resp.graph.clone();
                        let json = map_json(&graph);
                        let hits = serde_json::to_string(&view::hits(&resp)).unwrap_or_default();
                        let hits = QString::from(&hits);
                        let took = resp.took_ms;
                        send(Box::new(move |mut obj| {
                            obj.as_mut().map(json);
                            obj.as_mut().results(hits, took);
                            obj.phase(QString::from("done"));
                        }));
                    }
                    api::SearchEvent::Error { message } => {
                        let message = QString::from(&message);
                        send(Box::new(move |obj| obj.failed(message)));
                    }
                });
                if let Err(e) = result {
                    let message = QString::from(&e);
                    send(Box::new(move |obj| obj.failed(message)));
                }
            })
            .expect("fil de recherche");
    }
}

impl qobject::Backend {
    /// Travail bloquant (moteur, systemd, KWallet) hors du fil de Qt, puis un message.
    fn background(
        self: Pin<&mut Self>,
        work: impl FnOnce(&Arc<Slot>) -> Result<String, String> + Send + 'static,
    ) {
        let thread = self.qt_thread();
        let slot = self.rust().engine.clone();
        std::thread::spawn(move || {
            let (message, error) = match work(&slot) {
                Ok(m) => (m, false),
                Err(e) => (e, true),
            };
            if !message.is_empty() {
                let message = QString::from(&message);
                let _ = thread.queue(move |obj| obj.notice(message, error));
            }
        });
    }

    fn add_source(self: Pin<&mut Self>, spec: &QString) {
        let spec = spec.to_string();
        self.background(move |slot| {
            let view = slot.wait()?.add_source(&normalize_source(&spec))?;
            Ok(format!("Source ajoutée : {}", short(&view.spec)))
        });
    }

    fn remove_source(self: Pin<&mut Self>, id: &QString) {
        let id = id.to_string();
        self.background(move |slot| {
            slot.wait()?.remove_source(&id)?;
            Ok("Source retirée (ses documents restent consultables)".into())
        });
    }

    fn set_engine(self: Pin<&mut Self>, mode: &QString, url: &QString, token: &QString) {
        let (mode, url, token) = (mode.to_string(), url.to_string(), token.to_string());
        self.background(move |slot| {
            let mut settings = Settings::load();
            settings.engine = match mode.as_str() {
                "daemon" => Mode::Daemon,
                "embedded" => Mode::Embedded,
                "remote" => Mode::Remote,
                _ => Mode::Auto,
            };
            if settings.engine == Mode::Remote {
                settings.remote_url = url.trim().trim_end_matches('/').to_owned();
                if !token.trim().is_empty() {
                    crate::secrets::store_token(&settings.remote_url, token.trim())?;
                }
            }
            settings.save().map_err(|e| e.to_string())?;
            slot.connect();
            Ok(String::new())
        });
    }

    fn set_analysis(self: Pin<&mut Self>, parser: bool, precision: &QString, device: &QString) {
        let settings = api::EngineSettings {
            parser,
            precision: if precision.to_string() == "light" {
                api::ParserPrecision::Light
            } else {
                api::ParserPrecision::Standard
            },
            device: match device.to_string().as_str() {
                "gpu" => api::DeviceChoice::Gpu,
                "cpu" => api::DeviceChoice::Cpu,
                _ => api::DeviceChoice::Auto,
            },
        };
        self.background(move |slot| {
            slot.wait()?.set_settings(settings)?;
            Ok("Réglages d'analyse appliqués".into())
        });
    }

    fn open_document(self: Pin<&mut Self>, doc: &QString, passage: &QString) {
        let thread = self.qt_thread();
        let slot = self.rust().engine.clone();
        let Ok(id) = doc.to_string().parse::<u64>() else {
            return;
        };
        let passage = passage.to_string();
        std::thread::spawn(move || {
            let result = slot.wait().and_then(|e| e.document(id));
            let _ = thread.queue(move |obj| match result {
                Ok(d) => {
                    let json = view::document(&d, &passage, view::read_file);
                    obj.document(QString::from(&json.to_string()));
                }
                Err(e) => obj.notice(QString::from(&format!("Lecture impossible : {e}")), true),
            });
        });
    }

    fn reveal_file(self: Pin<&mut Self>, path: &QString) {
        let path = path.to_string();
        std::thread::spawn(move || {
            if show_in_file_manager(&path).is_err() {
                // Sans gestionnaire D-Bus : le dossier, avec l'application par défaut.
                let dir = std::path::Path::new(&path)
                    .parent()
                    .map_or(path.clone(), |p| p.display().to_string());
                let _ = std::process::Command::new("xdg-open")
                    .arg(dir)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
            }
        });
    }

    fn add_sources(self: Pin<&mut Self>, specs: &QString) {
        let specs: Vec<String> = specs
            .to_string()
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(normalize_source)
            .collect();
        self.background(move |slot| {
            let engine = slot.wait()?;
            let (mut ok, mut errors) = (0, Vec::new());
            for spec in &specs {
                match engine.add_source(spec) {
                    Ok(_) => ok += 1,
                    Err(e) => errors.push(format!("{} : {e}", short(spec))),
                }
            }
            match errors.first() {
                None => Ok(format!(
                    "{ok} source{} ajoutée{}",
                    if ok > 1 { "s" } else { "" },
                    if ok > 1 { "s" } else { "" }
                )),
                Some(e) => Err(format!("{ok} ajoutée(s), {} en échec — {e}", errors.len())),
            }
        });
    }

    fn answer_translation(self: Pin<&mut Self>, spec: &QString, accept: bool) {
        let spec = spec.to_string();
        self.background(move |slot| {
            slot.wait()?.answer_translation(api::TranslationRequest {
                spec: Some(spec.clone()),
                accept: Some(accept),
                device: None,
            })?;
            Ok(if accept {
                format!("{} : traduction en français lancée", short(&spec))
            } else {
                String::new()
            })
        });
    }

    fn set_translate_device(self: Pin<&mut Self>, device: &QString) {
        let device = match device.to_string().as_str() {
            "gpu" => api::DeviceChoice::Gpu,
            "cpu" => api::DeviceChoice::Cpu,
            _ => api::DeviceChoice::Auto,
        };
        self.background(move |slot| {
            slot.wait()?.answer_translation(api::TranslationRequest {
                spec: None,
                accept: None,
                device: Some(device),
            })?;
            Ok(String::new())
        });
    }

    fn pause_source(self: Pin<&mut Self>, id: &QString, paused: bool) {
        let id = id.to_string();
        self.background(move |slot| {
            let view = slot.wait()?.pause_source(&id, paused)?;
            Ok(format!(
                "{} : {}",
                short(&view.spec),
                if paused { "en pause" } else { "reprise" }
            ))
        });
    }

    fn load_stats(self: Pin<&mut Self>, word: &QString) {
        let word = word.to_string();
        let thread = self.qt_thread();
        let slot = self.rust().engine.clone();
        std::thread::spawn(move || {
            let json = match slot.wait().and_then(|e| e.stats(&word)) {
                Ok(s) => serde_json::to_string(&s).unwrap_or_default(),
                Err(e) => serde_json::json!({ "error": e }).to_string(),
            };
            let _ = thread.queue(move |obj| obj.stats(QString::from(&json)));
        });
    }

    fn load_catalog(self: Pin<&mut Self>) {
        let thread = self.qt_thread();
        let slot = self.rust().engine.clone();
        std::thread::spawn(move || {
            let json = match slot.wait().and_then(|e| e.catalog()) {
                Ok(c) => serde_json::to_string(&c).unwrap_or_default(),
                Err(e) => serde_json::json!({ "error": e }).to_string(),
            };
            let _ = thread.queue(move |obj| obj.catalog(QString::from(&json)));
        });
    }

    fn estimate_wikipedia(self: Pin<&mut Self>, edition: &QString, tier: &QString, limit: f64) {
        let limit = limit.max(0.0) as u64;
        let req = api::WikiEstimateRequest {
            edition: edition.to_string(),
            tier: if tier.to_string() == "top" {
                api::WikiTier::TopRead { limit }
            } else {
                api::WikiTier::Dump {
                    limit: (limit > 0).then_some(limit),
                }
            },
        };
        let thread = self.qt_thread();
        let slot = self.rust().engine.clone();
        std::thread::spawn(move || {
            let json = match slot.wait().and_then(|e| e.wikipedia_estimate(&req)) {
                Ok(e) => serde_json::json!({
                    "spec": req.tier.spec(&req.edition),
                    "articles": e.articles,
                    "download_bytes": e.download_bytes,
                    "disk_bytes": e.disk_bytes,
                    "seconds": e.seconds,
                }),
                Err(e) => serde_json::json!({ "spec": req.tier.spec(&req.edition), "error": e }),
            };
            let json = json.to_string();
            let _ = thread.queue(move |obj| obj.estimate(QString::from(&json)));
        });
    }

    fn refresh_feeds(self: Pin<&mut Self>) {
        self.rust().feeds_dirty.store(true, Ordering::SeqCst);
    }

    fn save_feed(self: Pin<&mut Self>, id: &QString, title: &QString, keywords: &QString) {
        let id = id.to_string();
        let req = api::FeedRequest {
            title: title.to_string(),
            keywords: keywords
                .to_string()
                .split([',', ';', '\n'])
                .map(|k| k.trim().to_owned())
                .filter(|k| !k.is_empty())
                .collect(),
        };
        let dirty = self.rust().feeds_dirty.clone();
        self.background(move |slot| {
            let view = slot.wait()?.save_feed(&id, req)?;
            dirty.store(true, Ordering::SeqCst);
            Ok(if id.is_empty() {
                format!("Flux « {} » créé", view.title)
            } else {
                String::new()
            })
        });
    }

    fn remove_feed(self: Pin<&mut Self>, id: &QString) {
        let id = id.to_string();
        let dirty = self.rust().feeds_dirty.clone();
        self.background(move |slot| {
            slot.wait()?.remove_feed(&id)?;
            dirty.store(true, Ordering::SeqCst);
            Ok("Flux retiré".into())
        });
    }

    fn feed_seen(self: Pin<&mut Self>, id: &QString) {
        let id = id.to_string();
        let dirty = self.rust().feeds_dirty.clone();
        self.background(move |slot| {
            slot.wait()?.feed_seen(&id)?;
            dirty.store(true, Ordering::SeqCst);
            Ok(String::new())
        });
    }

    fn set_daemon_enabled(self: Pin<&mut Self>, enabled: bool) {
        self.background(move |slot| {
            let action = if enabled { "enable" } else { "disable" };
            let ok = std::process::Command::new("systemctl")
                .args(["--user", action, "--now", "nknownd.service"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
            if !ok {
                return Err("le service nknownd n'a pas pu être modifié".into());
            }
            // Le moteur suit : démon arrêté → cœur intégré (en automatique), et inversement.
            slot.connect();
            Ok(if enabled {
                "Le démon démarre avec la session".into()
            } else {
                "Démon désactivé : l'app utilise son cœur intégré".into()
            })
        });
    }
}

/// Ce que l'utilisateur colle → spécification de source : un chemin est un dossier, une page
/// Wikipédia un article, une autre adresse web un flux (RSS, Atom, JSON Feed).
fn normalize_source(input: &str) -> String {
    let s = input.trim();
    let s = s.strip_prefix("file://").unwrap_or(s);
    if let Some(rest) = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        && let Some((host, path)) = rest.split_once("/wiki/")
        && let Some(edition) = host.strip_suffix(".wikipedia.org")
    {
        return format!("wiki:{edition}:{}", path.replace('_', " "));
    }
    if s.starts_with("http://") || s.starts_with("https://") {
        return format!("rss:{s}");
    }
    if let Some(home) = s.strip_prefix("~/") {
        return format!("{}/{home}", std::env::var("HOME").unwrap_or_default());
    }
    s.to_owned()
}

/// Nom court d'une source pour un message : le dossier, l'hôte du flux, l'article.
fn short(spec: &str) -> String {
    if let Some(rest) = spec.strip_prefix("wiki-top:") {
        let (ed, n) = rest.split_once(':').unwrap_or((rest, ""));
        return format!("Wikipédia {ed} · {n} plus lus");
    }
    if let Some(rest) = spec.strip_prefix("wiki-dump:") {
        return match rest.split_once(':') {
            Some((ed, n)) => format!("Wikipédia {ed} · {n} articles"),
            None => format!("Wikipédia {rest} · édition complète"),
        };
    }
    let s = if let Some(url) = spec
        .strip_prefix("rss:")
        .or_else(|| spec.strip_prefix("web:"))
    {
        let host = url.split("://").nth(1).unwrap_or(url);
        host.split('/')
            .next()
            .unwrap_or(host)
            .trim_start_matches("www.")
    } else if let Some(wiki) = spec.strip_prefix("wiki:") {
        wiki.split_once(':').map_or(wiki, |(_, title)| title)
    } else {
        spec.trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or(spec)
    };
    s.chars().take(60).collect()
}

/// `org.freedesktop.FileManager1.ShowItems` : Dolphin (ou un autre) ouvre le dossier et
/// sélectionne le fichier.
fn show_in_file_manager(path: &str) -> zbus::Result<()> {
    let mut uri = String::from("file://");
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            uri.push(b as char);
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    zbus::block_on(async {
        let conn = zbus::Connection::session().await?;
        conn.call_method(
            Some("org.freedesktop.FileManager1"),
            "/org/freedesktop/FileManager1",
            Some("org.freedesktop.FileManager1"),
            "ShowItems",
            &(vec![uri], ""),
        )
        .await
        .map(drop)
    })
}

fn request_json(req: &crate::instance::Request, n: u64) -> QString {
    let json = serde_json::json!({ "n": n, "query": req.query, "select": req.select });
    QString::from(&json.to_string())
}

fn phase_name(p: api::Phase) -> &'static str {
    match p {
        api::Phase::Analyze => "analyze",
        api::Phase::Search => "search",
        api::Phase::Graph => "graph",
    }
}

fn map_json(graph: &Graph) -> QString {
    QString::from(&serde_json::to_string(&view::map(graph)).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::normalize_source;

    #[test]
    fn pasted_sources() {
        assert_eq!(normalize_source(" /home/a/notes "), "/home/a/notes");
        assert_eq!(normalize_source("file:///home/a/notes"), "/home/a/notes");
        assert_eq!(
            normalize_source("https://fr.wikipedia.org/wiki/Tour_Eiffel"),
            "wiki:fr:Tour Eiffel"
        );
        assert_eq!(
            normalize_source("https://example.org/feed.xml"),
            "rss:https://example.org/feed.xml"
        );
        assert_eq!(normalize_source("rss:https://x/y"), "rss:https://x/y");
    }

    #[test]
    fn short_names() {
        use super::short;
        assert_eq!(short("/home/a/notes/"), "notes");
        assert_eq!(
            short("rss:https://www.lemonde.fr/rss/une.xml"),
            "lemonde.fr"
        );
        assert_eq!(short("wiki:fr:Tour Eiffel"), "Tour Eiffel");
        assert_eq!(short("wiki-top:fr:10000"), "Wikipédia fr · 10000 plus lus");
        assert_eq!(short("wiki-dump:en"), "Wikipédia en · édition complète");
        assert_eq!(short("web:https://doc.rs/x pages=5"), "doc.rs");
    }
}
