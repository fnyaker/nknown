//! Téléchargement des modèles au premier lancement.
//!
//! La liste des fichiers (taille, SHA-256, groupe) est intégrée au binaire (`models.toml`,
//! écrit par `packaging/models/stage.sh`) ; les fichiers sont des assets d'une release GitHub.
//! Chaque fichier est téléchargé dans `<nom>.part` (repris là où il s'est arrêté), vérifié,
//! puis renommé : un fichier présent sous son nom final est toujours complet et intègre.
//!
//! `NKNOWN_MODELS_URL` remplace l'adresse de la release (miroir, tests).

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::Error;
use crate::config::{Config, ParserPrecision};

const MANIFEST: &str = include_str!("../models.toml");
/// Essais par fichier (attente de 2, 4, 8 s entre deux).
const ATTEMPTS: u32 = 4;

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    /// Adresse de la release : `<release>/<asset>`.
    pub release: String,
    #[serde(rename = "file")]
    pub files: Vec<ModelFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ModelFile {
    /// Chemin dans le dossier des modèles.
    pub path: String,
    /// Nom de l'asset dans la release.
    pub asset: String,
    pub group: Group,
    /// Groupe `translate` : paire de langues du modèle (`en-fr`).
    #[serde(default)]
    pub pair: Option<String>,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Group {
    /// Lexiques et tagueurs : toujours.
    Base,
    /// Analyseur (configuration, tokeniseur) : dès qu'il est activé.
    Parser,
    /// Poids de l'analyseur, précision `standard`.
    Standard,
    /// Poids de l'analyseur, précision `light`.
    Light,
    /// Modèles de traduction : seulement ceux des langues acceptées (voir [`missing_pairs`]).
    Translate,
}

pub fn manifest() -> Manifest {
    let mut m: Manifest = toml::from_str(MANIFEST).expect("models.toml intégré valide");
    if let Ok(url) = std::env::var("NKNOWN_MODELS_URL")
        && !url.trim().is_empty()
    {
        m.release = url.trim().trim_end_matches('/').to_owned();
    }
    m
}

/// Groupes nécessaires à une configuration : la base, et l'analyseur fourni s'il est activé
/// sous son nom par défaut (un analyseur personnel n'est pas téléchargé).
pub fn groups(config: &Config) -> Vec<Group> {
    let mut g = vec![Group::Base];
    if config.parser.as_deref() == Some(crate::config::DEFAULT_PARSER) {
        g.push(Group::Parser);
        g.push(match config.parser_precision {
            ParserPrecision::Standard => Group::Standard,
            ParserPrecision::Light => Group::Light,
        });
    }
    g
}

/// Fichiers des `groups` absents ou de taille inattendue dans `dir`.
pub fn missing(manifest: &Manifest, dir: &Path, groups: &[Group]) -> Vec<ModelFile> {
    manifest
        .files
        .iter()
        .filter(|f| groups.contains(&f.group))
        .filter(|f| std::fs::metadata(dir.join(&f.path)).map(|m| m.len()).ok() != Some(f.size))
        .cloned()
        .collect()
}

/// Fichiers de traduction des paires `pairs` absents ou de taille inattendue dans `dir`.
pub fn missing_pairs(manifest: &Manifest, dir: &Path, pairs: &[&str]) -> Vec<ModelFile> {
    manifest
        .files
        .iter()
        .filter(|f| f.group == Group::Translate)
        .filter(|f| f.pair.as_deref().is_some_and(|p| pairs.contains(&p)))
        .filter(|f| std::fs::metadata(dir.join(&f.path)).map(|m| m.len()).ok() != Some(f.size))
        .cloned()
        .collect()
}

/// Paires de traduction voulues par la configuration (langues acceptées).
pub fn wanted_pairs(config: &Config) -> Vec<&'static str> {
    let mut pairs: Vec<&'static str> = config
        .translate_langs
        .iter()
        .filter_map(|l| nknown_core::lang::Lang::from_code(l))
        .filter_map(crate::translation::chain)
        .flat_map(|c| c.iter().copied())
        .collect();
    pairs.sort_unstable();
    pairs.dedup();
    pairs
}

