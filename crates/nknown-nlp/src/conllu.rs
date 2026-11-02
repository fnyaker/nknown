//! Lecture des corpus annotés Universal Dependencies (format CoNLL-U).
//!
//! Les phrases sont ramenées aux tokens de surface, comme les produit notre tokeniseur :
//! une contraction (« du » = de + le) devient un seul token, étiqueté avec la catégorie
//! de son premier mot (ADP). Les nœuds vides (`8.1`) sont ignorés.

use std::collections::HashMap;
use std::io::BufRead;

use nknown_core::analyze::{Feats, Gender, Number, Pos};

use crate::import::ImportError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoldToken {
    pub form: String,
    pub upos: Pos,
    pub feats: Feats,
    /// Index (dans la phrase) du token gouverneur ; `None` pour la racine et les contractions.
    pub head: Option<usize>,
    /// Relation de dépendance (`amod`, `nsubj`...) ; vide pour une contraction.
    pub deprel: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GoldSentence {
    pub tokens: Vec<GoldToken>,
}

impl GoldSentence {
    pub fn words(&self) -> Vec<&str> {
        self.tokens.iter().map(|t| t.form.as_str()).collect()
    }

    pub fn tags(&self) -> Vec<Pos> {
        self.tokens.iter().map(|t| t.upos).collect()
    }

    pub fn to_training(&self) -> crate::tagger::TrainingSentence {
        crate::tagger::TrainingSentence {
            words: self.tokens.iter().map(|t| t.form.clone()).collect(),
            tags: self.tags(),
        }
    }
}

struct Word {
    id: usize,
    form: String,
    upos: Pos,
    feats: Feats,
    head: usize,
    deprel: String,
}

pub fn parse(reader: impl BufRead) -> Result<Vec<GoldSentence>, ImportError> {
    let mut sentences = Vec::new();
    let mut words: Vec<Word> = Vec::new();
    let mut mwts: Vec<(usize, usize, String)> = Vec::new();
    for (n, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            if !words.is_empty() {
                sentences.push(assemble(&words, &mwts));
            }
            words.clear();
            mwts.clear();
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let bad = || ImportError::Format(format!("ligne {} : {line}", n + 1));
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() != 10 {
            return Err(bad());
        }
        let id = cols[0];
        if id.contains('.') {
            continue;
        }
        if let Some((a, b)) = id.split_once('-') {
            let (a, b) = (a.parse().map_err(|_| bad())?, b.parse().map_err(|_| bad())?);
            mwts.push((a, b, cols[1].to_owned()));
            continue;
        }
        words.push(Word {
            id: id.parse().map_err(|_| bad())?,
            form: cols[1].to_owned(),
            upos: parse_upos(cols[3]).ok_or_else(bad)?,
            feats: parse_feats(cols[5]),
            head: cols[6].parse().unwrap_or(0),
            deprel: cols[7].to_owned(),
        });
    }
    if !words.is_empty() {
        sentences.push(assemble(&words, &mwts));
    }
    Ok(sentences)
}

fn assemble(words: &[Word], mwts: &[(usize, usize, String)]) -> GoldSentence {
    let mut tokens = Vec::new();
    let mut surface_of: HashMap<usize, usize> = HashMap::new();
    let mut heads: Vec<Option<usize>> = Vec::new();
    for w in words {
        match mwts.iter().find(|(a, b, _)| (*a..=*b).contains(&w.id)) {
            Some((a, _, form)) => {
                if w.id == *a {
                    tokens.push(GoldToken {
                        form: form.clone(),
                        upos: w.upos,
                        feats: Feats::default(),
                        head: None,
                        deprel: String::new(),
                    });
                    heads.push(None);
                }
                surface_of.insert(w.id, tokens.len() - 1);
            }
            None => {
                surface_of.insert(w.id, tokens.len());
                tokens.push(GoldToken {
                    form: w.form.clone(),
                    upos: w.upos,
                    feats: w.feats,
                    head: None,
                    deprel: w.deprel.clone(),
                });
                heads.push(Some(w.head));
            }
        }
    }
    for (t, head) in tokens.iter_mut().zip(heads) {
        t.head = head.and_then(|h| surface_of.get(&h).copied());
    }
    GoldSentence { tokens }
}

pub fn parse_upos(s: &str) -> Option<Pos> {
    Some(match s {
        "ADJ" => Pos::Adj,
        "ADP" => Pos::Adp,
        "ADV" => Pos::Adv,
        "AUX" => Pos::Aux,
        "CCONJ" => Pos::Cconj,
        "DET" => Pos::Det,
        "INTJ" => Pos::Intj,
        "NOUN" => Pos::Noun,
        "NUM" => Pos::Num,
        "PART" => Pos::Part,
        "PRON" => Pos::Pron,
        "PROPN" => Pos::Propn,
        "PUNCT" => Pos::Punct,
        "SCONJ" => Pos::Sconj,
        "SYM" => Pos::Sym,
        "VERB" => Pos::Verb,
        "X" | "_" => Pos::X,
        _ => return None,
    })
}

fn parse_feats(s: &str) -> Feats {
    let mut feats = Feats::default();
    for kv in s.split('|') {
        match kv {
            "Gender=Masc" => feats.gender = Some(Gender::Masc),
            "Gender=Fem" => feats.gender = Some(Gender::Fem),
            "Gender=Neut" => feats.gender = Some(Gender::Neut),
            "Number=Sing" => feats.number = Some(Number::Sing),
            "Number=Plur" => feats.number = Some(Number::Plur),
            _ => {}
        }
    }
    feats
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# sent_id = 1
# text = Les projets du renouveau politique.
1\tLes\tle\tDET\t_\tNumber=Plur\t2\tdet\t_\t_
2\tprojets\tprojet\tNOUN\t_\tGender=Masc|Number=Plur\t0\troot\t_\t_
3-4\tdu\t_\t_\t_\t_\t_\t_\t_\t_
3\tde\tde\tADP\t_\t_\t5\tcase\t_\t_
4\tle\tle\tDET\t_\t_\t5\tdet\t_\t_
5\trenouveau\trenouveau\tNOUN\t_\tGender=Masc|Number=Sing\t2\tnmod\t_\t_
5.1\tvide\tvide\tX\t_\t_\t_\t_\t_\t_
6\tpolitique\tpolitique\tADJ\t_\tGender=Masc|Number=Sing\t5\tamod\t_\t_
7\t.\t.\tPUNCT\t_\t_\t2\tpunct\t_\t_

";

    #[test]
    fn contractions_become_surface_tokens() {
        let s = &parse(SAMPLE.as_bytes()).unwrap()[0];
        assert_eq!(
            s.words(),
            ["Les", "projets", "du", "renouveau", "politique", "."]
        );
        assert_eq!(s.tokens[2].upos, Pos::Adp);
        // Les têtes sont réindexées sur les tokens de surface.
        assert_eq!(s.tokens[4].deprel, "amod");
        assert_eq!(s.tokens[4].head, Some(3));
        assert_eq!(s.tokens[3].head, Some(1));
        assert_eq!(s.tokens[1].head, None);
        assert_eq!(s.tokens[1].feats.number, Some(Number::Plur));
    }
}
