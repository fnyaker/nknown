//! Intégration Plasma du démon, sur le bus de session (fonctionnalité `plasma`).
//!
//! - KRunner (`org.kde.krunner1`) : les meilleurs résultats d'une requête tapée dans le lanceur
//!   de Plasma ; Entrée ouvre le document, « Afficher dans NKnown » ouvre l'app sur la recherche
//!   avec ce résultat en valeur. Descripteur : `packaging/plasma/nknown-krunner.desktop`.
//! - Notifications : modèles prêts au premier lancement, ou téléchargement impossible.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nknown_api as api;
use nknown_service::Service;
use zbus::Variant as Value;
use zbus::interface;

pub const BUS_NAME: &str = "org.nknown.krunner";
pub const RUNNER_PATH: &str = "/runner";
/// Résultats proposés à KRunner : au-delà, la liste du lanceur se noie.
const MATCHES: usize = 6;
/// Valeurs de `KRunner::QueryMatch::Type`.
const POSSIBLE_MATCH: i32 = 30;
const INFORMATIONAL_MATCH: i32 = 50;

/// (id, texte, icône, type, pertinence, propriétés) : signature `(sssida{sv})`.
type RemoteMatch = (
    String,
    String,
    String,
    i32,
    f64,
    HashMap<String, Value<'static>>,
);

#[derive(Debug, Clone)]
enum Target {
    /// Ouvrir le document (fichier ou adresse) avec l'application par défaut.
    Open {
        url: String,
        query: String,
        node: String,
    },
    /// Ouvrir l'app sur la recherche.
    App { query: String, node: Option<String> },
}

struct Runner {
    service: Arc<Service>,
    /// Cibles des derniers résultats proposés, par identifiant.
    targets: Mutex<HashMap<String, Target>>,
}

#[interface(name = "org.kde.krunner1")]
impl Runner {
    fn actions(&self) -> Vec<(String, String, String)> {
        vec![("app".into(), "Afficher dans NKnown".into(), "nknown".into())]
    }

    #[zbus(name = "Match")]
    async fn find(&self, query: String) -> Vec<RemoteMatch> {
        let query = query.trim().to_owned();
        let service = self.service.clone();
        let req = api::SearchRequest {
            query: query.clone(),
            limit: MATCHES,
            variety: api::Variety::Normal,
        };
        let Ok(Ok(resp)) = tokio::task::spawn_blocking(move || service.search(&req, |_| {})).await
        else {
            return Vec::new();
        };
        let mut targets = HashMap::new();
        let mut out: Vec<RemoteMatch> = Vec::new();
        for h in resp.hits.iter().take(MATCHES) {
            let id = format!("doc:{}", h.doc);
            let node = format!("d{}", h.doc);
            let mut props: HashMap<String, Value<'static>> = HashMap::new();
            let snippet: String = h.snippet.chars().take(140).collect();
            props.insert(
                "subtext".into(),
                Value::from(format!("{} · {snippet}", h.source)),
            );
            props.insert("category".into(), Value::from("NKnown".to_owned()));
            let target = match &h.url {
                Some(url) => {
                    props.insert("urls".into(), Value::from(vec![url.clone()]));
                    Target::Open {
                        url: url.clone(),
                        query: query.clone(),
                        node,
                    }
                }
                None => Target::App {
                    query: query.clone(),
                    node: Some(node),
                },
            };
            targets.insert(id.clone(), target);
            let icon = if h.is_code {
                "text-x-script"
            } else {
                "text-x-generic"
            };
            out.push((
                id,
                h.title.clone().unwrap_or_else(|| h.source.clone()),
                icon.into(),
                POSSIBLE_MATCH,
                0.5 + 0.45 * f64::from(h.relative),
                props,
            ));
        }
        // Toujours en dernier : la carte complète dans l'app.
        let mut props: HashMap<String, Value<'static>> = HashMap::new();
        props.insert("category".into(), Value::from("NKnown".to_owned()));
        props.insert(
            "subtext".into(),
            Value::from(format!("{} résultats et leurs liens", resp.hits.len())),
        );
        targets.insert(
            "explore".into(),
            Target::App {
                query: query.clone(),
                node: None,
            },
        );
        out.push((
            "explore".into(),
            format!("Explorer « {query} » dans NKnown"),
            "nknown".into(),
            INFORMATIONAL_MATCH,
            0.3,
            props,
        ));
        if let Ok(mut t) = self.targets.lock() {
            *t = targets;
        }
        out
    }

