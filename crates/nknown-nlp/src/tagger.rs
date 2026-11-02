//! Couche 2 : étiqueteur morphosyntaxique (catégories UPOS).
//!
//! Perceptron moyenné, décodage glouton de gauche à droite. Les lectures du lexique
//! (couche 1) servent de traits, pas de contraintes : les conventions des ressources
//! et du corpus d'entraînement ne coïncident pas toujours (« que », « du »...),
//! le modèle apprend à les réconcilier.
//!
//! Les traits sont hachés (FNV-1a 64 bits, stable d'une version de Rust à l'autre)
//! pour que le modèle sauvegardé reste valide.
//!
//! Format de fichier : `MAGIC | u64 nombre de traits | (u64 hash, u8 n, n × (u8 classe, f32 poids))*`.

use std::collections::HashMap;
use std::path::Path;
use std::{fs, mem};

use nknown_core::analyze::Pos;

use crate::lexicon::{Lexicon, LexiconError, normalize};
use crate::tokenize;

const N: usize = Pos::ALL.len();
const START: u8 = N as u8;
const MAGIC: &[u8; 8] = b"NKTAG\0\0\x01";

/// Une phrase d'entraînement : mots et catégories de référence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrainingSentence {
    pub words: Vec<String>,
    pub tags: Vec<Pos>,
}

/// Modèle entraîné : pour chaque trait, les poids non nuls par classe.
pub struct Tagger {
    weights: HashMap<u64, Vec<(u8, f32)>>,
}

impl std::fmt::Debug for Tagger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tagger")
            .field("features", &self.weights.len())
            .finish()
    }
}

impl Tagger {
    /// Entraîne sur `sentences` pendant `iterations` passes. `on_epoch(passe, exactitude)`
    /// est appelé après chaque passe avec l'exactitude obtenue sur les données d'entraînement.
    pub fn train(
        sentences: &[TrainingSentence],
        lexicon: Option<&Lexicon>,
        iterations: usize,
        mut on_epoch: impl FnMut(usize, f64),
    ) -> Tagger {
        let contexts: Vec<Vec<WordCtx>> = sentences
            .iter()
            .map(|s| {
                let words: Vec<&str> = s.words.iter().map(String::as_str).collect();
                context(&words, lexicon)
            })
            .collect();
        let mut trainer = Trainer::default();
        let mut order: Vec<usize> = (0..sentences.len()).collect();
        let mut rng = XorShift(0x9e37_79b9_7f4a_7c15);
        let mut feats = Vec::new();
        for epoch in 1..=iterations {
            rng.shuffle(&mut order);
            let (mut correct, mut total) = (0usize, 0usize);
            for &s in &order {
                let (ctx, gold) = (&contexts[s], &sentences[s].tags);
                let (mut prev, mut prev2) = (START, START);
                for (i, &truth) in gold.iter().enumerate() {
                    features(ctx, i, prev, prev2, &mut feats);
                    let guess = argmax(&trainer.scores(&feats));
                    let truth = truth as u8;
                    trainer.step += 1;
                    if guess != truth {
                        trainer.update(&feats, truth, guess);
                    } else {
                        correct += 1;
                    }
                    total += 1;
                    // Entraînement sur les catégories prédites, comme au décodage.
                    (prev2, prev) = (prev, guess);
                }
            }
            on_epoch(epoch, correct as f64 / total.max(1) as f64);
        }
        trainer.average()
    }

    pub fn tag(&self, words: &[&str], lexicon: Option<&Lexicon>) -> Vec<Pos> {
        let ctx = context(words, lexicon);
        let mut feats = Vec::new();
        let (mut prev, mut prev2) = (START, START);
        let mut tags = Vec::with_capacity(words.len());
        for i in 0..words.len() {
            features(&ctx, i, prev, prev2, &mut feats);
            let mut scores = [0f32; N];
            for f in &feats {
                if let Some(ws) = self.weights.get(f) {
                    for &(class, w) in ws {
                        scores[usize::from(class)] += w;
                    }
                }
            }
            let guess = argmax(&scores);
            tags.push(Pos::ALL[usize::from(guess)]);
            (prev2, prev) = (prev, guess);
        }
        tags
    }

