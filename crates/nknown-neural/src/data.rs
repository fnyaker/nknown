//! Préparation des données : sous-mots XLM-R, exemples d'entraînement, lots.
//!
//! Chaque phrase est représentée par ses mots précédés d'une position racine :
//! la position 0 est le token `<s>` de l'encodeur et sert de racine de l'arbre,
//! le mot k (1-indexé) est représenté par son premier sous-mot.

use crate::tokenizer::Tokenizer;
use candle_core::{Device, Tensor};
use nknown_core::analyze::{Feats, Gender, Number, Pos};
use nknown_nlp::conllu::GoldSentence;

use crate::error::Result;

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
    let enc = tokenizer.encode_words(words);
    if enc.ids.len() > max_subwords {
        return Ok(None);
    }
    let mut first = vec![u32::MAX; words.len() + 1];
    first[0] = 0;
    for (pos, w) in enc.word_ids.iter().enumerate() {
        if let Some(w) = w {
            let slot = &mut first[*w as usize + 1];
            if *slot == u32::MAX {
                *slot = pos as u32;
            }
        }
    }
    // Un mot sans sous-mot (caractère invisible, ou supprimé par la normalisation du
    // tokeniseur) prend la position du mot précédent plutôt que de faire échouer la phrase.
    for w in 1..first.len() {
        if first[w] == u32::MAX {
            first[w] = first[w - 1];
        }
    }
    Ok(Some(Encoded {
        ids: enc.ids,
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

/// Entrées d'un lot, côté hôte, complétées au plus long (communes aux deux moteurs).
#[derive(Debug, Clone)]
pub struct HostBatch {
    /// [B × T] identifiants de sous-mots (1 = remplissage).
    pub ids: Vec<u32>,
    /// [B × T] 1 pour un vrai sous-mot, 0 pour le remplissage.
    pub mask: Vec<u32>,
    /// [B × N] position du premier sous-mot de chaque mot (racine en 0, complété par 0).
    pub first: Vec<u32>,
    pub batch_size: usize,
    /// T : nombre de sous-mots de la plus longue phrase.
    pub seq_len: usize,
    /// N = 1 + plus grand nombre de mots du lot.
    pub width: usize,
    /// Nombre de mots de chaque phrase (sans la racine).
    pub lengths: Vec<usize>,
}

impl HostBatch {
    pub fn new(encoded: &[&Encoded]) -> Self {
        let t = encoded.iter().map(|e| e.ids.len()).max().unwrap_or(0);
        let width = encoded.iter().map(|e| e.first.len()).max().unwrap_or(1);
        let (mut ids, mut mask, mut first) = (Vec::new(), Vec::new(), Vec::new());
        for e in encoded {
            let pad = t - e.ids.len();
            ids.extend(e.ids.iter().copied().chain(std::iter::repeat_n(1, pad)));
            mask.extend(std::iter::repeat_n(1u32, e.ids.len()).chain(std::iter::repeat_n(0, pad)));
            first.extend(
                e.first
                    .iter()
                    .copied()
                    .chain(std::iter::repeat_n(0, width - e.first.len())),
            );
        }
        Self {
            ids,
            mask,
            first,
            batch_size: encoded.len(),
            seq_len: t,
            width,
            lengths: encoded.iter().map(|e| e.word_count()).collect(),
        }
    }

    /// Positions RoBERTa : `pad_id + 1 + rang` pour les vrais sous-mots, `pad_id` sinon.
    pub fn positions(&self, pad_id: u32) -> Vec<u32> {
        self.mask
            .chunks(self.seq_len.max(1))
            .flat_map(|row| {
                row.iter().enumerate().map(move |(i, &m)| {
                    if m == 1 {
                        pad_id + 1 + i as u32
                    } else {
                        pad_id
                    }
                })
            })
            .collect()
    }
}

/// Lot converti en tenseurs candle.
#[derive(Debug)]
pub struct Batch {
    pub input_ids: Tensor,
    pub attention_mask: Tensor,
    /// [B, N]
    pub first: Tensor,
    pub lengths: Vec<usize>,
    pub width: usize,
}

pub fn batch(encoded: &[&Encoded], device: &Device) -> Result<Batch> {
    let h = HostBatch::new(encoded);
    let (b, t) = (h.batch_size, h.seq_len);
    Ok(Batch {
        input_ids: Tensor::from_vec(h.ids, (b, t), device)?,
        attention_mask: Tensor::from_vec(h.mask, (b, t), device)?,
        first: Tensor::from_vec(h.first, (b, h.width), device)?,
        lengths: h.lengths,
        width: h.width,
    })
}

/// Arbres de dépendances (Chu-Liu-Edmonds) à partir des log-probabilités d'arcs
/// `arcs[b][d][h]` ; renvoie pour chaque phrase `tree[d]` (0 = racine, mots 1-indexés).
pub fn decode_trees(arcs: &[Vec<Vec<f32>>], lengths: &[usize]) -> Vec<Vec<usize>> {
    lengths
        .iter()
        .enumerate()
        .map(|(b, &n)| {
            let matrix: Vec<Vec<f32>> = (0..=n)
                .map(|d| {
                    (0..=n)
                        .map(|h| {
                            if h == d {
                                f32::NEG_INFINITY
                            } else {
                                arcs[b][d][h]
                            }
                        })
                        .collect()
                })
                .collect();
            crate::mst::decode(&matrix)
        })
        .collect()
}

/// Assemble les analyses à partir des prédictions par position (racine en 0).
pub fn assemble(
    lengths: &[usize],
    upos: &[Vec<u32>],
    gender: &[Vec<u32>],
    number: &[Vec<u32>],
    trees: &[Vec<usize>],
    labels: &[Vec<u32>],
    deprels: &[String],
) -> Vec<nknown_nlp::ParsedSentence> {
    lengths
        .iter()
        .enumerate()
        .map(|(b, &n)| {
            let words = 1..=n;
            nknown_nlp::ParsedSentence {
                upos: words
                    .clone()
                    .map(|d| Pos::ALL[upos[b][d] as usize])
                    .collect(),
                feats: words
                    .clone()
                    .map(|d| Feats {
                        gender: GENDERS[gender[b][d] as usize],
                        number: NUMBERS[number[b][d] as usize],
                    })
                    .collect(),
                heads: words.clone().map(|d| trees[b][d].checked_sub(1)).collect(),
                deprels: words
                    .map(|d| deprels[labels[b][d] as usize].clone())
                    .collect(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_without_subwords_do_not_fail() {
        let tok =
            Tokenizer::from_file(std::env::var("NKNOWN_XLMR_TOKENIZER").unwrap_or_else(|_| {
                format!(
                    "{}/.local/share/nknown/xlm-roberta-base/tokenizer.json",
                    std::env::var("HOME").unwrap_or_default()
                )
            }));
        let Ok(tok) = tok else {
            eprintln!("tokeniseur XLM-R absent : test ignoré");
            return;
        };
        let e = encode(&tok, &["Bonjour", "\u{200b}", "monde"], 64)
            .unwrap()
            .unwrap();
        assert_eq!(e.first.len(), 4);
        assert!(e.first.iter().all(|&p| p != u32::MAX));
    }
}
