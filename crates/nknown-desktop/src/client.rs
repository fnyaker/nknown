//! Moteur de l'app : le démon local (socket Unix), le cœur intégré au processus, ou un serveur
//! distant (HTTPS, jeton). L'app ne connaît que l'API (`nknown-api`) : les trois se valent.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use nknown_api as api;
use nknown_service::{Options, Service};

use crate::settings::{Mode, Settings};

pub enum Engine {
    /// `nknownd` : partagé avec les autres clients (KRunner, ligne de commande).
    Daemon(PathBuf),
    /// Cœur dans l'app, quand le démon est désactivé.
    Embedded(Arc<Service>),
    /// `nknown-server` distant.
    Remote(Remote),
}

pub struct Remote {
    base: String,
    token: String,
    agent: ureq::Agent,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Daemon(p) => write!(f, "Daemon({})", p.display()),
            Self::Embedded(_) => f.write_str("Embedded"),
            Self::Remote(r) => write!(f, "Remote({})", r.base),
        }
    }
}

pub type Result<T> = std::result::Result<T, String>;

/// Le démon, démarré par systemd s'il est installé mais arrêté.
fn daemon_now() -> Option<Engine> {
    let daemon = Engine::Daemon(nknown_service::config::socket_path());
    if daemon.status().is_ok() {
        return Some(daemon);
    }
    let started = std::process::Command::new("systemctl")
        .args(["--user", "start", "nknownd.service"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if started {
        for _ in 0..30 {
            std::thread::sleep(Duration::from_millis(200));
            if daemon.status().is_ok() {
                return Some(daemon);
            }
        }
    }
    None
}

fn embedded() -> Result<Engine> {
    let service = Service::open(Options::user().map_err(|e| e.to_string())?)
        .map_err(|e| format!("cœur intégré : {e} (le démon utilise-t-il déjà la base ?)"))?;
    let service = Arc::new(service);
    nknown_service::watch::spawn(service.clone());
    Ok(Engine::Embedded(service))
}

impl Engine {
    /// Le moteur choisi dans les réglages ; en automatique, le démon (démarré au besoin), sinon
    /// le cœur intégré (qui télécharge ses modèles et collecte les sources comme le démon).
    pub fn connect(settings: &Settings) -> Result<Self> {
        match settings.engine {
            Mode::Auto => match daemon_now() {
                Some(d) => Ok(d),
                None => embedded(),
            },
            Mode::Daemon => daemon_now().ok_or_else(|| "démon injoignable".to_owned()),
            Mode::Embedded => embedded(),
            Mode::Remote => {
                let base = settings.remote_url.trim().trim_end_matches('/').to_owned();
                if !(base.starts_with("https://") || base.starts_with("http://")) {
                    return Err("adresse du serveur invalide".into());
                }
                let token = crate::secrets::token(&base)?;
                let agent: ureq::Agent = ureq::Agent::config_builder()
                    .timeout_connect(Some(Duration::from_secs(10)))
                    .timeout_recv_response(Some(Duration::from_secs(60)))
                    .http_status_as_error(false)
                    .build()
                    .into();
                let remote = Self::Remote(Remote { base, token, agent });
                remote.status()?;
                Ok(remote)
            }
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Daemon(_) => "daemon",
            Self::Embedded(_) => "embedded",
            Self::Remote(_) => "remote",
        }
    }

    pub fn status(&self) -> Result<api::Status> {
        match self {
            Self::Daemon(socket) => {
                let body = request(socket, "GET", api::routes::STATUS, None)?.read_all()?;
                serde_json::from_slice(&body).map_err(|e| e.to_string())
            }
            Self::Embedded(s) => s.status().map_err(|e| e.to_string()),
            Self::Remote(r) => r.json("GET", api::routes::STATUS, None),
        }
    }

    pub fn sources(&self) -> Result<Vec<api::SourceView>> {
        match self {
            Self::Daemon(socket) => {
                let body = request(socket, "GET", api::routes::SOURCES, None)?.read_all()?;
                serde_json::from_slice(&body).map_err(|e| e.to_string())
            }
            Self::Embedded(s) => Ok(s.sources()),
            Self::Remote(r) => r.json("GET", api::routes::SOURCES, None),
        }
    }

    /// Ajoute une source (dossier, `rss:…`, `wiki:…`) ; sa première collecte démarre aussitôt.
    pub fn add_source(&self, spec: &str) -> Result<api::SourceView> {
        let body = serde_json::to_vec(&api::AddSource {
            spec: spec.to_owned(),
        })
        .map_err(|e| e.to_string())?;
        match self {
            Self::Daemon(socket) => {
                let body =
                    request(socket, "POST", api::routes::SOURCES, Some(&body))?.read_all()?;
                serde_json::from_slice(&body).map_err(|e| e.to_string())
            }
            Self::Embedded(s) => {
                let view = s.add_source(spec).map_err(|e| e.to_string())?;
                let (s, spec) = (s.clone(), view.spec.clone());
                std::thread::spawn(move || {
                    let _ = s.collect(&spec);
                });
                Ok(view)
            }
            Self::Remote(r) => r.json("POST", api::routes::SOURCES, Some(&body)),
        }
    }

    pub fn remove_source(&self, id: &str) -> Result<()> {
        let route = format!("{}/{id}", api::routes::SOURCES);
        match self {
            Self::Daemon(socket) => request(socket, "DELETE", &route, None).map(drop),
            Self::Embedded(s) => s.remove_source(id).map(drop).map_err(|e| e.to_string()),
            Self::Remote(r) => r.call("DELETE", &route, None).map(drop),
        }
    }

    pub fn settings(&self) -> Result<api::EngineSettings> {
        match self {
            Self::Daemon(socket) => {
                let body = request(socket, "GET", api::routes::SETTINGS, None)?.read_all()?;
                serde_json::from_slice(&body).map_err(|e| e.to_string())
            }
            Self::Embedded(s) => Ok(s.settings()),
            Self::Remote(r) => r.json("GET", api::routes::SETTINGS, None),
        }
    }

    pub fn set_settings(&self, settings: api::EngineSettings) -> Result<api::EngineSettings> {
        let body = serde_json::to_vec(&settings).map_err(|e| e.to_string())?;
        match self {
            Self::Daemon(socket) => {
                let body =
                    request(socket, "PUT", api::routes::SETTINGS, Some(&body))?.read_all()?;
                serde_json::from_slice(&body).map_err(|e| e.to_string())
            }
            Self::Embedded(s) => s.update_settings(settings).map_err(|e| e.to_string()),
            Self::Remote(r) => r.json("PUT", api::routes::SETTINGS, Some(&body)),
        }
    }

    /// Un document entier (lecteur intégré).
    pub fn document(&self, id: u64) -> Result<api::DocumentView> {
        let route = format!("{}/{id}", api::routes::DOCUMENTS);
        match self {
            Self::Daemon(socket) => {
                let body = request(socket, "GET", &route, None)?.read_all()?;
                serde_json::from_slice(&body).map_err(|e| e.to_string())
            }
            Self::Embedded(s) => s
                .document(id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "document introuvable".into()),
            Self::Remote(r) => r.json("GET", &route, None),
        }
    }

    /// Recherche ; `on_event` reçoit chaque évènement du flux (progression, carte, résultat).
    pub fn search(&self, query: &str, on_event: &mut dyn FnMut(api::SearchEvent)) -> Result<()> {
        let req = api::SearchRequest {
            query: query.to_owned(),
            limit: 20,
        };
        let body = serde_json::to_vec(&req).map_err(|e| e.to_string())?;
        let mut lines: Box<dyn BufRead> = match self {
            Self::Daemon(socket) => {
                let resp = request(socket, "POST", api::routes::SEARCH, Some(&body))?;
                Box::new(BufReader::new(resp))
            }
            Self::Embedded(s) => {
                return s
                    .search(&req, on_event)
                    .map(|_| ())
                    .map_err(|e| e.to_string());
            }
            Self::Remote(r) => Box::new(BufReader::new(r.call(
                "POST",
                api::routes::SEARCH,
                Some(&body),
            )?)),
        };
        let mut line = String::new();
        loop {
            line.clear();
            if lines.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
                return Ok(());
            }
            if line.trim().is_empty() {
                continue;
            }
            let event = serde_json::from_str(&line).map_err(|e| e.to_string())?;
            on_event(event);
        }
    }
}

impl Remote {
    /// Requête au serveur : le jeton va dans l'en-tête, jamais dans l'adresse.
    fn call(&self, method: &str, route: &str, body: Option<&[u8]>) -> Result<Box<dyn Read + Send>> {
        let url = format!("{}{}{route}", self.base, api::PREFIX);
        let auth = format!("Bearer {}", self.token);
        let resp = match (method, body) {
            ("POST", Some(b)) => self
                .agent
                .post(&url)
                .header("Authorization", &auth)
                .header("Content-Type", "application/json")
                .header("Accept", api::NDJSON)
                .send(b),
            ("PUT", Some(b)) => self
                .agent
                .put(&url)
                .header("Authorization", &auth)
                .header("Content-Type", "application/json")
                .send(b),
            ("DELETE", _) => self
                .agent
                .delete(&url)
                .header("Authorization", &auth)
                .call(),
            _ => self.agent.get(&url).header("Authorization", &auth).call(),
        }
        .map_err(|e| format!("serveur : {e}"))?;
        let code = resp.status().as_u16();
        let body = resp.into_body().into_reader();
        if code >= 400 {
            let mut text = Vec::new();
            let _ = body.take(64 * 1024).read_to_end(&mut text);
            let message = serde_json::from_slice::<api::ApiError>(&text)
                .map(|e| e.error)
                .unwrap_or_else(|_| format!("HTTP {code}"));
            return Err(if code == 401 {
                "jeton refusé par le serveur".into()
            } else {
                message
            });
        }
        Ok(Box::new(body))
    }

    fn json<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        route: &str,
        body: Option<&[u8]>,
    ) -> Result<T> {
        let mut out = Vec::new();
        self.call(method, route, body)?
            .read_to_end(&mut out)
            .map_err(|e| e.to_string())?;
        serde_json::from_slice(&out).map_err(|e| e.to_string())
    }
}

