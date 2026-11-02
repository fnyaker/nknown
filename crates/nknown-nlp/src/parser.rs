//! Contrat d'un analyseur syntaxique complet (couche 3), implémenté hors de cette crate
//! (ex. `nknown-neural`) pour ne pas imposer ses dépendances au reste du pipeline.

use nknown_core::analyze::{Feats, Pos};
use nknown_core::lang::Lang;

/// Analyse d'une phrase : une entrée par token.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedSentence {
    pub upos: Vec<Pos>,
    pub feats: Vec<Feats>,
    /// Index du gouverneur dans la phrase, `None` pour la racine.
    pub heads: Vec<Option<usize>>,
    pub deprels: Vec<String>,
}

pub trait SentenceParser: Send + Sync {
    fn supports(&self, lang: Lang) -> bool;
    /// Analyse un lot de phrases déjà découpées. `None` pour une phrase non analysable
    /// (trop longue, langue non gérée) : le pipeline se rabat alors sur la couche 2.
    fn parse(&self, lang: Lang, sentences: &[Vec<&str>]) -> Vec<Option<ParsedSentence>>;
}
