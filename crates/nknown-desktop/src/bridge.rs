//! Pont Rust ↔ QML : l'objet `Backend` lance les recherches et relaie leur progression.
//!
//! Les échanges avec le QML passent par du JSON (carte disposée, résultats, état) : le QML n'a
//! qu'à afficher et animer. Une recherche relancée avant la fin de la précédente l'éclipse : les
//! évènements d'une recherche dépassée sont ignorés.

// Le pont généré par cxx déclare des blocs `unsafe extern "C++"` : seul ce fichier y a droit.
#![allow(unsafe_code)]

use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use nknown_api::{self as api, Graph};

use crate::client::Engine;
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
        type Backend = super::BackendRust;

        /// Connexion au moteur et suivi de son état ; à appeler une fois l'interface prête.
        #[qinvokable]
        fn start(self: Pin<&mut Backend>);

        #[qinvokable]
        fn search(self: Pin<&mut Backend>, query: &QString);

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
    engine: Arc<OnceLock<Result<Engine, String>>>,
    generation: Arc<AtomicU64>,
}

impl qobject::Backend {
    fn start(mut self: Pin<&mut Self>) {
        if let Ok(prefix) = std::env::var("NKNOWN_APP_SNAPSHOT") {
            let query = std::env::var("NKNOWN_APP_QUERY").unwrap_or_default();
            let json = serde_json::json!({ "snapshot": prefix, "query": query });
            self.as_mut()
                .set_automation(QString::from(&json.to_string()));
        }
        let thread = self.qt_thread();
        let engine = self.rust().engine.clone();
        std::thread::Builder::new()
            .name("nknown-moteur".into())
            .spawn(move || {
                let engine = engine.get_or_init(Engine::connect);
                loop {
                    let status = match engine {
                        Ok(e) => match e.status() {
                            Ok(s) => serde_json::json!({ "engine": e.name(), "status": s }),
                            Err(err) => serde_json::json!({ "engine": e.name(), "error": err }),
                        },
                        Err(err) => serde_json::json!({ "engine": "none", "error": err }),
                    };
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
                let Some(Ok(engine)) = engine.get() else {
                    send(Box::new(|obj| {
                        obj.failed(QString::from("moteur en démarrage"))
                    }));
                    return;
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
