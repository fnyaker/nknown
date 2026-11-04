//! Organisation des données dans le stockage clé-valeur.
//!
//! | Préfixe | Clé | Valeur |
//! |---|---|---|
//! | `M` | nom | compteur u64 (identifiants, statistiques globales) |
//! | `H` | empreinte du contenu | id du document (dédoublonnage) |
//! | `D` | id | [`DocRecord`] |
//! | `P` | id | [`PassageRecord`] |
//! | `L` | langue, catégorie, lemme normalisé | id du lemme |
//! | `N` | id | [`LemmaRecord`] |
//! | `S` | id du lemme | nombre de passages qui le contiennent (df) |
//! | `E` | de, type, vers | poids f32 (voir [`crate::graph::Edge`]) |
//! | `Q` | nom, adjectif, passage | — : l'adjectif qualifie le nom dans ce passage |
//! | `q` | passage, nom, adjectif | — : index inverse de `Q` (suppression d'un passage) |
//! | `X` | source, 0, identifiant externe | doc u64, empreinte u64, date i64 : contenu déjà vu |
//! | `C` | source | date u64 de la dernière collecte (point de reprise) |
//! | `A` | id du document | autorité f32 (PageRank, moyenne 1) |
//! | `T` | langue, 0, racine, 0, id du lemme | catégorie (octet) : lemmes de même racine |
//!
//! Identifiants : un seul compteur pour tous les nœuds (documents, passages, lemmes).

use nknown_core::analyze::Pos;
use nknown_core::lang::Lang;
use nknown_core::nature::Nature;
use serde::{Deserialize, Serialize};

use crate::graph::NodeId;

pub const META: u8 = b'M';
pub const HASH: u8 = b'H';
pub const DOC: u8 = b'D';
pub const PASSAGE: u8 = b'P';
pub const LEXEME: u8 = b'L';
pub const NODE: u8 = b'N';
pub const STATS: u8 = b'S';
pub const QUALIFIED: u8 = b'Q';
pub const QUALIFIED_REV: u8 = b'q';
pub const SEEN: u8 = b'X';
pub const CHECKPOINT: u8 = b'C';
pub const AUTHORITY: u8 = b'A';
pub const STEM: u8 = b'T';

/// Version du format de la base (compteur `schema`) : 2 ajoute l'index des racines.
pub const SCHEMA: &str = "schema";
pub const SCHEMA_VERSION: u64 = 2;

pub const NEXT_ID: &str = "next_id";
pub const PASSAGES: &str = "passages";
pub const TOTAL_LEN: &str = "total_len";
pub const DOCS: &str = "docs";

pub fn id_key(prefix: u8, id: NodeId) -> Vec<u8> {
    let mut k = vec![prefix];
    k.extend_from_slice(&id.0.to_be_bytes());
    k
}

pub fn meta_key(name: &str) -> Vec<u8> {
    let mut k = vec![META];
    k.extend_from_slice(name.as_bytes());
    k
}

pub fn hash_key(hash: u64) -> Vec<u8> {
    let mut k = vec![HASH];
    k.extend_from_slice(&hash.to_be_bytes());
    k
}

/// Les lemmes sont indexés en minuscules : « Paris » et « paris » (nom propre) se rejoignent,
/// mais pas « Paris » (nom propre) et « pari » (nom).
pub fn normalize_lemma(lemma: &str) -> String {
    lemma
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            '\u{2019}' => Some('\''),
            '\u{0300}' | '\u{0301}' => None,
            'ё' => Some('е'),
            c => Some(c),
        })
        .collect()
}

/// Octet de catégorie des termes de code dans les clés de lemmes (hors des valeurs de `Pos`) :
/// `map` dans du code et « map » dans un texte anglais sont deux termes distincts.
pub const CODE_TERM: u8 = 200;

/// Nature d'un terme de recherche : un mot (avec sa catégorie) ou un terme de code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TermKind {
    Word(Pos),
    Code,
}

