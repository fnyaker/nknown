//! Contrat de l'analyse linguistique.

use std::ops::Range;

use crate::lang::Lang;

/// Catégories grammaticales Universal Dependencies (UPOS).
///
/// `#[repr(u8)]` : la valeur numérique sert d'encodage dans les fichiers compilés,
/// ne pas réordonner les variantes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
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

impl Pos {
    /// Toutes les variantes, dans l'ordre de leur valeur numérique.
    pub const ALL: [Pos; 17] = [
        Pos::Adj,
        Pos::Adp,
        Pos::Adv,
        Pos::Aux,
        Pos::Cconj,
        Pos::Det,
        Pos::Intj,
        Pos::Noun,
        Pos::Num,
        Pos::Part,
        Pos::Pron,
        Pos::Propn,
        Pos::Punct,
        Pos::Sconj,
        Pos::Sym,
        Pos::Verb,
        Pos::X,
    ];

    pub fn from_u8(b: u8) -> Option<Pos> {
        Pos::ALL.get(usize::from(b)).copied()
    }
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

/// Domaine d'un token : langue humaine, ou terme de code (identifiant, chemin).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Domain {
    #[default]
    Prose,
    Code,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// Position en octets dans le texte analysé.
    pub span: Range<usize>,
    /// Forme normalisée : lemme pour la prose, identifiant en minuscules pour le code.
    pub lemma: String,
    /// Catégorie grammaticale (prose) ; `X` pour un terme de code.
    pub pos: Pos,
    pub feats: Feats,
    pub domain: Domain,
}

/// Nature d'un bloc de texte.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SegmentKind {
    Prose,
    /// Code, avec son langage s'il est connu (`rust`, `python`...).
    Code {
        lang: Option<String>,
    },
}

/// Bloc homogène du document : les tokens `tokens` (indices dans [`Analysis::tokens`]).
/// Les blocs se suivent et ne se chevauchent pas ; un commentaire au milieu du code
/// est un bloc de prose entre deux blocs de code.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Segment {
    pub tokens: Range<usize>,
    pub kind: SegmentKind,
}

/// Relation entre deux tokens (indices dans [`Analysis::tokens`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Relation {
    /// Un adjectif qualifie un nom (« rues **calmes** »).
    Qualifies { adj: usize, noun: usize },
    /// Dépendance syntaxique Universal Dependencies : `dep` dépend de `head`
    /// (`None` pour la racine de la phrase), avec l'étiquette `label` (`nsubj`, `obj`, `amod`...).
    Dependency {
        dep: usize,
        head: Option<usize>,
        label: String,
    },
}

impl Relation {
    /// La même relation, indices décalés de `offset` (phrase → document).
    pub fn shifted(self, offset: usize) -> Relation {
        match self {
            Relation::Qualifies { adj, noun } => Relation::Qualifies {
                adj: adj + offset,
                noun: noun + offset,
            },
            Relation::Dependency { dep, head, label } => Relation::Dependency {
                dep: dep + offset,
                head: head.map(|h| h + offset),
                label,
            },
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Analysis {
    pub tokens: Vec<Token>,
    pub relations: Vec<Relation>,
    /// Blocs prose / code ; vide = tout le texte est de la prose.
    pub segments: Vec<Segment>,
}

pub trait Analyzer {
    fn supports(&self, lang: Lang) -> bool;
    fn analyze(&self, lang: Lang, text: &str) -> Analysis;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pos_encoding_is_stable() {
        for (i, p) in Pos::ALL.iter().enumerate() {
            assert_eq!(*p as usize, i);
            assert_eq!(Pos::from_u8(*p as u8), Some(*p));
        }
        assert_eq!(Pos::from_u8(17), None);
    }
}
