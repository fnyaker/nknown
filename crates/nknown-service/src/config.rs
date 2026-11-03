//! Configuration et emplacements, dans le profil de l'utilisateur (XDG), jamais dans `/etc`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::Error;

/// `$XDG_DATA_HOME/nknown` (défaut `~/.local/share/nknown`), ou `$NKNOWN_DATA`.
pub fn data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("NKNOWN_DATA") {
        return PathBuf::from(d);
    }
    xdg("XDG_DATA_HOME", ".local/share").join("nknown")
}

/// `$XDG_CONFIG_HOME/nknown/config.toml` (défaut `~/.config/nknown/config.toml`).
pub fn config_path() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config")
        .join("nknown")
        .join("config.toml")
}

/// `$XDG_RUNTIME_DIR/nknown/nknownd.sock` : socket du démon local.
pub fn socket_path() -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    runtime.join("nknown").join("nknownd.sock")
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    match std::env::var_os(var) {
        Some(d) if Path::new(&d).is_absolute() => PathBuf::from(d),
        _ => home().join(fallback),
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Analyseur fourni (téléchargé au premier lancement avec les autres modèles).
pub const DEFAULT_PARSER: &str = "parser-multi";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Sources collectées (dossiers, `rss:…`, `wiki:…`).
    pub sources: Vec<String>,
    /// Intervalle entre deux collectes complètes (`30m`, `2h`, `90s`).
    pub collect_every: String,
    /// Surveiller les dossiers et réindexer un fichier dès qu'il change.
    pub watch_files: bool,
    /// Dossier des lexiques et tagueurs (`<langue>.lex`, `<langue>.tag`) ; défaut :
    /// `<données>/models`.
    pub models: Option<PathBuf>,
    /// Analyseur neuronal : dossier (CPU) ou `vulkan:<dossier>` (GPU), relatif au dossier des
    /// modèles. Par défaut l'analyseur fourni ; aucun avec `parser = ""`.
    pub parser: Option<String>,
    /// Précision de l'analyseur sur CPU.
    pub parser_precision: ParserPrecision,
}

/// Précision de l'analyseur sur CPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParserPrecision {
    /// Table des plongements en 4 bits, couches en 8 bits : précision intacte (~215 Mo).
    #[default]
    Standard,
    /// Tout en 4 bits : ~50 Mo de moins, environ un point d'exactitude en moins.
    Light,
}

impl ParserPrecision {
    /// Spécification de modèle correspondante (`light:` devant un dossier ; le reste inchangé).
    pub fn apply(self, spec: &str) -> String {
        let plain = !spec.contains(':') || spec.starts_with('/');
        match self {
            Self::Light if plain => format!("light:{spec}"),
            _ => spec.to_owned(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        // Par défaut : les documents de l'utilisateur, s'ils existent.
        let documents = home().join("Documents");
        Self {
            sources: documents
                .is_dir()
                .then(|| documents.to_string_lossy().into_owned())
                .into_iter()
                .collect(),
            collect_every: "30m".into(),
            watch_files: true,
            models: None,
            parser: Some(DEFAULT_PARSER.into()),
            parser_precision: ParserPrecision::Standard,
        }
    }
}

impl Config {
    /// Lit `path`, ou la configuration par défaut si le fichier n'existe pas.
    pub fn load(path: &Path) -> Result<Self, Error> {
        match std::fs::read_to_string(path) {
            Ok(s) => {
                toml::from_str(&s).map_err(|e| Error::Invalid(format!("{}: {e}", path.display())))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), Error> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| Error::Invalid(e.to_string()))?;
        // Écriture atomique : un fichier à moitié écrit ne doit jamais être relu.
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    pub fn models_dir(&self, data: &Path) -> PathBuf {
        self.models.clone().unwrap_or_else(|| data.join("models"))
    }

    pub fn interval(&self) -> Result<Duration, Error> {
        parse_interval(&self.collect_every)
    }
}

/// `90s`, `30m`, `2h` → durée (minimum une minute).
pub fn parse_interval(s: &str) -> Result<Duration, Error> {
    let bad = || Error::Invalid(format!("intervalle invalide : {s} (ex. 30m, 2h)"));
    let (n, unit) = s.split_at(s.len().saturating_sub(1));
    let n: u64 = n.parse().map_err(|_| bad())?;
    let secs = match unit {
        "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        _ => return Err(bad()),
    };
    Ok(Duration::from_secs(secs.max(60)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intervals() {
        assert_eq!(parse_interval("2h").unwrap().as_secs(), 7200);
        assert_eq!(parse_interval("5s").unwrap().as_secs(), 60);
        assert!(parse_interval("abc").is_err());
    }

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/config.toml");
        assert_eq!(Config::load(&path).unwrap(), Config::default());
        let c = Config {
            sources: vec!["rss:https://example.org/feed".into()],
            parser: Some("vulkan:/m".into()),
            parser_precision: ParserPrecision::Light,
            ..Config::default()
        };
        c.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), c);
    }

    #[test]
    fn precision_prefixes_plain_dirs_only() {
        let light = ParserPrecision::Light;
        assert_eq!(light.apply("/m/parser-multi"), "light:/m/parser-multi");
        assert_eq!(light.apply("parser-multi"), "light:parser-multi");
        assert_eq!(light.apply("vulkan:/m/p"), "vulkan:/m/p");
        assert_eq!(light.apply("f32:/m/p"), "f32:/m/p");
        assert_eq!(ParserPrecision::Standard.apply("/m/p"), "/m/p");
        let c: Config = toml::from_str("parser_precision = \"light\"").unwrap();
        assert_eq!(c.parser_precision, light);
    }
}