    fn run(&self, match_id: String, action_id: String) {
        let target = self
            .targets
            .lock()
            .ok()
            .and_then(|t| t.get(&match_id).cloned());
        let Some(target) = target else { return };
        let spawned = match (target, action_id.as_str()) {
            (Target::Open { query, node, .. }, "app") => open_app(&query, Some(&node)),
            (Target::Open { url, .. }, _) => Command::new("xdg-open").arg(url).spawn().map(drop),
            (Target::App { query, node }, _) => open_app(&query, node.as_deref()),
        };
        if let Err(e) = spawned {
            eprintln!("KRunner : {e}");
        }
    }
}

/// L'app (`nknown-app`, voisine du démon ou dans le PATH), ouverte sur une recherche.
fn open_app(query: &str, node: Option<&str>) -> std::io::Result<()> {
    let neighbour = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join("nknown-app")))
        .filter(|p| p.is_file());
    let mut cmd = Command::new(neighbour.unwrap_or_else(|| PathBuf::from("nknown-app")));
    cmd.arg("--query").arg(query);
    if let Some(node) = node {
        cmd.arg("--select").arg(node);
    }
    cmd.spawn().map(drop)
}

/// Publie le lanceur KRunner et surveille l'état des modèles pour notifier. Sans bus de session
/// (serveur, conteneur), ne fait rien.
pub async fn spawn(service: Arc<Service>) {
    let runner = Runner {
        service: service.clone(),
        targets: Mutex::new(HashMap::new()),
    };
    let built = zbus::connection::Builder::session()
        .name(BUS_NAME)
        .serve_at(RUNNER_PATH, runner)
        .build()
        .await;
    let conn = match built {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Plasma : bus de session indisponible ({e})");
            return;
        }
    };
    tokio::spawn(watch_models(conn, service));
}

/// Notifie le passage des modèles à « prêts » après un téléchargement, ou son échec (une fois).
async fn watch_models(conn: zbus::Connection, service: Arc<Service>) {
    let mut was_downloading = false;
    let mut failure_told = false;
    loop {
        let s = service.clone();
        let state = tokio::task::spawn_blocking(move || s.status().map(|st| st.models))
            .await
            .ok()
            .and_then(Result::ok);
        match state {
            Some(api::ModelsState::Downloading { .. }) => was_downloading = true,
            Some(api::ModelsState::Ready) if was_downloading => {
                notify(
                    &conn,
                    "NKnown est prêt",
                    "Modèles installés : vos documents sont en cours d'indexation.",
                )
                .await;
                was_downloading = false;
            }
            Some(api::ModelsState::Failed { message }) if !failure_told => {
                notify(
                    &conn,
                    "NKnown : modèles indisponibles",
                    &format!("{message}\nNouvel essai automatique dans quelques minutes."),
                )
                .await;
                failure_told = true;
            }
            _ => {}
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn notify(conn: &zbus::Connection, summary: &str, body: &str) {
    let hints: HashMap<&str, Value<'_>> = HashMap::new();
    let sent = conn
        .call_method(
            Some("org.freedesktop.Notifications"),
            "/org/freedesktop/Notifications",
            Some("org.freedesktop.Notifications"),
            "Notify",
            &(
                "NKnown",
                0u32,
                "nknown",
                summary,
                body,
                Vec::<&str>::new(),
                hints,
                -1i32,
            ),
        )
        .await;
    if let Err(e) = sent {
        eprintln!("notification : {e}");
    }
}
