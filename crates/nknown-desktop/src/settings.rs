//! Réglages de l'app (`$XDG_CONFIG_HOME/nknown/app.toml`) : quel moteur utiliser. Le jeton d'un
//! serveur distant n'est jamais ici : il est rangé dans KWallet (`secrets.rs`).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Le démon s'il tourne (démarré au besoin), sinon le cœur intégré.
    #[default]
    Auto,
    Daemon,
    Embedded,
    Remote,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub engine: Mode,
    /// Adresse du serveur distant (`https://…`), pour `engine = "remote"`.
    pub remote_url: String,
}

fn path() -> PathBuf {
    nknown_service::config::config_path().with_file_name("app.toml")
}

impl Settings {
    pub fn load() -> Self {
        std::fs::read_to_string(path())
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, path)
    }
}
