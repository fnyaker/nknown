//! Serveur HTTP de NKnown : l'API REST (`/api/v1`, voir `nknown-api`) et, pour le serveur
//! distant, l'interface web. Le même routeur sert le démon local (socket Unix, sans jeton).

pub mod auth;
pub mod daemon;
#[cfg(feature = "plasma")]
pub mod plasma;

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use nknown_api as api;
use nknown_service::Service;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;

pub use auth::Auth;

/// Taille maximale d'un corps de requête (un document ajouté par l'API).
const MAX_BODY: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct AppState {
    pub service: Arc<Service>,
    pub auth: Arc<Auth>,
}

/// Routeur complet. `web` : dossier de l'interface web compilée (serveur distant seulement).
pub fn router(state: AppState, web: Option<PathBuf>) -> Router {
    use api::routes::*;
    let protected = Router::new()
        .route(STATUS, get(status))
        .route(SEARCH, post(search))
        .route(DOCUMENTS, post(add_document))
        .route(&format!("{DOCUMENTS}/{{id}}"), get(document))
        .route(SOURCES, get(sources).post(add_source))
        .route(SETTINGS, get(settings).put(update_settings))
        .route(
            &format!("{SOURCES}/{{id}}"),
            axum::routing::delete(remove_source),
        )
        .route("/collect", post(collect))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth));
    let api = Router::new()
        .route(SESSION, post(open_session).delete(close_session))
        .merge(protected)
        .fallback(|| async { error(StatusCode::NOT_FOUND, "route inconnue") })
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .with_state(state);

    let mut app = Router::new().nest(api::PREFIX, api);
    if let Some(dir) = web {
        let index = dir.join("index.html");
        app = app.fallback_service(ServeDir::new(dir).fallback(ServeFile::new(index)));
    }
    security_headers(app)
}

fn security_headers(app: Router) -> Router {
    let headers = [
        (
            header::CONTENT_SECURITY_POLICY,
            "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' \
             'unsafe-inline'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; \
             base-uri 'none'; form-action 'none'",
        ),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::REFERRER_POLICY, "no-referrer"),
        (header::X_FRAME_OPTIONS, "DENY"),
    ];
    headers.into_iter().fold(app, |app, (name, value)| {
        app.layer(SetResponseHeaderLayer::if_not_present(
            name,
            HeaderValue::from_static(value),
        ))
    })
}

// ---------------------------------------------------------------- authentification

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

fn session_cookie(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == api::SESSION_COOKIE)
        .map(|(_, v)| v)
}

async fn require_auth(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let headers = req.headers();
    if state.auth.check(bearer(headers), session_cookie(headers)) {
        next.run(req).await
    } else {
        error(StatusCode::UNAUTHORIZED, "jeton manquant ou invalide")
    }
}

async fn open_session(
    State(state): State<AppState>,
    Json(req): Json<api::SessionRequest>,
) -> Response {
    match state.auth.open_session(&req.token) {
        Some(id) => (
            StatusCode::NO_CONTENT,
            [(header::SET_COOKIE, state.auth.cookie(&id))],
        )
            .into_response(),
        None => error(StatusCode::UNAUTHORIZED, "jeton invalide"),
    }
}

async fn close_session(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(id) = session_cookie(&headers) {
        state.auth.close_session(id);
    }
    (
        StatusCode::NO_CONTENT,
        [(
            header::SET_COOKIE,
            format!("{}=; Path=/api; Max-Age=0", api::SESSION_COOKIE),
        )],
    )
        .into_response()
}

// ---------------------------------------------------------------- routes

fn error(status: StatusCode, message: impl Into<String>) -> Response {
    (
        status,
        Json(api::ApiError {
            error: message.into(),
        }),
    )
        .into_response()
}

fn service_error(e: nknown_service::Error) -> Response {
    let status = match e {
        nknown_service::Error::Invalid(_) => StatusCode::BAD_REQUEST,
        nknown_service::Error::Unsupported(_) => StatusCode::NOT_IMPLEMENTED,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    error(status, e.to_string())
}

/// Exécute un travail bloquant du cœur hors des fils de tokio.
// Une `Response` en erreur est l'idiome axum ; elle ne transite que jusqu'au gestionnaire.
#[allow(clippy::result_large_err)]
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, nknown_service::Error> + Send + 'static,
) -> Result<T, Response> {
    match tokio::task::spawn_blocking(f).await {
        Ok(r) => r.map_err(service_error),
        Err(_) => Err(error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "tâche interrompue",
        )),
    }
}

