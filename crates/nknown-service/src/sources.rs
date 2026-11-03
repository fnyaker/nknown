//! Sources décrites par une chaîne (`spec`) : dossier, flux, Wikipédia.

use nknown_api::SourceKind;
use nknown_core::source::{Source, SourceId};
use nknown_sources::{LocalFiles, RssSource, Selection, WebSource, WikipediaSource};

use crate::Error;

/// Ouvre la source décrite par `spec` :
/// `<dossier>`, `rss:<url>`, `wiki:<édition>:<Titre>|<Titre>`, `wiki-search:<édition>:<requête>`,
/// `wiki-cat:<édition>:<Catégorie>`, `web:<url> [pages=<n>] [depth=<n>]`.
pub fn open(spec: &str) -> Result<Box<dyn Source>, Error> {
    let wiki = |rest: &str| -> Result<(String, String), Error> {
        let (edition, arg) = rest
            .split_once(':')
            .ok_or_else(|| Error::Invalid(format!("édition manquante : {spec}")))?;
        Ok((edition.to_owned(), arg.to_owned()))
    };
    Ok(if let Some(url) = spec.strip_prefix("rss:") {
        Box::new(RssSource::new(url))
    } else if let Some(rest) = spec.strip_prefix("wiki:") {
        let (edition, titles) = wiki(rest)?;
        let titles = titles.split('|').map(|t| t.trim().to_owned()).collect();
        Box::new(WikipediaSource::new(edition, Selection::Titles(titles)))
    } else if let Some(rest) = spec.strip_prefix("wiki-search:") {
        let (edition, query) = wiki(rest)?;
        Box::new(WikipediaSource::new(
            edition,
            Selection::Search { query, limit: 10 },
        ))
    } else if let Some(rest) = spec.strip_prefix("wiki-cat:") {
        let (edition, name) = wiki(rest)?;
        Box::new(WikipediaSource::new(
            edition,
            Selection::Category { name, limit: 50 },
        ))
    } else if let Some(rest) = spec.strip_prefix("web:") {
        Box::new(WebSource::parse(rest).map_err(|e| Error::Invalid(format!("{spec} : {e}")))?)
    } else if std::path::Path::new(spec).is_dir() {
        Box::new(LocalFiles::new(SourceId(format!("files:{spec}")), spec))
    } else {
        return Err(Error::Invalid(format!("source inconnue : {spec}")));
    })
}

/// Nature d'une source d'après sa description.
pub fn kind_of(spec: &str) -> SourceKind {
    if spec.starts_with("rss:") {
        SourceKind::Feed
    } else if spec.starts_with("wiki") {
        SourceKind::Encyclopedia
    } else if spec.starts_with("web:") {
        SourceKind::Web
    } else {
        SourceKind::File
    }
}

/// Identifiant opaque d'une source (empreinte FNV-1a de `spec`), sûr dans une URL.
pub fn id_of(spec: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in spec.bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Nom court d'une source pour la carte : dernier dossier, hôte d'un flux, édition de Wikipédia.
pub fn label(source: &str) -> String {
    if let Some(path) = source.strip_prefix("files:") {
        let path = path.trim_end_matches('/');
        return path.rsplit('/').next().unwrap_or(path).to_owned();
    }
    if let Some(edition) = source.strip_prefix("wikipedia:") {
        return format!("{edition}.wikipedia");
    }
    if let Some(rest) = source.split_once("://").map(|(_, r)| r) {
        return rest
            .split('/')
            .next()
            .unwrap_or(rest)
            .trim_start_matches("www.")
            .to_owned();
    }
    source.to_owned()
}

/// Fichiers dont le nom seul ne dit rien : on les nomme par leur dossier.
const GENERIC_STEMS: &[&str] = &[
    "lib", "mod", "main", "index", "readme", "__init__", "init", "default", "build",
];
/// Dossiers de rangement sautés pour trouver ce dossier parlant.
const PLAIN_DIRS: &[&str] = &["src", "source", "sources", "lib", "app"];

/// Titre affiché d'un document. Un fichier garde son extension (`lib.rs`, pas `lib`) et, si son
/// nom est générique, le dossier qui le distingue (`nknown-server › lib.rs`). Le titre indexé
/// reste le radical, qui sert à la correspondance avec la requête.
pub fn display_title(
    title: Option<&str>,
    external_id: Option<&str>,
    source: &str,
) -> Option<String> {
    let path = external_id
        .filter(|_| source.starts_with("files:"))
        .map(std::path::Path::new);
    let Some(name) = path.and_then(|p| p.file_name()).and_then(|n| n.to_str()) else {
        return title.map(str::to_owned);
    };
    let stem = name.split('.').next().unwrap_or(name).to_ascii_lowercase();
    if !GENERIC_STEMS.contains(&stem.as_str()) {
        return Some(name.to_owned());
    }
    let parent = path
        .and_then(|p| p.parent())
        .into_iter()
        .flat_map(|p| p.iter().rev())
        .filter_map(|c| c.to_str())
        .find(|c| !PLAIN_DIRS.contains(&c.to_ascii_lowercase().as_str()));
    Some(match parent {
        Some(dir) => format!("{dir} › {name}"),
        None => name.to_owned(),
    })
}

/// Adresse à ouvrir : celle du document, ou `file://` pour un fichier local.
pub fn open_url(url: Option<&str>, external_id: Option<&str>, source: &str) -> Option<String> {
    if let Some(url) = url {
        return Some(url.to_owned());
    }
    let path = external_id.filter(|p| source.starts_with("files:") && p.starts_with('/'))?;
    let mut out = String::from("file://");
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    Some(out)
}

pub fn api_kind(code: u8) -> SourceKind {
    match code {
        0 => SourceKind::Feed,
        1 => SourceKind::Encyclopedia,
        2 => SourceKind::Web,
        3 => SourceKind::File,
        _ => SourceKind::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert_eq!(label("files:/home/a/notes/"), "notes");
        assert_eq!(
            label("rss:https://www.lemonde.fr/rss/une.xml"),
            "lemonde.fr"
        );
        assert_eq!(id_of("a"), id_of("a"));
        assert_ne!(id_of("a"), id_of("b"));
    }

    #[test]
    fn titles_and_urls() {
        let src = "files:/p";
        let t = |path: &str| display_title(Some("x"), Some(path), src);
        assert_eq!(
            t("/p/crates/nknown-server/src/lib.rs").as_deref(),
            Some("nknown-server › lib.rs")
        );
        assert_eq!(
            t("/p/docs/architecture.md").as_deref(),
            Some("architecture.md")
        );
        assert_eq!(t("/p/README.md").as_deref(), Some("p › README.md"));
        // Hors fichiers : le titre du document.
        assert_eq!(
            display_title(Some("Paris"), Some("guid"), "rss:x").as_deref(),
            Some("Paris")
        );
        assert_eq!(
            open_url(None, Some("/p/mes notes/é.md"), src).as_deref(),
            Some("file:///p/mes%20notes/%C3%A9.md")
        );
        assert_eq!(
            open_url(Some("https://a"), None, src).as_deref(),
            Some("https://a")
        );
        assert_eq!(open_url(None, Some("guid"), "rss:x"), None);
    }
}
