//! Source : fichiers texte d'un dossier local (`.txt`, `.md`), parcouru récursivement.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use nknown_core::item::RawItem;
use nknown_core::source::{Capabilities, ItemStream, Source, SourceError, SourceId, SourceKind};
use nknown_core::time::{DateConfidence, Dated, Timestamp};

const EXTENSIONS: &[&str] = &["txt", "md"];

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
        if ft.is_dir() {
            collect(&path, out)?;
        } else if ft.is_file()
            && path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| EXTENSIONS.contains(&e))
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
    let mut item = RawItem::new(id.clone(), fs::read_to_string(path)?, fetched_at);
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

        let mut src = LocalFiles::new(SourceId("test".into()), &dir);
        let items: Vec<_> = src.fetch(None).unwrap().map(Result::unwrap).collect();
        fs::remove_dir_all(&dir).unwrap();

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].text, "Bonjour Paris");
        assert_eq!(items[0].title.as_deref(), Some("a"));
        assert_eq!(items[0].date().confidence, DateConfidence::Inferred);
    }
}
