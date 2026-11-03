//! `nknown-server` : serveur distant (TCP), protégé par jeton, avec l'interface web.
//!
//! Variables d'environnement :
//! - `NKNOWN_DATA` : dossier des données (base, modèles, `config.toml`, jeton généré) ;
//! - `NKNOWN_ADDR` : adresse d'écoute (défaut `0.0.0.0:8080`) ;
//! - `NKNOWN_TOKEN` : jeton d'accès ; sinon lu dans `<données>/token`, généré au besoin ;
//! - `NKNOWN_WEB` : dossier de l'interface web compilée ;
//! - `NKNOWN_INSECURE_COOKIE=1` : cookie sans `Secure`, pour un accès en HTTP clair (réseau
//!   local de confiance seulement ; préférer un mandataire TLS).

use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use nknown_server::{AppState, Auth, router};
use nknown_service::{Config, Options, Service};

#[tokio::main]
async fn main() -> std::process::ExitCode {
    match run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("erreur : {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let data = nknown_service::config::data_dir();
    std::fs::create_dir_all(&data)?;
    let config_path = data.join("config.toml");
    let config = Config {
        // Pas de dossier personnel par défaut sur un serveur.
        sources: vec![],
        watch_files: false,
        ..Config::default()
    };
    let config = if config_path.exists() {
        Config::load(&config_path)?
    } else {
        config
    };
    let token = token(&data)?;
    let service = tokio::task::spawn_blocking(move || {
        Service::open(Options {
            data_dir: data,
            config,
            config_path: Some(config_path),
        })
    })
    .await??;
    let service = Arc::new(service);
    nknown_service::watch::spawn(service.clone());

    let web = std::env::var_os("NKNOWN_WEB")
        .map(PathBuf::from)
        .filter(|d| d.join("index.html").is_file());
    if web.is_none() {
        eprintln!("interface web absente (NKNOWN_WEB) : API seule");
    }
    let secure = std::env::var("NKNOWN_INSECURE_COOKIE").as_deref() != Ok("1");
    let app = router(
        AppState {
            service,
            auth: Arc::new(Auth::token(token, secure)),
        },
        web,
    );
    let addr = std::env::var("NKNOWN_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    eprintln!("nknown-server à l'écoute sur {addr}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}

/// Jeton : `NKNOWN_TOKEN`, sinon `<données>/token` (0600), créé au premier démarrage.
fn token(data: &Path) -> std::io::Result<String> {
    if let Ok(t) = std::env::var("NKNOWN_TOKEN")
        && !t.trim().is_empty()
    {
        return Ok(t.trim().to_owned());
    }
    let path = data.join("token");
    match std::fs::read_to_string(&path) {
        Ok(t) if !t.trim().is_empty() => return Ok(t.trim().to_owned()),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let t = nknown_server::auth::random_hex(32);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)?;
    std::io::Write::write_all(&mut f, t.as_bytes())?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    eprintln!("jeton d'accès généré dans {}", path.display());
    Ok(t)
}

async fn shutdown() {
    let ctrl_c = tokio::signal::ctrl_c();
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("signal SIGTERM");
    tokio::select! {
        _ = ctrl_c => {},
        _ = term.recv() => {},
    }
}
