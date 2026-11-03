//! Catalogue de sources proposées à l'import (`catalog/sources.toml` du dépôt).
//!
//! Lu depuis la branche main du dépôt au plus une fois par jour (cache dans le dossier des
//! données), sinon le cache, sinon la copie intégrée au binaire : le catalogue est toujours là,
//! même hors ligne. `NKNOWN_CATALOG_URL` remplace l'adresse (miroir, essais).

use std::path::Path;
use std::time::{Duration, SystemTime};

use nknown_api as api;
use serde::Deserialize;

const EMBEDDED: &str = include_str!("../../../catalog/sources.toml");
const URL: &str = "https://raw.githubusercontent.com/fnyaker/nknown/main/catalog/sources.toml";
const CACHE: &str = "catalog.toml";
const REFRESH: Duration = Duration::from_secs(24 * 3600);

#[derive(Debug, Deserialize)]
struct File {
    #[serde(default)]
    themes: std::collections::BTreeMap<String, String>,
    #[serde(rename = "source", default)]
    sources: Vec<Entry>,
}

#[derive(Debug, Deserialize)]
struct Entry {
    name: String,
    spec: String,
    lang: String,
    theme: String,
    #[serde(default)]
    description: Option<String>,
}

fn parse(text: &str) -> Option<File> {
    toml::from_str::<File>(text)
        .ok()
        .filter(|f| !f.sources.is_empty())
}

/// Texte du catalogue : celui du dépôt s'il est rafraîchi, le cache, ou la copie intégrée.
fn text(data_dir: &Path) -> String {
    let cache = data_dir.join(CACHE);
    let fresh = std::fs::metadata(&cache)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age < REFRESH);
    if !fresh {
        let url = std::env::var("NKNOWN_CATALOG_URL").unwrap_or_else(|_| URL.to_owned());
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(10)))
            .user_agent(concat!("NKnown/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        if let Ok(mut resp) = agent.get(&url).call()
            && let Ok(body) = resp.body_mut().read_to_string()
            && parse(&body).is_some()
        {
            let tmp = cache.with_extension("toml.tmp");
            if std::fs::write(&tmp, &body).is_ok() {
                let _ = std::fs::rename(&tmp, &cache);
            }
            return body;
        }
    }
    std::fs::read_to_string(&cache)
        .ok()
        .filter(|t| parse(t).is_some())
        .unwrap_or_else(|| EMBEDDED.to_owned())
}

/// Le catalogue ; `added` marque les sources déjà configurées.
pub fn load(data_dir: &Path, configured: &[String]) -> api::Catalog {
    let file = parse(&text(data_dir))
        .or_else(|| parse(EMBEDDED))
        .unwrap_or(File {
            themes: Default::default(),
            sources: Vec::new(),
        });
    api::Catalog {
        themes: file.themes.into_iter().collect(),
        entries: file
            .sources
            .into_iter()
            .map(|e| api::CatalogEntry {
                added: configured.contains(&e.spec),
                kind: crate::sources::kind_of(&e.spec),
                name: e.name,
                spec: e.spec,
                lang: e.lang,
                theme: e.theme,
                description: e.description,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_catalog_is_valid() {
        let f = parse(EMBEDDED).expect("catalogue intégré valide");
        assert!(f.sources.len() >= 20);
        for e in &f.sources {
            assert!(
                f.themes.contains_key(&e.theme),
                "thème inconnu : {}",
                e.theme
            );
            assert!(crate::sources::open(&e.spec).is_ok(), "{}", e.spec);
            assert!(
                ["fr", "en", "ru", "zh"].contains(&e.lang.as_str()),
                "{}",
                e.lang
            );
        }
    }

    #[test]
    fn marks_configured_sources() {
        let dir = tempfile::tempdir().unwrap();
        // Cache récent : pas de réseau pendant le test.
        std::fs::write(dir.path().join(CACHE), EMBEDDED).unwrap();
        let first = parse(EMBEDDED).unwrap().sources[0].spec.clone();
        let c = load(dir.path(), std::slice::from_ref(&first));
        assert!(c.entries.iter().any(|e| e.spec == first && e.added));
        assert!(c.entries.iter().filter(|e| e.added).count() == 1);
    }
}
