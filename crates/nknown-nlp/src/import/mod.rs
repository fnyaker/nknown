//! Importeurs de ressources lexicales vers un [`LexiconBuilder`](crate::LexiconBuilder).
//!
//! Chaque ressource a son importeur ; tous alimentent le même builder, qui fusionne.

pub mod lexique;
pub mod relations;
pub mod wiktextract;

use std::{error, fmt, io};

use crate::tokenize;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ImportStats {
    /// Lignes lues.
    pub entries: u64,
    /// Lignes retenues (bonne langue, catégorie reconnue).
    pub used: u64,
    /// Lignes illisibles, ignorées.
    pub invalid: u64,
    /// Couples (forme, analyse) proposés au lexique, avant dédoublonnage.
    pub analyses: u64,
}

#[derive(Debug)]
pub enum ImportError {
    Io(io::Error),
    /// Fichier d'un format inattendu (ex. colonne obligatoire absente).
    Format(String),
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImportError::Io(e) => write!(f, "lecture de la ressource : {e}"),
            ImportError::Format(m) => write!(f, "format inattendu : {m}"),
        }
    }
}

impl error::Error for ImportError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            ImportError::Io(e) => Some(e),
            ImportError::Format(_) => None,
        }
    }
}

impl From<io::Error> for ImportError {
    fn from(e: io::Error) -> Self {
        ImportError::Io(e)
    }
}

/// Vrai si `form` est un seul mot pour le tokeniseur : les autres ne seraient jamais retrouvés.
/// Un mot chinois ou japonais est gardé entier même si le segmenteur le découperait
/// (« 北京大学 ») : sans espaces ni ponctuation, c'est bien une entrée de dictionnaire.
pub(crate) fn is_single_word(form: &str) -> bool {
    if form.chars().any(tokenize::is_cjk) {
        return !form.is_empty() && form.chars().all(char::is_alphanumeric);
    }
    matches!(tokenize::words(form).as_slice(), [span] if *span == (0..form.len()))
}

/// Retire les accents toniques des lemmes (dictionnaires russes).
pub(crate) fn strip_stress(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, '\u{0300}' | '\u{0301}'))
        .collect()
}
