//! Tâches de fond du démon : collecte périodique et réindexation des dossiers surveillés.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use notify::{EventKind, RecursiveMode, Watcher};

use crate::Service;

/// Délai de calme avant de réindexer : un enregistrement produit souvent plusieurs évènements.
const DEBOUNCE: Duration = Duration::from_secs(3);

/// Délai avant un nouvel essai de téléchargement des modèles (hors ligne…).
const MODELS_RETRY: Duration = Duration::from_secs(300);

/// Télécharge au besoin les modèles, puis lance la collecte périodique (une première collecte
/// immédiate) et, si la configuration le demande, la surveillance des dossiers. Rien n'est
/// indexé avant que les modèles soient là : un document indexé sans lemmes le resterait. Les
/// fils tournent jusqu'à la fin du processus.
pub fn spawn(service: Arc<Service>) {
    let svc = service.clone();
    std::thread::Builder::new()
        .name("nknown-collecte".into())
        .spawn(move || {
            while let Err(e) = svc.ensure_models() {
                eprintln!("modèles : {e} ; nouvel essai dans 5 min");
                std::thread::sleep(MODELS_RETRY);
            }
            if svc.config().watch_files {
                let watched = svc.clone();
                std::thread::Builder::new()
                    .name("nknown-surveillance".into())
                    .spawn(move || {
                        if let Err(e) = watch_folders(&watched) {
                            eprintln!("surveillance des dossiers : {e}");
                        }
                    })
                    .expect("fil de surveillance");
            }
            loop {
                for (spec, r) in svc.collect_all() {
                    match r {
                        Ok(s) => eprintln!(
                            "{spec} : {} nouveaux, {} mis à jour, {} inchangés, {} erreurs",
                            s.added, s.updated, s.unchanged, s.errors
                        ),
                        Err(e) => eprintln!("{spec} : {e}"),
                    }
                }
                let every = svc.config().interval().unwrap_or(Duration::from_secs(1800));
                std::thread::sleep(every);
            }
        })
        .expect("fil de collecte");
}

/// Dossiers configurés comme sources.
fn folders(service: &Service) -> Vec<PathBuf> {
    service
        .config()
        .sources
        .iter()
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .collect()
}

fn watch_folders(service: &Service) -> notify::Result<()> {
    let (tx, rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(tx)?;
    let mut watched = folders(service);
    for dir in &watched {
        watcher.watch(dir, RecursiveMode::Recursive)?;
    }
    // Dossier source → échéance de réindexation.
    let mut pending: Vec<(PathBuf, Instant)> = Vec::new();
    loop {
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Ok(event)) => {
                if matches!(
                    event.kind,
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                ) {
                    for path in &event.paths {
                        if let Some(root) = watched.iter().find(|r| path.starts_with(r))
                            && !is_ignored(root, path)
                        {
                            let due = Instant::now() + DEBOUNCE;
                            match pending.iter_mut().find(|(r, _)| r == root) {
                                Some((_, d)) => *d = due,
                                None => pending.push((root.clone(), due)),
                            }
                        }
                    }
                }
            }
            Ok(Err(e)) => eprintln!("surveillance : {e}"),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
        let now = Instant::now();
        let (ready, rest): (Vec<_>, Vec<_>) = pending.into_iter().partition(|(_, d)| *d <= now);
        pending = rest;
        for (root, _) in ready {
            if let Err(e) = service.collect(&root.to_string_lossy()) {
                eprintln!("{} : {e}", root.display());
            }
        }
        // Sources ajoutées ou retirées entre-temps.
        let current = folders(service);
        if current != watched {
            for dir in watched.iter().filter(|d| !current.contains(d)) {
                let _ = watcher.unwatch(dir);
            }
            for dir in current.iter().filter(|d| !watched.contains(d)) {
                watcher.watch(dir, RecursiveMode::Recursive)?;
            }
            watched = current;
        }
    }
}

/// Changement dans un dossier que la collecte ignore de toute façon (`target/`, `.git/`…).
fn is_ignored(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root).is_ok_and(|rel| {
        rel.components().any(|c| {
            let name = c.as_os_str().to_string_lossy();
            name.starts_with('.') || nknown_sources::files::SKIPPED_DIRS.contains(&name.as_ref())
        })
    })
}
