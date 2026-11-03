//! `nknownd` : démon local de l'utilisateur (service systemd `--user`).
//!
//! Sert l'API sur un socket Unix réservé à l'utilisateur (`$XDG_RUNTIME_DIR/nknown/`), collecte
//! les sources configurées et réindexe les dossiers surveillés. Données et configuration dans
//! le profil (XDG). Aucun port réseau ouvert.

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use nknown_server::{AppState, Auth, router};
use nknown_service::{Options, Service};

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
    let socket = nknown_service::config::socket_path();
    if std::env::args().any(|a| a == "--socket") {
        println!("{}", socket.display());
        return Ok(());
    }
    let service = tokio::task::spawn_blocking(|| Service::open(Options::user()?)).await??;
    let service = Arc::new(service);
    nknown_service::watch::spawn(service.clone());

    let dir = socket.parent().expect("dossier du socket");
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    // Socket d'une instance précédente arrêtée brutalement.
    if socket.exists() {
        if tokio::net::UnixStream::connect(&socket).await.is_ok() {
            return Err(format!("nknownd tourne déjà ({})", socket.display()).into());
        }
        std::fs::remove_file(&socket)?;
    }
    let listener = tokio::net::UnixListener::bind(&socket)?;
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
    eprintln!("nknownd à l'écoute sur {}", socket.display());

    let app = router(
        AppState {
            service,
            auth: Arc::new(Auth::None),
        },
        None,
    );
    let served = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown())
        .await;
    let _ = std::fs::remove_file(&socket);
    Ok(served?)
}

async fn shutdown() {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("signal SIGTERM");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {},
        _ = term.recv() => {},
    }
}
