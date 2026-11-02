//! Couche 1 : le lexique compilé.
//!
//! Associe chaque forme (en minuscules) à ses analyses possibles (lemme, catégorie, traits),
//! triées par fréquence décroissante. Les formes sont stockées dans un automate `fst`,
//! qui pointe vers une liste d'analyses.
//!
//! Format de fichier (entiers en little-endian) :
//!
//! ```text
//! MAGIC (8 octets) | len_fst u64 | len_postings u64 | len_analyses u64 | len_strings u64
//! fst       : forme -> position dans postings
//! postings  : par forme, varint nombre, puis pour chaque analyse : varint id, f32 fréquence
//! analyses  : 8 octets par analyse : u32 position du lemme dans strings, u8 pos, u8 genre, u8 nombre, u8 réservé
//! strings   : lemmes, chacun préfixé de sa longueur en varint
//! ```

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::{error, fmt, fs, io};

use nknown_core::analyze::{Feats, Gender, Number, Pos};

const MAGIC: &[u8; 8] = b"NKLEX\0\0\x02";
const HEADER_LEN: usize = MAGIC.len() + 4 * 8;
const ANALYSIS_LEN: usize = 8;

/// Une analyse possible d'une forme. Une forme peut en avoir plusieurs (« ferme »).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LexEntry<'a> {
    pub lemma: &'a str,
    pub pos: Pos,
    pub feats: Feats,
    /// Fréquence de cette lecture de la forme, par million de mots (0 si inconnue).
    pub freq: f32,
}

#[derive(Debug)]
pub enum LexiconError {
    Io(io::Error),
    Fst(fst::Error),
    Format(&'static str),
}

impl fmt::Display for LexiconError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LexiconError::Io(e) => write!(f, "erreur d'E/S : {e}"),
            LexiconError::Fst(e) => write!(f, "automate invalide : {e}"),
            LexiconError::Format(m) => write!(f, "fichier lexique invalide : {m}"),
        }
    }
}

impl error::Error for LexiconError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            LexiconError::Io(e) => Some(e),
            LexiconError::Fst(e) => Some(e),
            LexiconError::Format(_) => None,
        }
    }
}

impl From<io::Error> for LexiconError {
    fn from(e: io::Error) -> Self {
        LexiconError::Io(e)
    }
}

impl From<fst::Error> for LexiconError {
    fn from(e: fst::Error) -> Self {
        LexiconError::Fst(e)
    }
}

/// Accumule les couples (forme, analyse) puis produit un [`Lexicon`].
#[derive(Debug, Default)]
pub struct LexiconBuilder {
    /// Forme en minuscules -> ids d'analyses, dans l'ordre d'insertion, sans doublon.
    forms: BTreeMap<String, Vec<u32>>,
    analyses: Vec<(u32, Pos, Feats)>,
    analysis_ids: HashMap<(u32, Pos, Feats), u32>,
    strings: Vec<u8>,
    lemma_offsets: HashMap<String, u32>,
    /// (forme, lemme, catégorie) -> fréquence. Indépendante des traits : les ressources
    /// ne s'accordent pas toujours sur eux, et la fréquence doit survivre à l'élagage.
    freqs: HashMap<(String, u32, Pos), f32>,
}