    pub fn feature_count(&self) -> usize {
        self.weights.len()
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut keys: Vec<&u64> = self.weights.keys().collect();
        keys.sort();
        let mut out = MAGIC.to_vec();
        out.extend_from_slice(&(keys.len() as u64).to_le_bytes());
        for k in keys {
            let ws = &self.weights[k];
            out.extend_from_slice(&k.to_le_bytes());
            out.push(ws.len() as u8);
            for &(class, w) in ws {
                out.push(class);
                out.extend_from_slice(&w.to_le_bytes());
            }
        }
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Tagger, LexiconError> {
        let bad = |m| LexiconError::Format(m);
        let mut r = bytes
            .strip_prefix(MAGIC.as_slice())
            .ok_or(bad("modèle : en-tête absent ou version inconnue"))?;
        let mut take = |n: usize| -> Result<&[u8], LexiconError> {
            if r.len() < n {
                return Err(bad("modèle tronqué"));
            }
            let (head, tail) = r.split_at(n);
            r = tail;
            Ok(head)
        };
        let count = u64::from_le_bytes(take(8)?.try_into().expect("8 octets"));
        let mut weights = HashMap::new();
        for _ in 0..count {
            let key = u64::from_le_bytes(take(8)?.try_into().expect("8 octets"));
            let n = take(1)?[0];
            let mut ws = Vec::with_capacity(usize::from(n));
            for _ in 0..n {
                let rec = take(5)?;
                if usize::from(rec[0]) >= N {
                    return Err(bad("modèle : classe inconnue"));
                }
                ws.push((
                    rec[0],
                    f32::from_le_bytes(rec[1..5].try_into().expect("4 octets")),
                ));
            }
            weights.insert(key, ws);
        }
        if !r.is_empty() {
            return Err(bad("modèle : octets en trop"));
        }
        Ok(Tagger { weights })
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), LexiconError> {
        fs::write(path, self.to_bytes())?;
        Ok(())
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Tagger, LexiconError> {
        Self::from_bytes(&fs::read(path)?)
    }
}

// ---------------------------------------------------------------- traits

/// Ce que le tagueur sait d'un mot, calculé une fois par phrase.
struct WordCtx {
    norm: String,
    shape: &'static str,
    /// Catégories proposées par le lexique (un bit par `Pos`).
    lex_mask: u32,
    /// Catégorie la plus fréquente selon le lexique, `START` si inconnu.
    lex_top: u8,
}

fn context(words: &[&str], lexicon: Option<&Lexicon>) -> Vec<WordCtx> {
    words
        .iter()
        .map(|w| {
            let (mut lex_mask, mut lex_top) = (0u32, START);
            if let Some(lex) = lexicon {
                for (k, e) in lex.lookup(w).enumerate() {
                    if k == 0 {
                        lex_top = e.pos as u8;
                    }
                    lex_mask |= 1 << (e.pos as u32);
                }
            }
            WordCtx {
                norm: normalize(w),
                shape: shape(w),
                lex_mask,
                lex_top,
            }
        })
        .collect()
}

fn shape(w: &str) -> &'static str {
    let first = w.chars().next().unwrap_or(' ');
    let letters = || w.chars().filter(|c| c.is_alphabetic());
    if !tokenize::is_word(w) {
        "p"
    } else if w.chars().all(|c| c.is_numeric()) {
        "d"
    } else if w.chars().any(|c| c.is_numeric()) {
        "dx"
    } else if letters().count() > 1 && letters().all(char::is_uppercase) {
        "X"
    } else if first.is_uppercase() {
        if w.contains('-') { "Xx-" } else { "Xx" }
    } else if w.contains('-') {
        "x-"
    } else {
        "x"
    }
}

fn suffix(s: &str, n: usize) -> &str {
    let start = s.char_indices().rev().nth(n - 1).map_or(0, |(i, _)| i);
    &s[start..]
}

fn prefix(s: &str, n: usize) -> &str {
    let end = s.char_indices().nth(n).map_or(s.len(), |(i, _)| i);
    &s[..end]
}

/// FNV-1a 64 bits, chaque champ suivi d'un séparateur.
struct Fnv(u64);

impl Fnv {
    fn new(template: u8) -> Self {
        Fnv(0xcbf2_9ce4_8422_2325).byte(template)
    }
    fn byte(mut self, b: u8) -> Self {
        self.0 ^= u64::from(b);
        self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        self
    }
    fn str(self, s: &str) -> Self {
        s.bytes().fold(self, Fnv::byte).byte(0xff)
    }
    fn num(self, v: u32) -> Self {
        v.to_le_bytes().into_iter().fold(self, Fnv::byte)
    }
}

fn features(ctx: &[WordCtx], i: usize, prev: u8, prev2: u8, out: &mut Vec<u64>) {
    const BOS: &str = "<s>";
    const EOS: &str = "</s>";
    let at = |k: isize| -> Option<&WordCtx> { ctx.get(i.checked_add_signed(k)?) };
    let word = |k: isize| at(k).map_or(if k < 0 { BOS } else { EOS }, |c| c.norm.as_str());
    let mask = |k: isize| at(k).map_or(u32::MAX, |c| c.lex_mask);
    let top = |k: isize| at(k).map_or(u32::from(START) + 1, |c| u32::from(c.lex_top));
    let w = &ctx[i];

    out.clear();
    out.extend([
        Fnv::new(0).0,
        Fnv::new(1).str(&w.norm).0,
        Fnv::new(2).str(suffix(&w.norm, 3)).0,
        Fnv::new(3).str(suffix(&w.norm, 2)).0,
        Fnv::new(4).str(suffix(&w.norm, 1)).0,
        Fnv::new(5).str(prefix(&w.norm, 2)).0,
        Fnv::new(6).str(w.shape).0,
        Fnv::new(7).num(u32::from(prev)).0,
        Fnv::new(8).num(u32::from(prev)).num(u32::from(prev2)).0,
        Fnv::new(9).num(u32::from(prev)).str(&w.norm).0,
        Fnv::new(10).str(word(-1)).0,
        Fnv::new(11).str(suffix(word(-1), 3)).0,
        Fnv::new(12).str(word(1)).0,
        Fnv::new(13).str(suffix(word(1), 3)).0,
        Fnv::new(14).str(word(-2)).0,
        Fnv::new(15).str(word(2)).0,
        Fnv::new(16).num(w.lex_mask).0,
        Fnv::new(17).num(u32::from(w.lex_top)).0,
        Fnv::new(18).num(mask(1)).0,
        Fnv::new(19).num(top(1)).0,
        Fnv::new(20).num(u32::from(prev)).num(w.lex_mask).0,
        Fnv::new(21).str(w.shape).num(u32::from(i == 0)).0,
        Fnv::new(22).str(&w.norm).str(word(1)).0,
        Fnv::new(23).str(word(-1)).str(&w.norm).0,
    ]);
}

fn argmax(scores: &[f32; N]) -> u8 {
    let mut best = 0;
    for (k, &s) in scores.iter().enumerate() {
        if s > scores[best] {
            best = k;
        }
    }
    best as u8
}

// ---------------------------------------------------------------- entraînement

#[derive(Clone, Copy)]
struct Param {
    class: u8,
    w: f32,
    /// Somme des poids au fil des pas, pour la moyenne.
    total: f64,
    /// Dernier pas où `total` a été mis à jour.
    ts: u64,
}

#[derive(Default)]
struct Trainer {
    params: HashMap<u64, Vec<Param>>,
    step: u64,
}

impl Trainer {
    fn scores(&self, feats: &[u64]) -> [f32; N] {
        let mut scores = [0f32; N];
        for f in feats {
            if let Some(ps) = self.params.get(f) {
                for p in ps {
                    scores[usize::from(p.class)] += p.w;
                }
            }
        }
        scores
    }