/// Réponse HTTP en cours de lecture : corps brut ou découpé en morceaux (`chunked`).
struct Response {
    reader: BufReader<UnixStream>,
    chunked: bool,
    /// Octets restants dans le morceau courant.
    left: usize,
    done: bool,
}

impl Read for Response {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.read_body(buf).map_err(std::io::Error::other)
    }
}

impl Response {
    fn read_all(mut self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            let n = self.read_body(&mut buf)?;
            if n == 0 {
                return Ok(out);
            }
            out.extend_from_slice(&buf[..n]);
        }
    }

    fn read_body(&mut self, buf: &mut [u8]) -> Result<usize> {
        if self.done {
            return Ok(0);
        }
        if !self.chunked {
            return self.reader.read(buf).map_err(|e| e.to_string());
        }
        if self.left == 0 {
            let mut size = String::new();
            self.reader
                .read_line(&mut size)
                .map_err(|e| e.to_string())?;
            let size = size.trim();
            if size.is_empty() {
                // Fin de ligne d'un morceau précédent.
                return self.read_body(buf);
            }
            let n = usize::from_str_radix(size.split(';').next().unwrap_or("0"), 16)
                .map_err(|e| format!("morceau HTTP : {e}"))?;
            if n == 0 {
                self.done = true;
                return Ok(0);
            }
            self.left = n;
        }
        let want = buf.len().min(self.left);
        let n = self
            .reader
            .read(&mut buf[..want])
            .map_err(|e| e.to_string())?;
        if n == 0 {
            self.done = true;
        }
        self.left -= n;
        Ok(n)
    }
}

