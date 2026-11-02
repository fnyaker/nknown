//! Le contrat d'une source de données.

use std::{error, fmt, io};

use crate::item::RawItem;
use crate::time::Timestamp;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceId(pub String);

/// Famille de source : sert d'a priori pour la nature et la fiabilité des contenus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceKind {
    Feed,
    Encyclopedia,
    Web,
    LocalFile,
    Other,
}

/// Ce qu'une source sait fournir. Le noyau s'adapte au lieu d'exiger.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub published_at: bool,
    pub lang: bool,
    /// Sait ne renvoyer que les nouveautés depuis une date.
    pub incremental: bool,
}

#[derive(Debug)]
pub enum SourceError {
    Io(io::Error),
    Parse(String),
    Unavailable(String),
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SourceError::Io(e) => write!(f, "erreur d'E/S : {e}"),
            SourceError::Parse(m) => write!(f, "contenu invalide : {m}"),
            SourceError::Unavailable(m) => write!(f, "source indisponible : {m}"),
        }
    }
}

impl error::Error for SourceError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            SourceError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for SourceError {
    fn from(e: io::Error) -> Self {
        SourceError::Io(e)
    }
}

pub type ItemStream<'a> = Box<dyn Iterator<Item = Result<RawItem, SourceError>> + 'a>;

/// Une source de contenus. Synchrone et compatible `dyn` pour l'instant ;
/// le passage à l'asynchrone se décidera avec la source RSS.
pub trait Source {
    fn id(&self) -> &SourceId;
    fn kind(&self) -> SourceKind;
    fn capabilities(&self) -> Capabilities;
    /// Renvoie les contenus, uniquement ceux postérieurs à `since` si la source est incrémentale.
    fn fetch(&mut self, since: Option<Timestamp>) -> Result<ItemStream<'_>, SourceError>;
}
