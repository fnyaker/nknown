//! Une seule fenêtre : un second lancement (depuis KRunner, le menu, la ligne de commande)
//! transmet sa demande à l'app déjà ouverte puis s'arrête.
//!
//! Socket `$XDG_RUNTIME_DIR/nknown/app.sock` (dossier 0700) ; une ligne JSON par demande.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Ce que demande un lancement : une recherche, un nœud à mettre en valeur.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub query: Option<String>,
    pub select: Option<String>,
}

impl Request {
    /// `--query <texte>`, `--select <nœud>` ; le reste est ignoré.
    pub fn from_args(args: impl IntoIterator<Item = String>) -> Self {
        let mut req = Self::default();
        let mut args = args.into_iter();
        while let Some(a) = args.next() {
            match a.as_str() {
                "--query" => req.query = args.next().filter(|q| !q.trim().is_empty()),
                "--select" => req.select = args.next(),
                _ => {}
            }
        }
        req
    }

    pub fn is_empty(&self) -> bool {
        self.query.is_none() && self.select.is_none()
    }
}

fn socket_path() -> PathBuf {
    nknown_service::config::socket_path().with_file_name("app.sock")
}

/// Transmet `req` à une app déjà ouverte ; `false` si aucune ne répond.
pub fn forward(req: &Request) -> bool {
    let Ok(mut stream) = UnixStream::connect(socket_path()) else {
        return false;
    };
    let line = serde_json::to_string(req).unwrap_or_default();
    stream
        .write_all(line.as_bytes())
        .and_then(|()| stream.write_all(b"\n"))
        .is_ok()
}

/// Écoute les demandes des lancements suivants ; `on_request` est appelé dans un fil à part.
pub fn listen(on_request: impl Fn(Request) + Send + 'static) {
    let path = socket_path();
    if let Some(dir) = path.parent() {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    // Socket d'une app arrêtée brutalement (aucune ne répond : `forward` a échoué).
    let _ = std::fs::remove_file(&path);
    let Ok(listener) = UnixListener::bind(&path) else {
        return;
    };
    std::thread::Builder::new()
        .name("nknown-instance".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut line = String::new();
                if BufReader::new(stream).read_line(&mut line).is_ok()
                    && let Ok(req) = serde_json::from_str::<Request>(&line)
                {
                    on_request(req);
                }
            }
        })
        .expect("fil d'instance");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments() {
        let r = Request::from_args(
            ["--query", "chat noir", "--select", "d12", "--inconnu"].map(String::from),
        );
        assert_eq!(r.query.as_deref(), Some("chat noir"));
        assert_eq!(r.select.as_deref(), Some("d12"));
        assert!(Request::from_args(["--query", " "].map(String::from)).is_empty());
    }
}
