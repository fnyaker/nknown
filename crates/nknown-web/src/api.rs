//! Appels à l'API du serveur (même origine, cookie de session `HttpOnly`) ; la recherche est un
//! flux NDJSON lu au fil de l'eau.

use js_sys::{Reflect, Uint8Array};
use nknown_api as api;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{ReadableStreamDefaultReader, Request, RequestCredentials, RequestInit, Response};

pub type Result<T> = std::result::Result<T, String>;

fn js(e: JsValue) -> String {
    e.as_string()
        .or_else(|| {
            Reflect::get(&e, &"message".into())
                .ok()
                .and_then(|m| m.as_string())
        })
        .unwrap_or_else(|| "erreur réseau".into())
}

/// Requête vers `/api/v1<route>` ; le corps est du JSON, jamais l'adresse (aucune donnée dans
/// l'URL).
async fn call(method: &str, route: &str, body: Option<String>, accept: &str) -> Result<Response> {
    let opts = RequestInit::new();
    opts.set_method(method);
    opts.set_credentials(RequestCredentials::SameOrigin);
    if let Some(b) = &body {
        opts.set_body(&JsValue::from_str(b));
    }
    let url = format!("{}{route}", api::PREFIX);
    let req = Request::new_with_str_and_init(&url, &opts).map_err(js)?;
    let headers = req.headers();
    headers.set("Accept", accept).map_err(js)?;
    if body.is_some() {
        headers
            .set("Content-Type", "application/json")
            .map_err(js)?;
    }
    let window = web_sys::window().ok_or("pas de fenêtre")?;
    let resp: Response = JsFuture::from(window.fetch_with_request(&req))
        .await
        .map_err(js)?
        .dyn_into()
        .map_err(js)?;
    Ok(resp)
}

async fn text(resp: &Response) -> Result<String> {
    let t = JsFuture::from(resp.text().map_err(js)?).await.map_err(js)?;
    Ok(t.as_string().unwrap_or_default())
}

/// Erreur lisible d'une réponse en échec (`{"error": …}` du serveur).
async fn failure(resp: &Response) -> String {
    let body = text(resp).await.unwrap_or_default();
    serde_json::from_str::<api::ApiError>(&body)
        .map(|e| e.error)
        .unwrap_or_else(|_| format!("HTTP {}", resp.status()))
}

/// État du serveur ; `Ok(None)` s'il faut s'authentifier.
pub async fn status() -> Result<Option<api::Status>> {
    let resp = call("GET", api::routes::STATUS, None, "application/json").await?;
    match resp.status() {
        401 => Ok(None),
        200 => serde_json::from_str(&text(&resp).await?)
            .map(Some)
            .map_err(|e| e.to_string()),
        _ => Err(failure(&resp).await),
    }
}

/// Traduction : sources proposées ou décidées.
pub async fn translation() -> Result<api::TranslationView> {
    let resp = call("GET", api::routes::TRANSLATION, None, "application/json").await?;
    if !resp.ok() {
        return Err(failure(&resp).await);
    }
    serde_json::from_str(&text(&resp).await?).map_err(|e| e.to_string())
}

/// Réponse à une proposition de traduction.
pub async fn answer_translation(spec: &str, accept: bool) -> Result<api::TranslationView> {
    let body = serde_json::to_string(&api::TranslationRequest {
        spec: Some(spec.to_owned()),
        accept: Some(accept),
        device: None,
    })
    .map_err(|e| e.to_string())?;
    let resp = call(
        "POST",
        api::routes::TRANSLATION,
        Some(body),
        "application/json",
    )
    .await?;
    if !resp.ok() {
        return Err(failure(&resp).await);
    }
    serde_json::from_str(&text(&resp).await?).map_err(|e| e.to_string())
}

/// Échange le jeton contre un cookie de session ; le jeton ne reste pas dans la page.
pub async fn login(token: &str) -> Result<()> {
    let body = serde_json::to_string(&api::SessionRequest {
        token: token.to_owned(),
    })
    .map_err(|e| e.to_string())?;
    let resp = call("POST", api::routes::SESSION, Some(body), "application/json").await?;
    if resp.ok() {
        Ok(())
    } else if resp.status() == 401 {
        Err("jeton refusé".into())
    } else {
        Err(failure(&resp).await)
    }
}

/// Recherche en flux : `on_event` reçoit chaque évènement dès son arrivée.
pub async fn search(query: &str, mut on_event: impl FnMut(api::SearchEvent)) -> Result<()> {
    let body = serde_json::to_string(&api::SearchRequest {
        query: query.to_owned(),
        limit: 20,
    })
    .map_err(|e| e.to_string())?;
    let resp = call("POST", api::routes::SEARCH, Some(body), api::NDJSON).await?;
    if !resp.ok() {
        return Err(failure(&resp).await);
    }
    let stream = resp.body().ok_or("réponse sans corps")?;
    let reader: ReadableStreamDefaultReader = stream.get_reader().unchecked_into();
    let mut buf: Vec<u8> = Vec::new();
    loop {
        let chunk = JsFuture::from(reader.read()).await.map_err(js)?;
        if Reflect::get(&chunk, &"done".into())
            .ok()
            .and_then(|d| d.as_bool())
            .unwrap_or(true)
        {
            break;
        }
        let value: Uint8Array = Reflect::get(&chunk, &"value".into())
            .map_err(js)?
            .unchecked_into();
        buf.extend(value.to_vec());
        // Lignes complètes : un évènement chacune.
        while let Some(end) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=end).collect();
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            match serde_json::from_slice(&line) {
                Ok(event) => on_event(event),
                Err(e) => return Err(e.to_string()),
            }
        }
    }
    Ok(())
}