async fn status(State(state): State<AppState>) -> Response {
    match blocking(move || state.service.status()).await {
        Ok(s) => Json(s).into_response(),
        Err(r) => r,
    }
}

/// Flux NDJSON d'évènements (progression puis résultat), ou le résultat seul si le client
/// demande `application/json`.
async fn search(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<api::SearchRequest>,
) -> Response {
    let wants_json = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("application/json") && !v.contains(api::NDJSON));
    if wants_json {
        return match blocking(move || state.service.search(&req, |_| {})).await {
            Ok(r) => Json(r).into_response(),
            Err(r) => r,
        };
    }
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<String, std::io::Error>>(16);
    tokio::task::spawn_blocking(move || {
        let send = |e: &api::SearchEvent| {
            let mut line = serde_json::to_string(e).expect("évènement sérialisable");
            line.push('\n');
            // Client parti : on termine quand même la recherche, sans rien envoyer.
            let _ = tx.blocking_send(Ok(line));
        };
        if let Err(e) = state.service.search(&req, |e| send(&e)) {
            send(&api::SearchEvent::Error {
                message: e.to_string(),
            });
        }
    });
    (
        [
            (header::CONTENT_TYPE, api::NDJSON),
            (header::CACHE_CONTROL, "no-store"),
        ],
        Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx)),
    )
        .into_response()
}

async fn document(State(state): State<AppState>, Path(id): Path<u64>) -> Response {
    match blocking(move || state.service.document(id)).await {
        Ok(Some(d)) => Json(d).into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, "document inconnu"),
        Err(r) => r,
    }
}

async fn add_document(
    State(state): State<AppState>,
    Json(req): Json<api::AddDocument>,
) -> Response {
    match blocking(move || state.service.add_document(req)).await {
        Ok(a) => (StatusCode::CREATED, Json(a)).into_response(),
        Err(r) => r,
    }
}

async fn settings(State(state): State<AppState>) -> Response {
    match blocking(move || Ok(state.service.settings())).await {
        Ok(s) => Json(s).into_response(),
        Err(r) => r,
    }
}

/// Réglages d'analyse ; les modèles sont rechargés avant la réponse.
async fn update_settings(
    State(state): State<AppState>,
    Json(req): Json<api::EngineSettings>,
) -> Response {
    match blocking(move || state.service.update_settings(req)).await {
        Ok(s) => Json(s).into_response(),
        Err(r) => r,
    }
}

async fn sources(State(state): State<AppState>) -> Response {
    match blocking(move || Ok(state.service.sources())).await {
        Ok(s) => Json(s).into_response(),
        Err(r) => r,
    }
}

/// Ajoute une source et lance sa première collecte en arrière-plan.
async fn add_source(State(state): State<AppState>, Json(req): Json<api::AddSource>) -> Response {
    let svc = state.service.clone();
    match blocking(move || svc.add_source(&req.spec)).await {
        Ok(view) => {
            let (svc, spec) = (state.service.clone(), view.spec.clone());
            tokio::task::spawn_blocking(move || {
                if let Err(e) = svc.collect(&spec) {
                    eprintln!("{spec} : {e}");
                }
            });
            (StatusCode::CREATED, Json(view)).into_response()
        }
        Err(r) => r,
    }
}

async fn remove_source(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match blocking(move || state.service.remove_source(&id)).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => error(StatusCode::NOT_FOUND, "source inconnue"),
        Err(r) => r,
    }
}

