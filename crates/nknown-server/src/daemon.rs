//! Démon local de l'utilisateur (service systemd `--user`) : `nknownd`, ou `nknown daemon`.
//!
//! Sert l'API sur un socket Unix réservé à l'utilisateur (`$XDG_RUNTIME_DIR/nknown/`), télécharge
//! les modèles au premier lancement, collecte les sources configurées et réindexe les dossiers
//! surveillés. Données et configuration dans le profil (XDG). Aucun port réseau ouvert.

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use crate::{AppState, Auth, router};
use nknown_service::{Options, Service};

/// Point d'entrée du démon (`nknownd`, ou `nknown daemon`). `--socket` affiche le chemin du
/// socket et s'arrête.
pub fn main() -> std::process::ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("erreur : {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run()) {
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
    // Le dossier doit être à nous : dans un /tmp partagé, un autre utilisateur aurait pu le
    // créer d'avance pour intercepter les requêtes.
    {
        use std::os::unix::fs::MetadataExt;
        let me = std::fs::metadata("/proc/self")?.uid();
        let owner = std::fs::symlink_metadata(dir)?.uid();
        if owner != me || std::fs::symlink_metadata(dir)?.file_type().is_symlink() {
            return Err(format!(
                "{} n'appartient pas à l'utilisateur : refusé",
                dir.display()
            )
            .into());
        }
    }
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
