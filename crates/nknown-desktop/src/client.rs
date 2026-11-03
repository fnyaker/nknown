//! Moteur de l'app : le démon local (socket Unix, défaut) ou, s'il est arrêté, le cœur embarqué
//! dans le processus. L'app ne connaît que l'API (`nknown-api`) : les deux se valent.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use nknown_api as api;
use nknown_service::{Options, Service};

pub enum Engine {
    /// `nknownd` : partagé avec les autres clients (KRunner, ligne de commande).
    Daemon(PathBuf),
    /// Cœur dans l'app, quand le démon est désactivé.
    Embedded(Arc<Service>),
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Daemon(p) => write!(f, "Daemon({})", p.display()),
            Self::Embedded(_) => f.write_str("Embedded"),
        }
    }
}

pub type Result<T> = std::result::Result<T, String>;

impl Engine {
    /// Le démon s'il répond, sinon le cœur embarqué (qui télécharge ses modèles au besoin et
    /// collecte les sources comme le ferait le démon).
    pub fn connect() -> Result<Self> {
        let socket = nknown_service::config::socket_path();
        let daemon = Self::Daemon(socket);
        if daemon.status().is_ok() {
            return Ok(daemon);
        }
        let service = Service::open(Options::user().map_err(|e| e.to_string())?)
            .map_err(|e| format!("cœur embarqué : {e}"))?;
        let service = Arc::new(service);
        nknown_service::watch::spawn(service.clone());
        Ok(Self::Embedded(service))
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Daemon(_) => "daemon",
            Self::Embedded(_) => "embedded",
        }
    }

    pub fn status(&self) -> Result<api::Status> {
        match self {
            Self::Daemon(socket) => {
                let body = request(socket, "GET", api::routes::STATUS, None)?.read_all()?;
                serde_json::from_slice(&body).map_err(|e| e.to_string())
            }
            Self::Embedded(s) => s.status().map_err(|e| e.to_string()),
        }
    }

    /// Recherche ; `on_event` reçoit chaque évènement du flux (progression, carte, résultat).
    pub fn search(&self, query: &str, on_event: &mut dyn FnMut(api::SearchEvent)) -> Result<()> {
        let req = api::SearchRequest {
            query: query.to_owned(),
            limit: 20,
        };
        match self {
            Self::Daemon(socket) => {
                let body = serde_json::to_vec(&req).map_err(|e| e.to_string())?;
                let mut resp = request(socket, "POST", api::routes::SEARCH, Some(&body))?;
                let mut line = Vec::new();
                loop {
                    line.clear();
                    if resp.read_line(&mut line)? == 0 {
                        return Ok(());
                    }
                    if line.iter().all(u8::is_ascii_whitespace) {
                        continue;
                    }
                    let event = serde_json::from_slice(&line).map_err(|e| e.to_string())?;
                    on_event(event);
                }
            }
            Self::Embedded(s) => s
                .search(&req, on_event)
                .map(|_| ())
                .map_err(|e| e.to_string()),
        }
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

    /// Une ligne du corps (NDJSON), fin de ligne comprise ; 0 à la fin du corps.
    fn read_line(&mut self, line: &mut Vec<u8>) -> Result<usize> {
        let mut byte = [0u8; 1];
        loop {
            if self.read_body(&mut byte)? == 0 {
                return Ok(line.len());
            }
            line.push(byte[0]);
            if byte[0] == b'\n' {
                return Ok(line.len());
            }
        }
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