/// Relance la collecte de toutes les sources, en arrière-plan.
async fn collect(State(state): State<AppState>) -> Response {
    tokio::task::spawn_blocking(move || {
        state.service.collect_all();
    });
    StatusCode::ACCEPTED.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use nknown_service::{Config, Options};
    use tower::ServiceExt;

    fn app(auth: Auth) -> (tempfile::TempDir, Router) {
        let dir = tempfile::tempdir().unwrap();
        let service = Service::open(Options {
            data_dir: dir.path().to_owned(),
            config: Config {
                sources: vec![],
                ..Config::default()
            },
            config_path: None,
        })
        .unwrap();
        let state = AppState {
            service: Arc::new(service),
            auth: Arc::new(auth),
        };
        (dir, router(state, None))
    }

    async fn call(app: &Router, req: Request<Body>) -> (StatusCode, HeaderMap, String) {
        let res = app.clone().oneshot(req).await.unwrap();
        let (parts, body) = res.into_parts();
        let bytes = body.collect().await.unwrap().to_bytes();
        (
            parts.status,
            parts.headers,
            String::from_utf8(bytes.to_vec()).unwrap(),
        )
    }

    fn post(uri: &str) -> axum::http::request::Builder {
        Request::post(format!("{}{uri}", api::PREFIX))
            .header(header::CONTENT_TYPE, "application/json")
    }

    #[tokio::test]
    async fn token_is_required() {
        let (_d, app) = app(Auth::token("t0k3n".into(), true));
        let status_req = |auth: Option<&str>| {
            let mut b = Request::get(format!("{}/status", api::PREFIX));
            if let Some(a) = auth {
                b = b.header(header::AUTHORIZATION, a);
            }
            b.body(Body::empty()).unwrap()
        };
        assert_eq!(
            call(&app, status_req(None)).await.0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            call(&app, status_req(Some("Bearer faux"))).await.0,
            StatusCode::UNAUTHORIZED
        );
        let (code, headers, _) = call(&app, status_req(Some("Bearer t0k3n"))).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");

        // Interface web : jeton échangé contre un cookie HttpOnly.
        let body = r#"{"token":"t0k3n"}"#;
        let (code, headers, _) = call(&app, post("/session").body(Body::from(body)).unwrap()).await;
        assert_eq!(code, StatusCode::NO_CONTENT);
        let cookie = headers[header::SET_COOKIE].to_str().unwrap().to_owned();
        assert!(cookie.contains("HttpOnly") && cookie.contains("Secure"));
        let pair = cookie.split(';').next().unwrap();
        let req = Request::get(format!("{}/status", api::PREFIX))
            .header(header::COOKIE, pair)
            .body(Body::empty())
            .unwrap();
        assert_eq!(call(&app, req).await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn add_and_search_stream() {
        let (_d, app) = app(Auth::None);
        let body =
            r#"{"text":"Les graphes pondérés relient les mots et les textes.","title":"Graphes"}"#;
        let (code, _, out) = call(&app, post("/documents").body(Body::from(body)).unwrap()).await;
        assert_eq!(code, StatusCode::CREATED, "{out}");
        let added: api::Added = serde_json::from_str(&out).unwrap();

        let body = r#"{"query":"graphes mots"}"#;
        let (code, headers, out) =
            call(&app, post("/search").body(Body::from(body)).unwrap()).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(headers[header::CONTENT_TYPE], api::NDJSON);
        let events: Vec<api::SearchEvent> = out
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert!(matches!(events[0], api::SearchEvent::Phase { .. }));
        let api::SearchEvent::Done(r) = events.last().unwrap() else {
            panic!("pas de résultat : {out}");
        };
        assert_eq!(r.hits[0].doc, added.id);

        let req = post("/search")
            .header(header::ACCEPT, "application/json")
            .body(Body::from(body))
            .unwrap();
        let (_, _, out) = call(&app, req).await;
        let r: api::SearchResponse = serde_json::from_str(&out).unwrap();
        assert_eq!(r.hits.len(), 1);

        let req = Request::get(format!("{}/documents/{}", api::PREFIX, added.id))
            .body(Body::empty())
            .unwrap();
        let (code, _, out) = call(&app, req).await;
        assert_eq!(code, StatusCode::OK, "{out}");
        let req = Request::get(format!("{}/documents/999999", api::PREFIX))
            .body(Body::empty())
            .unwrap();
        assert_eq!(call(&app, req).await.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn invalid_source_is_a_client_error() {
        let (_d, app) = app(Auth::None);
        let body = r#"{"spec":"nimporte:quoi"}"#;
        let (code, _, out) = call(&app, post("/sources").body(Body::from(body)).unwrap()).await;
        assert_eq!(code, StatusCode::BAD_REQUEST);
        assert!(out.contains("source inconnue"));
    }
}
