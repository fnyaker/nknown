//! Contrat de l'analyse linguistique.

use std::ops::Range;

use crate::lang::Lang;

/// Catégories grammaticales Universal Dependencies (UPOS).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pos {
    Adj,
    Adp,
    Adv,
    Aux,
    Cconj,
    Det,
    Intj,
    Noun,
    Num,
    Part,
    Pron,
    Propn,
    Punct,
    Sconj,
    Sym,
    Verb,
    /// Inconnu.
    X,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Gender {
    Masc,
    Fem,
    Neut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Number {
    Sing,
    Plur,
}

/// Traits morphologiques utiles à l'accord (rattachement adjectif → nom).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Feats {
    pub gender: Option<Gender>,
    pub number: Option<Number>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// Position en octets dans le texte analysé.
    pub span: Range<usize>,
    pub lemma: String,
    pub pos: Pos,
    pub feats: Feats,
}

/// Relation entre deux tokens (indices dans [`Analysis::tokens`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Relation {
    Qualifies { adj: usize, noun: usize },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Analysis {
    pub tokens: Vec<Token>,
    pub relations: Vec<Relation>,
}

pub trait Analyzer {
    fn supports(&self, lang: Lang) -> bool;
    fn analyze(&self, lang: Lang, text: &str) -> Analysis;
}
