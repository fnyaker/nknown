//! Pont Rust ↔ QML : l'objet `Backend` lance les recherches et relaie leur progression.
//!
//! Les échanges avec le QML passent par du JSON (carte disposée, résultats, état) : le QML n'a
//! qu'à afficher et animer. Une recherche relancée avant la fin de la précédente l'éclipse : les
//! évènements d'une recherche dépassée sont ignorés.

// Le pont généré par cxx déclare des blocs `unsafe extern "C++"` : seul ce fichier y a droit.
#![allow(unsafe_code)]

use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

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
        #[qinvokable]
        #[cxx_name = "setDaemonEnabled"]
        fn set_daemon_enabled(self: Pin<&mut Backend>, enabled: bool);

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
    }

    impl cxx_qt::Threading for Backend {}
}

/// Mise à jour de l'interface, exécutée dans le fil de Qt.
type UiUpdate = Box<dyn FnOnce(Pin<&mut qobject::Backend>) + Send>;

/// Intervalle de relecture de l'état du moteur.
const STATUS_EVERY: Duration = Duration::from_secs(2);

#[derive(Default)]
pub struct BackendRust {
    status: QString,
    /// Pilotage pour les captures et les essais visuels (JSON ; vide sinon) :
    /// `NKNOWN_APP_QUERY` (requête à lancer), `NKNOWN_APP_SNAPSHOT` (préfixe des captures).
    automation: QString,
    /// Demande de lancement (`--query`, `--select`), la sienne puis celles des lancements
    /// suivants (JSON avec un numéro, pour que deux demandes identiques se distinguent).
    request: QString,
    /// Une zone de notification accueille l'icône : fermer la fenêtre l'y range.
    tray_available: bool,
    engine: Arc<Slot>,
    generation: Arc<AtomicU64>,
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
        engine.connect();
        std::thread::Builder::new()
            .name("nknown-moteur".into())
            .spawn(move || {
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
                            Ok(s) => serde_json::json!({
                                "engine": e.name(),
                                "status": s,
                                "sources": e.sources().unwrap_or_default(),
                            }),
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
                let result = engine.search(&query, &mut |event| match event {
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
    let s = if let Some(url) = spec.strip_prefix("rss:") {
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
    }
}