/// Télécharge `files` dans `dir`. `progress(fait, total)` en octets, sur l'ensemble.
pub fn fetch(
    manifest: &Manifest,
    dir: &Path,
    files: &[ModelFile],
    progress: &mut dyn FnMut(u64, u64),
) -> Result<(), Error> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .timeout_per_read(Some(Duration::from_secs(60)))
        .user_agent(concat!("NKnown/", env!("CARGO_PKG_VERSION")))
        .http_status_as_error(false)
        .build()
        .into();
    let total: u64 = files.iter().map(|f| f.size).sum();
    let mut before = 0;
    for f in files {
        let url = format!("{}/{}", manifest.release, f.asset);
        // Une coupure ne fait pas tout échouer : on reprend là où le fichier s'est arrêté.
        let mut attempt = 0;
        loop {
            match fetch_one(&agent, &url, dir, f, &mut |done| {
                progress(before + done, total)
            }) {
                Ok(()) => break,
                Err(Failure::Network(e)) if attempt + 1 < ATTEMPTS => {
                    attempt += 1;
                    eprintln!(
                        "{} : {e} ; nouvel essai ({attempt}/{})",
                        f.path,
                        ATTEMPTS - 1
                    );
                    std::thread::sleep(Duration::from_secs(2u64.pow(attempt)));
                }
                Err(Failure::Network(e) | Failure::Fatal(e)) => return Err(e),
            }
        }
        before += f.size;
        progress(before, total);
    }
    Ok(())
}

fn part_path(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    target.with_file_name(name)
}

/// Une erreur réseau se réessaie ; un contenu faux ou une erreur disque, non.
enum Failure {
    Network(Error),
    Fatal(Error),
}

impl From<std::io::Error> for Failure {
    fn from(e: std::io::Error) -> Self {
        Self::Fatal(e.into())
    }
}

fn fetch_one(
    agent: &ureq::Agent,
    url: &str,
    dir: &Path,
    f: &ModelFile,
    progress: &mut dyn FnMut(u64),
) -> Result<(), Failure> {
    let network =
        |e: &dyn std::fmt::Display| Failure::Network(Error::Invalid(format!("{url} : {e}")));
    let target = dir.join(&f.path);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let part = part_path(&target);
    let mut have = std::fs::metadata(&part).map_or(0, |m| m.len());
    if have > f.size {
        std::fs::remove_file(&part)?;
        have = 0;
    }
    if have < f.size {
        let mut req = agent.get(url);
        if have > 0 {
            req = req.header("Range", &format!("bytes={have}-"));
        }
        let resp = req.call().map_err(|e| network(&e))?;
        let status = resp.status().as_u16();
        let resumed = match status {
            206 => true,
            200 => false,
            _ => return Err(network(&format!("HTTP {status}"))),
        };
        if !resumed {
            have = 0;
        }
        let mut out = OpenOptions::new()
            .create(true)
            .write(true)
            .append(resumed)
            .truncate(!resumed)
            .open(&part)?;
        let mut body = resp.into_body().into_reader();
        let mut buf = vec![0u8; 1 << 16];
        loop {
            let n = body.read(&mut buf).map_err(|e| network(&e))?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])?;
            have += n as u64;
            if have > f.size {
                break;
            }
            progress(have);
        }
        out.sync_all()?;
    }
    if have != f.size || sha256_of(&part).map_err(Failure::Fatal)? != f.sha256 {
        // Fichier corrompu ou d'une autre version : on repartira de zéro.
        std::fs::remove_file(&part)?;
        return Err(Failure::Fatal(Error::Invalid(format!(
            "{} : contenu inattendu (taille ou SHA-256)",
            f.path
        ))));
    }
    std::fs::rename(&part, &target)?;
    Ok(())
}