impl TermKind {
    pub fn byte(self) -> u8 {
        match self {
            TermKind::Word(pos) => pos as u8,
            TermKind::Code => CODE_TERM,
        }
    }
}

pub fn lexeme_key(lang: Lang, kind: TermKind, lemma: &str) -> Vec<u8> {
    let mut k = vec![LEXEME];
    k.extend_from_slice(lang.as_str().as_bytes());
    k.push(kind.byte());
    k.extend_from_slice(normalize_lemma(lemma).as_bytes());
    k
}

/// Préfixe des lemmes de racine `stem` dans la langue `lang`.
pub fn stem_prefix(lang: Lang, stem: &str) -> Vec<u8> {
    let mut k = vec![STEM];
    k.extend_from_slice(lang.as_str().as_bytes());
    k.push(0);
    k.extend_from_slice(stem.as_bytes());
    k.push(0);
    k
}

pub fn stem_key(lang: Lang, stem: &str, lemma: NodeId) -> Vec<u8> {
    let mut k = stem_prefix(lang, stem);
    k.extend_from_slice(&lemma.0.to_be_bytes());
    k
}

pub fn qualified_prefix(noun: NodeId, adj: NodeId) -> Vec<u8> {
    let mut k = vec![QUALIFIED];
    k.extend_from_slice(&noun.0.to_be_bytes());
    k.extend_from_slice(&adj.0.to_be_bytes());
    k
}

pub fn qualified_key(noun: NodeId, adj: NodeId, passage: NodeId) -> Vec<u8> {
    let mut k = qualified_prefix(noun, adj);
    k.extend_from_slice(&passage.0.to_be_bytes());
    k
}

pub fn qualified_rev_key(passage: NodeId, noun: NodeId, adj: NodeId) -> Vec<u8> {
    let mut k = vec![QUALIFIED_REV];
    k.extend_from_slice(&passage.0.to_be_bytes());
    k.extend_from_slice(&noun.0.to_be_bytes());
    k.extend_from_slice(&adj.0.to_be_bytes());
    k
}

pub fn seen_key(source: &str, external_id: &str) -> Vec<u8> {
    let mut k = vec![SEEN];
    k.extend_from_slice(source.as_bytes());
    k.push(0);
    k.extend_from_slice(external_id.as_bytes());
    k
}

pub fn checkpoint_key(source: &str) -> Vec<u8> {
    let mut k = vec![CHECKPOINT];
    k.extend_from_slice(source.as_bytes());
    k
}

pub fn u64_value(v: Option<Vec<u8>>) -> u64 {
    v.and_then(|b| b.try_into().ok())
        .map_or(0, u64::from_be_bytes)
}

pub fn f32_value(v: &[u8]) -> f32 {
    v.try_into().map_or(0.0, f32::from_le_bytes)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocRecord {
    pub source: String,
    /// `SourceKind` encodé (voir [`crate::source_kind_code`]).
    pub kind: u8,
    pub url: Option<String>,
    pub title: Option<String>,
    /// Date retenue (secondes Unix) et sa fiabilité (0 déclarée, 1 déduite, 2 collecte seule).
    pub date: i64,
    pub date_confidence: u8,
    pub fetched_at: i64,
    pub lang: String,
    pub nature: [f32; Nature::COUNT],
    pub first_passage: u64,
    pub passages: u32,
    /// Identifiant dans la source (lien, GUID, titre d'article), pour les mises à jour.
    pub external_id: Option<String>,
    /// Empreinte du contenu (source + texte).
    pub hash: u64,
    /// Lemmes du titre.
    pub title_lemmas: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PassageRecord {
    pub doc: u64,
    pub ordinal: u32,
    pub text: String,
    /// Nombre de mots indexés (longueur pour BM25).
    pub len: u32,
    /// Passage de code (sinon prose), et son langage s'il est connu.
    pub is_code: bool,
    pub code_lang: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LemmaRecord {
    pub lang: String,
    pub pos: u8,
    pub lemma: String,
}