impl LexiconBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ajoute une analyse pour `form`. Les doublons sont ignorés. À fréquence égale,
    /// l'ordre d'insertion est conservé.
    pub fn insert(&mut self, form: &str, lemma: &str, pos: Pos, feats: Feats) {
        self.insert_analysis(form, lemma, pos, feats);
    }

    /// Comme [`insert`](Self::insert), avec la fréquence de cette lecture (par million).
    /// Si plusieurs fréquences sont fournies pour une même lecture, la plus haute l'emporte.
    pub fn insert_with_frequency(
        &mut self,
        form: &str,
        lemma: &str,
        pos: Pos,
        feats: Feats,
        freq: f32,
    ) {
        let (key, lemma_off) = self.insert_analysis(form, lemma, pos, feats);
        let slot = self.freqs.entry((key, lemma_off, pos)).or_insert(0.0);
        *slot = slot.max(freq);
    }

    fn insert_analysis(
        &mut self,
        form: &str,
        lemma: &str,
        pos: Pos,
        feats: Feats,
    ) -> (String, u32) {
        let lemma_off = match self.lemma_offsets.get(lemma) {
            Some(&off) => off,
            None => {
                let off = u32::try_from(self.strings.len()).expect("table des lemmes > 4 Gio");
                write_varint(&mut self.strings, lemma.len() as u64);
                self.strings.extend_from_slice(lemma.as_bytes());
                self.lemma_offsets.insert(lemma.to_owned(), off);
                off
            }
        };
        let analysis = (lemma_off, pos, feats);
        let id = *self.analysis_ids.entry(analysis).or_insert_with(|| {
            self.analyses.push(analysis);
            u32::try_from(self.analyses.len() - 1).expect("plus de 4 milliards d'analyses")
        });
        let key = normalize(form);
        let ids = self.forms.entry(key.clone()).or_default();
        if !ids.contains(&id) {
            ids.push(id);
        }
        (key, lemma_off)
    }

    pub fn form_count(&self) -> usize {
        self.forms.len()
    }

    /// Retire les analyses moins précises qu'une autre de même lemme et catégorie :
    /// « livres » = livre/Noun/Plur est redondant avec livre/Noun/Masc,Plur.
    fn prune_subsumed(&self, ids: &[u32]) -> Vec<u32> {
        let get = |id: u32| self.analyses[id as usize];
        ids.iter()
            .copied()
            .filter(|&id| {
                let (lemma, pos, feats) = get(id);
                !ids.iter().any(|&other| {
                    let (l2, p2, f2) = get(other);
                    other != id && l2 == lemma && p2 == pos && refines(f2, feats)
                })
            })
            .collect()
    }

    pub fn build(self) -> Result<Lexicon, LexiconError> {
        let mut postings = Vec::new();
        let mut fst = fst::MapBuilder::memory();
        for (form, ids) in &self.forms {
            let mut ranked: Vec<(u32, f32)> = self
                .prune_subsumed(ids)
                .into_iter()
                .map(|id| {
                    let (lemma_off, pos, _) = self.analyses[id as usize];
                    let freq = self.freqs.get(&(form.clone(), lemma_off, pos));
                    (id, freq.copied().unwrap_or(0.0))
                })
                .collect();
            // Tri stable : à fréquence égale, l'ordre d'insertion est gardé.
            ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
            fst.insert(form, postings.len() as u64)?;
            write_varint(&mut postings, ranked.len() as u64);
            for (id, freq) in ranked {
                write_varint(&mut postings, u64::from(id));
                postings.extend_from_slice(&freq.to_le_bytes());
            }
        }
        let mut analyses = Vec::with_capacity(self.analyses.len() * ANALYSIS_LEN);
        for (lemma_off, pos, feats) in &self.analyses {
            analyses.extend_from_slice(&lemma_off.to_le_bytes());
            analyses.extend_from_slice(&[
                *pos as u8,
                encode_gender(feats.gender),
                encode_number(feats.number),
                0,
            ]);
        }
        Ok(Lexicon {
            fst: fst.into_map(),
            postings,
            analyses,
            strings: self.strings,
        })
    }
}

/// Lexique compilé, en lecture seule.
pub struct Lexicon {
    fst: fst::Map<Vec<u8>>,
    postings: Vec<u8>,
    analyses: Vec<u8>,
    strings: Vec<u8>,
}

impl fmt::Debug for Lexicon {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Lexicon")
            .field("forms", &self.form_count())
            .field("analyses", &self.analysis_count())
            .finish()
    }
}