fn request(
    socket: &std::path::Path,
    method: &str,
    route: &str,
    body: Option<&[u8]>,
) -> Result<Response> {
    let mut stream = UnixStream::connect(socket).map_err(|e| format!("démon injoignable : {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(120)))
        .map_err(|e| e.to_string())?;
    let body = body.unwrap_or_default();
    let head = format!(
        "{method} {}{route} HTTP/1.1\r\nHost: nknownd\r\nContent-Type: application/json\r\n\
         Accept: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        api::PREFIX,
        api::NDJSON,
        body.len()
    );
    stream
        .write_all(head.as_bytes())
        .and_then(|()| stream.write_all(body))
        .map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream);
    let mut status = String::new();
    reader.read_line(&mut status).map_err(|e| e.to_string())?;
    let code: u16 = status
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| format!("réponse inattendue : {status:?}"))?;
    let mut chunked = false;
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).map_err(|e| e.to_string())?;
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("transfer-encoding")
            && value.trim().eq_ignore_ascii_case("chunked")
        {
            chunked = true;
        }
    }
    let resp = Response {
        reader,
        chunked,
        left: 0,
        done: false,
    };
    if code >= 400 {
        let body = resp.read_all().unwrap_or_default();
        let message = serde_json::from_slice::<api::ApiError>(&body)
            .map(|e| e.error)
            .unwrap_or_else(|_| format!("HTTP {code}"));
        return Err(message);
    }
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    /// Faux démon : répond `body` en morceaux, comme axum pour un flux.
    fn serve(chunks: Vec<&'static str>) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.sock");
        let listener = UnixListener::bind(&path).unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut req = [0u8; 4096];
            let _ = s.read(&mut req).unwrap();
            let mut out = String::from(
                "HTTP/1.1 200 OK\r\ncontent-type: application/x-ndjson\r\ntransfer-encoding: chunked\r\n\r\n",
            );
            for c in chunks {
                out.push_str(&format!("{:x}\r\n{c}\r\n", c.len()));
            }
            out.push_str("0\r\n\r\n");
            s.write_all(out.as_bytes()).unwrap();
        });
        (dir, path)
    }

    #[test]
    fn reads_a_chunked_ndjson_stream() {
        // Une ligne coupée entre deux morceaux, deux lignes dans un même morceau.
        let (_dir, path) = serve(vec![
            "{\"type\":\"phase\",\"pha",
            "se\":\"analyze\"}\n{\"type\":\"phase\",\"phase\":\"search\"}\n",
            "{\"type\":\"error\",\"message\":\"x\"}\n",
        ]);
        let mut events = Vec::new();
        Engine::Daemon(path)
            .search("chat", &mut |e| events.push(e))
            .unwrap();
        assert_eq!(
            events,
            vec![
                api::SearchEvent::Phase {
                    phase: api::Phase::Analyze
                },
                api::SearchEvent::Phase {
                    phase: api::Phase::Search
                },
                api::SearchEvent::Error {
                    message: "x".into()
                },
            ]
        );
    }
}