fn sha256_of(path: &Path) -> Result<String, Error> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_manifest_is_complete() {
        let m = manifest();
        assert!(m.release.starts_with("https://"));
        for g in [Group::Base, Group::Parser, Group::Standard, Group::Light] {
            assert!(m.files.iter().any(|f| f.group == g), "{g:?}");
        }
        assert!(m.files.iter().all(|f| f.sha256.len() == 64 && f.size > 0));
        assert!(m.files.iter().all(|f| !f.asset.contains('/')));
    }

    #[test]
    fn translation_models_are_wanted_per_accepted_language() {
        let m = manifest();
        let mut c = Config::default();
        assert!(wanted_pairs(&c).is_empty());
        c.translate_langs = vec!["zho".into()];
        // Le chinois passe par l'anglais.
        assert_eq!(wanted_pairs(&c), ["en-fr", "zh-en"]);
        let dir = tempfile::tempdir().unwrap();
        let files = missing_pairs(&m, dir.path(), &wanted_pairs(&c));
        assert_eq!(files.len(), 8, "{files:?}");
        assert!(files.iter().all(|f| f.group == Group::Translate));
        // Chaque langue traduisible a tous ses fichiers au manifeste.
        for lang in crate::translation::languages() {
            for pair in crate::translation::chain(lang).unwrap() {
                assert_eq!(missing_pairs(&m, dir.path(), &[pair]).len(), 4, "{pair}");
            }
        }
        // Les modèles de traduction ne font pas partie de la base.
        assert!(
            missing(&m, dir.path(), &groups(&c))
                .iter()
                .all(|f| f.group != Group::Translate)
        );
    }

    #[test]
    fn groups_follow_the_configuration() {
        let mut c = Config {
            parser: Some(crate::config::DEFAULT_PARSER.into()),
            ..Config::default()
        };
        assert_eq!(groups(&c), [Group::Base, Group::Parser, Group::Standard]);
        c.parser_precision = ParserPrecision::Light;
        assert_eq!(groups(&c), [Group::Base, Group::Parser, Group::Light]);
        c.parser = Some("/mon/analyseur".into());
        assert_eq!(groups(&c), [Group::Base]);
    }

    /// Serveur HTTP minimal : sert `body`, en reprise si on demande une plage.
    fn serve(body: Vec<u8>, requests: usize) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(requests) {
                let mut s = stream.unwrap();
                let mut req = [0u8; 2048];
                let n = s.read(&mut req).unwrap();
                let req = String::from_utf8_lossy(&req[..n]).to_lowercase();
                let from = req
                    .lines()
                    .find_map(|l| l.strip_prefix("range: bytes="))
                    .and_then(|r| {
                        r.trim_end_matches('-')
                            .trim()
                            .trim_end_matches('-')
                            .parse()
                            .ok()
                    })
                    .unwrap_or(0usize);
                let (status, part) = if from > 0 {
                    ("206 Partial Content", &body[from..])
                } else {
                    ("200 OK", &body[..])
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    part.len()
                );
                s.write_all(head.as_bytes()).unwrap();
                s.write_all(part).unwrap();
            }
        });
        format!("http://{addr}")
    }

    #[test]
    fn download_resumes_and_verifies() {
        let body: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let sha = {
            let mut h = Sha256::new();
            h.update(&body);
            h.finalize()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        };
        let file = ModelFile {
            path: "sub/model.bin".into(),
            asset: "sub--model.bin".into(),
            group: Group::Base,
            pair: None,
            size: body.len() as u64,
            sha256: sha,
        };
        let dir = tempfile::tempdir().unwrap();
        // Un téléchargement interrompu a laissé le début du fichier.
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/model.bin.part"), &body[..70_000]).unwrap();
        let m = Manifest {
            release: serve(body.clone(), 1),
            files: vec![file.clone()],
        };
        assert_eq!(missing(&m, dir.path(), &[Group::Base]).len(), 1);
        let mut last = 0;
        fetch(&m, dir.path(), &m.files, &mut |done, total| {
            assert!(done <= total);
            last = done;
        })
        .unwrap();
        assert_eq!(last, body.len() as u64);
        assert_eq!(
            std::fs::read(dir.path().join("sub/model.bin")).unwrap(),
            body
        );
        assert!(!dir.path().join("sub/model.bin.part").exists());
        assert!(missing(&m, dir.path(), &[Group::Base]).is_empty());

        // Contenu différent de celui annoncé : refusé, rien n'est installé.
        let mut wrong = file;
        wrong.path = "autre.bin".into();
        wrong.sha256 = "0".repeat(64);
        let m = Manifest {
            release: serve(body, 1),
            files: vec![wrong],
        };
        assert!(fetch(&m, dir.path(), &m.files, &mut |_, _| {}).is_err());
        assert!(!dir.path().join("autre.bin").exists());
        assert!(!dir.path().join("autre.bin.part").exists());
    }
}
