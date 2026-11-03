//! Source : fichiers d'un dossier local, parcouru récursivement — textes, documentation,
//! code et configuration. Le format (texte, Markdown, code et son langage) est déduit de
//! l'extension et transmis dans `meta["format"]`. Les dossiers de dépendances et de
//! compilation, les fichiers binaires et trop gros sont ignorés.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use nknown_core::item::RawItem;
use nknown_core::source::{Capabilities, ItemStream, Source, SourceError, SourceId, SourceKind};
use nknown_core::time::{DateConfidence, Dated, Timestamp};

/// Extension → format (`plain`, `markdown`, `html`, `code:<langage>`).
fn format_of(ext: &str) -> Option<String> {
    let lang = match ext {
        "txt" | "text" | "org" | "log" => return Some("plain".into()),
        "md" | "markdown" | "rst" | "adoc" => return Some("markdown".into()),
        "html" | "htm" => return Some("html".into()),
        "rs" => "rust",
        "py" | "pyi" => "python",
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "ts" | "tsx" => "typescript",
        "go" => "go",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" | "hh" => "cpp",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "swift" => "swift",
        "cs" => "csharp",
        "rb" => "ruby",
        "php" => "php",
        "lua" => "lua",
        "sh" | "bash" | "zsh" | "fish" => "shell",
        "sql" => "sql",
        "hs" => "haskell",
        "scala" => "scala",
        "ex" | "exs" => "elixir",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "json" => "json",
        "ini" | "cfg" | "conf" => "ini",
        "nix" => "nix",
        "css" | "scss" => "css",
        "qml" => "qml",
        "vim" => "vim",
        _ => return None,
    };
    Some(format!("code:{lang}"))
}

/// Dossiers jamais parcourus : dépendances, compilation, caches, gestion de versions.
pub const SKIPPED_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "target",
    "node_modules",
    ".venv",
    "venv",
    "__pycache__",
    "dist",
    "build",
    ".cache",
    ".idea",
    ".vscode",
    ".mypy_cache",
    ".pytest_cache",
    ".tox",
];

/// Au-delà, un fichier est probablement généré (données, code minifié).
const MAX_FILE_BYTES: u64 = 1 << 20;

#[derive(Debug)]
pub struct LocalFiles {
    id: SourceId,
    root: PathBuf,
}

impl LocalFiles {
    pub fn new(id: SourceId, root: impl Into<PathBuf>) -> Self {
        Self {
            id,
            root: root.into(),
        }
    }
}

/// Les liens symboliques sont ignorés (évite les boucles).
fn collect(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let ft = entry.file_type()?;
        let name = entry.file_name();
        if ft.is_dir() {
            if !SKIPPED_DIRS.contains(&name.to_string_lossy().as_ref()) {
                collect(&path, out)?;
            }
        } else if ft.is_file()
            && path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| format_of(&e.to_ascii_lowercase()).is_some())
            && entry.metadata().is_ok_and(|m| m.len() <= MAX_FILE_BYTES)
        {
            out.push(path);
        }
    }
    Ok(())
}

fn read_item(
    path: &Path,
    id: &SourceId,
    since: Option<Timestamp>,
    fetched_at: Timestamp,
) -> Result<Option<RawItem>, SourceError> {
    let mtime = fs::metadata(path)?
        .modified()
        .ok()
        .and_then(Timestamp::from_system);
    if let (Some(since), Some(m)) = (since, mtime)
        && m <= since
    {
        return Ok(None);
    }
    let Ok(mut text) = String::from_utf8(fs::read(path)?) else {
        return Ok(None); // pas du texte UTF-8 : fichier binaire
    };
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mut format = format_of(&ext).unwrap_or_else(|| "plain".into());
    if format == "html" {
        text = crate::html::to_text(&text);
        format = "markdown".into(); // les blocs <pre> deviennent des blocs délimités
    }
    let mut item = RawItem::new(id.clone(), text, fetched_at);
    item.meta.push(("format".into(), format));
    item.external_id = Some(path.display().to_string());
    item.title = path.file_stem().map(|s| s.to_string_lossy().into_owned());
    // La date de modification n'est qu'un indice de la date réelle du contenu.
    item.published_at = mtime.map(|at| Dated {
        at,
        confidence: DateConfidence::Inferred,
    });
    Ok(Some(item))
}

impl Source for LocalFiles {
    fn id(&self) -> &SourceId {
        &self.id
    }

    fn kind(&self) -> SourceKind {
        SourceKind::LocalFile
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            published_at: true,
            lang: false,
            incremental: true,
        }
    }

    fn fetch(&mut self, since: Option<Timestamp>) -> Result<ItemStream<'_>, SourceError> {
        let mut paths = Vec::new();
        collect(&self.root, &mut paths)?;
        paths.sort();
        let fetched_at = Timestamp::now();
        let id = self.id.clone();
        Ok(Box::new(paths.into_iter().filter_map(move |path| {
            read_item(&path, &id, since, fetched_at).transpose()
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_text_files_recursively() {
        let dir = std::env::temp_dir().join(format!("nknown-files-{}", std::process::id()));
        fs::create_dir_all(dir.join("sous")).unwrap();
        fs::write(dir.join("a.txt"), "Bonjour Paris").unwrap();
        fs::write(dir.join("sous/b.md"), "# Note").unwrap();
        fs::write(dir.join("ignore.bin"), [0u8, 1, 2]).unwrap();
        fs::create_dir_all(dir.join("target")).unwrap();
        fs::write(dir.join("target/gen.rs"), "fn x() {}").unwrap();

        let mut src = LocalFiles::new(SourceId("test".into()), &dir);
        let items: Vec<_> = src.fetch(None).unwrap().map(Result::unwrap).collect();
        fs::remove_dir_all(&dir).unwrap();

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].text, "Bonjour Paris");
        assert_eq!(items[0].title.as_deref(), Some("a"));
        assert_eq!(items[0].date().confidence, DateConfidence::Inferred);
    }
}