impl Lexicon {
    /// Analyses possibles de `form` (comparaison en minuscules), dans l'ordre de préférence.
    pub fn lookup<'a>(&'a self, form: &str) -> impl Iterator<Item = LexEntry<'a>> + use<'a> {
        let mut rest: &[u8] = match self.fst.get(normalize(form)) {
            Some(off) => usize::try_from(off)
                .ok()
                .and_then(|o| self.postings.get(o..))
                .unwrap_or(&[]),
            None => &[],
        };
        let count = read_varint(&mut rest).unwrap_or(0);
        (0..count).map_while(move |_| {
            let id = read_varint(&mut rest)?;
            let (freq, tail) = rest.split_first_chunk::<4>()?;
            rest = tail;
            self.analysis(id, f32::from_le_bytes(*freq))
        })
    }

    fn analysis(&self, id: u64, freq: f32) -> Option<LexEntry<'_>> {
        let start = usize::try_from(id).ok()?.checked_mul(ANALYSIS_LEN)?;
        let rec = self.analyses.get(start..start + ANALYSIS_LEN)?;
        let lemma_off = u32::from_le_bytes(rec[..4].try_into().ok()?) as usize;
        let mut s = self.strings.get(lemma_off..)?;
        let len = usize::try_from(read_varint(&mut s)?).ok()?;
        let lemma = std::str::from_utf8(s.get(..len)?).ok()?;
        Some(LexEntry {
            lemma,
            pos: Pos::from_u8(rec[4])?,
            feats: Feats {
                gender: decode_gender(rec[5])?,
                number: decode_number(rec[6])?,
            },
            freq,
        })
    }

    pub fn form_count(&self) -> usize {
        self.fst.len()
    }

    pub fn analysis_count(&self) -> usize {
        self.analyses.len() / ANALYSIS_LEN
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let fst = self.fst.as_fst().as_bytes();
        let sections = [fst, &self.postings, &self.analyses, &self.strings];
        let total = HEADER_LEN + sections.iter().map(|s| s.len()).sum::<usize>();
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(MAGIC);
        for s in sections {
            out.extend_from_slice(&(s.len() as u64).to_le_bytes());
        }
        for s in sections {
            out.extend_from_slice(s);
        }
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, LexiconError> {
        if bytes.len() < HEADER_LEN || &bytes[..MAGIC.len()] != MAGIC {
            return Err(LexiconError::Format("en-tête absent ou version inconnue"));
        }
        let mut lens = [0usize; 4];
        for (i, len) in lens.iter_mut().enumerate() {
            let at = MAGIC.len() + i * 8;
            let raw = u64::from_le_bytes(bytes[at..at + 8].try_into().expect("8 octets"));
            *len = usize::try_from(raw).map_err(|_| LexiconError::Format("taille de section"))?;
        }
        let mut rest = &bytes[HEADER_LEN..];
        let mut take = |len: usize| -> Result<Vec<u8>, LexiconError> {
            if rest.len() < len {
                return Err(LexiconError::Format("fichier tronqué"));
            }
            let (head, tail) = rest.split_at(len);
            rest = tail;
            Ok(head.to_vec())
        };
        let fst = fst::Map::new(take(lens[0])?)?;
        let postings = take(lens[1])?;
        let analyses = take(lens[2])?;
        let strings = take(lens[3])?;
        if !rest.is_empty() {
            return Err(LexiconError::Format("octets en trop"));
        }
        if analyses.len() % ANALYSIS_LEN != 0 {
            return Err(LexiconError::Format("table des analyses mal alignée"));
        }
        Ok(Self {
            fst,
            postings,
            analyses,
            strings,
        })
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), LexiconError> {
        fs::write(path, self.to_bytes())?;
        Ok(())
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, LexiconError> {
        Self::from_bytes(&fs::read(path)?)
    }
}

/// Clé de recherche : minuscules, apostrophe typographique ramenée à `'`.
pub(crate) fn normalize(form: &str) -> String {
    form.to_lowercase().replace('\u{2019}', "'")
}

/// `a` est au moins aussi précis que `b` et compatible avec lui.
fn refines(a: Feats, b: Feats) -> bool {
    (b.gender.is_none() || b.gender == a.gender) && (b.number.is_none() || b.number == a.number)
}

fn encode_gender(g: Option<Gender>) -> u8 {
    match g {
        None => 0,
        Some(Gender::Masc) => 1,
        Some(Gender::Fem) => 2,
        Some(Gender::Neut) => 3,
    }
}

/// `None` = octet invalide ; `Some(None)` = genre non renseigné.
fn decode_gender(b: u8) -> Option<Option<Gender>> {
    Some(match b {
        0 => None,
        1 => Some(Gender::Masc),
        2 => Some(Gender::Fem),
        3 => Some(Gender::Neut),
        _ => return None,
    })
}

fn encode_number(n: Option<Number>) -> u8 {
    match n {
        None => 0,
        Some(Number::Sing) => 1,
        Some(Number::Plur) => 2,
    }
}

fn decode_number(b: u8) -> Option<Option<Number>> {
    Some(match b {
        0 => None,
        1 => Some(Number::Sing),
        2 => Some(Number::Plur),
        _ => return None,
    })
}

/// Entier non signé encodé en LEB128.
fn write_varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn read_varint(input: &mut &[u8]) -> Option<u64> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let (&byte, rest) = input.split_first()?;
        *input = rest;
        v |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feats(gender: Option<Gender>, number: Option<Number>) -> Feats {
        Feats { gender, number }
    }

    fn sample() -> Lexicon {
        let mut b = LexiconBuilder::new();
        let m_sg = feats(Some(Gender::Masc), Some(Number::Sing));
        let f_sg = feats(Some(Gender::Fem), Some(Number::Sing));
        b.insert("livre", "livre", Pos::Noun, m_sg);
        b.insert("livre", "livre", Pos::Noun, f_sg);
        b.insert("livre", "livrer", Pos::Verb, Feats::default());
        b.insert("livre", "livre", Pos::Noun, m_sg); // doublon ignoré
        b.insert("Paris", "Paris", Pos::Propn, Feats::default());
        b.insert(
            "paris",
            "pari",
            Pos::Noun,
            feats(Some(Gender::Masc), Some(Number::Plur)),
        );
        b.build().unwrap()
    }

    #[test]
    fn lookup_returns_all_analyses_in_order() {
        let lex = sample();
        let livre: Vec<_> = lex.lookup("Livre").collect();
        assert_eq!(livre.len(), 3);
        assert_eq!(livre[0].feats.gender, Some(Gender::Masc));
        assert_eq!(livre[1].feats.gender, Some(Gender::Fem));
        assert_eq!((livre[2].lemma, livre[2].pos), ("livrer", Pos::Verb));

        let paris: Vec<_> = lex.lookup("paris").map(|e| e.lemma).collect();
        assert_eq!(paris, ["Paris", "pari"]);
        assert_eq!(lex.lookup("inconnu").count(), 0);
    }

    #[test]
    fn less_specific_analyses_are_pruned() {
        let mut b = LexiconBuilder::new();
        let plur = feats(None, Some(Number::Plur));
        b.insert(
            "rues",
            "rue",
            Pos::Noun,
            feats(Some(Gender::Fem), Some(Number::Plur)),
        );
        b.insert("rues", "rue", Pos::Noun, plur);
        b.insert("rues", "ruer", Pos::Verb, feats(None, Some(Number::Sing)));
        b.insert("calmes", "calme", Pos::Adj, plur);
        let lex = b.build().unwrap();
        let rues: Vec<_> = lex.lookup("rues").map(|e| (e.lemma, e.pos)).collect();
        assert_eq!(rues, [("rue", Pos::Noun), ("ruer", Pos::Verb)]);
        assert_eq!(lex.lookup("calmes").count(), 1);
    }

    #[test]
    fn analyses_are_ranked_by_frequency() {
        let mut b = LexiconBuilder::new();
        let m_sg = feats(Some(Gender::Masc), Some(Number::Sing));
        // Wiktextract d'abord, sans fréquence...
        b.insert("été", "été", Pos::Noun, m_sg);
        b.insert("été", "être", Pos::Verb, Feats::default());
        // ...puis Lexique, avec fréquences et traits moins précis pour le nom.
        b.insert_with_frequency(
            "été",
            "été",
            Pos::Noun,
            feats(None, Some(Number::Sing)),
            61.8,
        );
        b.insert_with_frequency("été", "être", Pos::Verb, Feats::default(), 1041.2);
        let lex = b.build().unwrap();

        let got: Vec<_> = lex
            .lookup("été")
            .map(|e| (e.lemma, e.feats, e.freq))
            .collect();
        assert_eq!(
            got,
            [("être", Feats::default(), 1041.2), ("été", m_sg, 61.8)]
        );
    }

    #[test]
    fn apostrophes_are_normalized() {
        let mut b = LexiconBuilder::new();
        b.insert("aujourd'hui", "aujourd'hui", Pos::Adv, Feats::default());
        let lex = b.build().unwrap();
        assert_eq!(lex.lookup("Aujourd\u{2019}hui").count(), 1);
    }

    #[test]
    fn bytes_roundtrip() {
        let lex = sample();
        let back = Lexicon::from_bytes(&lex.to_bytes()).unwrap();
        assert_eq!(back.form_count(), lex.form_count());
        assert!(lex.lookup("livre").eq(back.lookup("livre")));
    }

    #[test]
    fn rejects_corrupted_files() {
        let bytes = sample().to_bytes();
        assert!(Lexicon::from_bytes(b"pas un lexique").is_err());
        assert!(Lexicon::from_bytes(&bytes[..bytes.len() - 1]).is_err());
        let mut bad_magic = bytes.clone();
        bad_magic[0] = b'X';
        assert!(Lexicon::from_bytes(&bad_magic).is_err());
    }

    #[test]
    fn varint_roundtrip() {
        for v in [0, 1, 127, 128, 300, u64::from(u32::MAX), u64::MAX] {
            let mut buf = Vec::new();
            write_varint(&mut buf, v);
            assert_eq!(read_varint(&mut buf.as_slice()), Some(v));
        }
    }
}
