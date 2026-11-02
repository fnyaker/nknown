//! Couche 1 : le lexique.
//!
//! Implémentation provisoire en mémoire. La version cible charge des automates `fst`
//! compilés hors ligne depuis les dumps Wiktextract.

use std::collections::HashMap;

use nknown_core::analyze::{Feats, Pos};

/// Une analyse possible d'une forme. Une forme peut en avoir plusieurs (« ferme »).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexEntry {
    pub lemma: String,
    pub pos: Pos,
    pub feats: Feats,
}

#[derive(Debug, Default)]
pub struct Lexicon {
    forms: HashMap<String, Vec<LexEntry>>,
}

impl Lexicon {
    pub fn insert(&mut self, form: &str, entry: LexEntry) {
        self.forms
            .entry(form.to_lowercase())
            .or_default()
            .push(entry);
    }

    pub fn lookup(&self, form: &str) -> &[LexEntry] {
        self.forms
            .get(&form.to_lowercase())
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}