    fn update(&mut self, feats: &[u64], truth: u8, guess: u8) {
        let step = self.step;
        for &f in feats {
            let ps = self.params.entry(f).or_default();
            for (class, delta) in [(truth, 1.0), (guess, -1.0)] {
                let p = match ps.iter_mut().position(|p| p.class == class) {
                    Some(k) => &mut ps[k],
                    None => {
                        ps.push(Param {
                            class,
                            w: 0.0,
                            total: 0.0,
                            ts: step,
                        });
                        ps.last_mut().expect("vient d'être ajouté")
                    }
                };
                p.total += (step - p.ts) as f64 * f64::from(p.w);
                p.ts = step;
                p.w += delta;
            }
        }
    }

    fn average(mut self) -> Tagger {
        let step = self.step.max(1);
        let weights = mem::take(&mut self.params)
            .into_iter()
            .filter_map(|(f, ps)| {
                let ws: Vec<(u8, f32)> = ps
                    .into_iter()
                    .map(|p| {
                        let total = p.total + (step - p.ts) as f64 * f64::from(p.w);
                        (p.class, (total / step as f64) as f32)
                    })
                    .filter(|&(_, w)| w != 0.0)
                    .collect();
                (!ws.is_empty()).then_some((f, ws))
            })
            .collect();
        Tagger { weights }
    }
}

/// Générateur pseudo-aléatoire minimal et déterministe (mélange des phrases).
struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            let j = (self.next() % (i as u64 + 1)) as usize;
            v.swap(i, j);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sentence(pairs: &[(&str, Pos)]) -> TrainingSentence {
        TrainingSentence {
            words: pairs.iter().map(|(w, _)| w.to_string()).collect(),
            tags: pairs.iter().map(|(_, t)| *t).collect(),
        }
    }

    #[test]
    fn learns_context_dependent_tags() {
        use Pos::*;
        // « ferme » : nom après un déterminant, verbe après un pronom.
        let data = [
            sentence(&[("la", Det), ("ferme", Noun), ("brûle", Verb)]),
            sentence(&[("il", Pron), ("ferme", Verb), ("la", Det), ("porte", Noun)]),
            sentence(&[("une", Det), ("ferme", Noun), ("isolée", Adj)]),
            sentence(&[("elle", Pron), ("ferme", Verb), ("tard", Adv)]),
        ];
        let tagger = Tagger::train(&data, None, 10, |_, _| {});
        assert_eq!(tagger.tag(&["la", "ferme"], None), [Det, Noun]);
        assert_eq!(tagger.tag(&["il", "ferme"], None), [Pron, Verb]);

        let back = Tagger::from_bytes(&tagger.to_bytes()).unwrap();
        assert_eq!(back.tag(&["elle", "ferme"], None), [Pron, Verb]);
        assert!(Tagger::from_bytes(b"NKTAG").is_err());
    }

    #[test]
    fn affixes_are_char_based() {
        assert_eq!(suffix("été", 2), "té");
        assert_eq!(suffix("a", 3), "a");
        assert_eq!(prefix("été", 2), "ét");
        assert_eq!(shape("Paris"), "Xx");
        assert_eq!(shape("ONU"), "X");
        assert_eq!(shape("2024"), "d");
        assert_eq!(shape(","), "p");
    }
}
