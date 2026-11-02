//! Préparation des données : sous-mots XLM-R, exemples d'entraînement, lots.
//!
//! Chaque phrase est représentée par ses mots précédés d'une position racine :
//! la position 0 est le token `<s>` de l'encodeur et sert de racine de l'arbre,
//! le mot k (1-indexé) est représenté par son premier sous-mot.

use candle_core::{Device, Tensor};
use nknown_core::analyze::{Feats, Gender, Number, Pos};
use nknown_nlp::conllu::GoldSentence;
use tokenizers::{EncodeInput, InputSequence, Tokenizer};

use crate::error::{Error, Result};

pub const N_UPOS: usize = Pos::ALL.len();
pub const GENDERS: [Option<Gender>; 4] = [
    None,
    Some(Gender::Masc),
    Some(Gender::Fem),
    Some(Gender::Neut),
];
pub const NUMBERS: [Option<Number>; 3] = [None, Some(Number::Sing), Some(Number::Plur)];

/// Valeur des cibles ignorées (contractions sans dépendance de référence).
pub const IGNORE: i64 = -1;

#[derive(Debug, Clone)]
pub struct Encoded {
    pub ids: Vec<u32>,
    /// Position du premier sous-mot de chaque mot ; `first[0] = 0` (racine = `<s>`).
    pub first: Vec<u32>,
}

impl Encoded {
    pub fn word_count(&self) -> usize {
        self.first.len() - 1
    }
}

/// Découpe une phrase déjà tokenisée en sous-mots. `None` si elle dépasse `max_subwords`.
pub fn encode(
    tokenizer: &Tokenizer,
    words: &[&str],
    max_subwords: usize,
) -> Result<Option<Encoded>> {
    let input = EncodeInput::Single(InputSequence::from(words.to_vec()));
    let enc = tokenizer.encode(input, true)?;
    if enc.get_ids().len() > max_subwords {
        return Ok(None);
    }
    let mut first = vec![u32::MAX; words.len() + 1];
    first[0] = 0;
    for (pos, w) in enc.get_word_ids().iter().enumerate() {
        if let Some(w) = w {
            let slot = &mut first[*w as usize + 1];
            if *slot == u32::MAX {
                *slot = pos as u32;
            }
        }
    }
    if first.contains(&u32::MAX) {
        return Err(Error::Format("mot sans sous-mot après tokenisation".into()));
    }
    Ok(Some(Encoded {
        ids: enc.get_ids().to_vec(),
        first,
    }))
}

/// Cibles de référence d'une phrase (une valeur par mot, sans la racine).
#[derive(Debug, Clone)]
pub struct Targets {
    pub upos: Vec<u32>,
    pub gender: Vec<u32>,
    pub number: Vec<u32>,
    /// Gouverneur : 0 = racine, k = mot k (1-indexé), `IGNORE` = non évalué.
    pub heads: Vec<i64>,
    /// Étiquette (index dans `deprels`), `IGNORE` = non évaluée.
    pub labels: Vec<i64>,
}

#[derive(Debug, Clone)]
pub struct Example {
    pub encoded: Encoded,
    pub targets: Targets,
}

pub fn feats_ids(f: Feats) -> (u32, u32) {
    let g = GENDERS.iter().position(|&g| g == f.gender).unwrap_or(0) as u32;
    let n = NUMBERS.iter().position(|&n| n == f.number).unwrap_or(0) as u32;
    (g, n)
}

pub fn deprel_vocab(sentences: &[GoldSentence]) -> Vec<String> {
    let mut labels: Vec<String> = sentences
        .iter()
        .flat_map(|s| s.tokens.iter().map(|t| t.deprel.clone()))
        .filter(|d| !d.is_empty())
        .collect();
    labels.sort();
    labels.dedup();
    labels
}

/// Exemples d'entraînement ; renvoie aussi le nombre de phrases écartées (trop longues).
pub fn examples(
    tokenizer: &Tokenizer,
    sentences: &[GoldSentence],
    deprels: &[String],
    max_subwords: usize,
) -> Result<(Vec<Example>, usize)> {
    let mut out = Vec::new();
    let mut skipped = 0;
    for s in sentences {
        let Some(encoded) = encode(tokenizer, &s.words(), max_subwords)? else {
            skipped += 1;
            continue;
        };
        let mut t = Targets {
            upos: Vec::new(),
            gender: Vec::new(),
            number: Vec::new(),
            heads: Vec::new(),
            labels: Vec::new(),
        };
        for tok in &s.tokens {
            let (g, n) = feats_ids(tok.feats);
            t.upos.push(tok.upos as u32);
            t.gender.push(g);
            t.number.push(n);
            if tok.deprel.is_empty() {
                t.heads.push(IGNORE);
                t.labels.push(IGNORE);
            } else {
                t.heads.push(tok.head.map_or(0, |h| h as i64 + 1));
                t.labels.push(
                    deprels
                        .iter()
                        .position(|d| *d == tok.deprel)
                        .map_or(IGNORE, |i| i as i64),
                );
            }
        }
        out.push(Example {
            encoded,
            targets: t,
        });
    }
    Ok((out, skipped))
}

/// Entrées d'un lot pour l'encodeur, complétées au plus long.
#[derive(Debug)]
pub struct Batch {
    pub input_ids: Tensor,
    pub attention_mask: Tensor,
    /// [B, N] : position du premier sous-mot de chaque mot (racine en 0, complété par 0).
    pub first: Tensor,
    /// Nombre de mots de chaque phrase (sans la racine).
    pub lengths: Vec<usize>,
    /// N = 1 + plus grand nombre de mots du lot.
    pub width: usize,
}

pub fn batch(encoded: &[&Encoded], device: &Device) -> Result<Batch> {
    let b = encoded.len();
    let t = encoded.iter().map(|e| e.ids.len()).max().unwrap_or(0);
    let width = encoded.iter().map(|e| e.first.len()).max().unwrap_or(1);
    let (mut ids, mut mask, mut first) = (Vec::new(), Vec::new(), Vec::new());
    for e in encoded {
        let pad = t - e.ids.len();
        ids.extend(e.ids.iter().copied().chain(std::iter::repeat_n(1, pad))); // 1 = <pad>
        mask.extend(std::iter::repeat_n(1u32, e.ids.len()).chain(std::iter::repeat_n(0, pad)));
        first.extend(
            e.first
                .iter()
                .copied()
                .chain(std::iter::repeat_n(0, width - e.first.len())),
        );
    }
    Ok(Batch {
        input_ids: Tensor::from_vec(ids, (b, t), device)?,
        attention_mask: Tensor::from_vec(mask, (b, t), device)?,
        first: Tensor::from_vec(first, (b, width), device)?,
        lengths: encoded.iter().map(|e| e.word_count()).collect(),
        width,
    })
}
