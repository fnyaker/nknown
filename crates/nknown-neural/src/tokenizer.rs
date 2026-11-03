//! Tokeniseur SentencePiece Unigram (celui d'XLM-R), compact.
//!
//! Reproduit exactement la chaîne de `tokenizers` pour des mots déjà découpés : jetons spéciaux,
//! normalisation précompilée (table de caractères de SentencePiece, par graphème), découpage aux
//! espaces, marque `▁` devant chaque morceau et coupure sur `▁`, puis segmentation de Viterbi
//! (inconnus fusionnés, pénalité de 10). L'équivalence est vérifiée sur les corpus UD
//! (`tests::same_as_huggingface`).
//!
//! Les 250 002 sous-mots tiennent dans un automate `fst` (quelques Mo) au lieu d'un trie de
//! tables de hachage (~280 Mo). `tokenizer.json` n'est lu qu'une fois : sa forme compacte est
//! enregistrée à côté (`tokenizer.nkt`) et relue directement ensuite.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use fst::raw::Output;
use serde::Deserialize;
use spm_precompiled::Precompiled;

use crate::error::{Error, Result};

/// Pénalité d'un caractère inconnu, sous le plus petit score (SentencePiece, `tokenizers`).
const UNK_PENALTY: f64 = 10.0;
/// Marque de début de mot de SentencePiece.
const SPACE: char = '▁';
const CACHE: &str = "tokenizer.nkt";
const MAGIC: &[u8; 8] = b"NKTOK\x00\x00\x01";

/// Sous-mots d'une phrase, avec le mot d'origine de chacun (`None` : `<s>`, `</s>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encoding {
    pub ids: Vec<u32>,
    pub word_ids: Vec<Option<u32>>,
}

pub struct Tokenizer {
    /// Sous-mot → identifiant.
    pieces: fst::Map<Vec<u8>>,
    scores: Vec<f64>,
    unk_score: f64,
    unk_id: u32,
    bos_id: u32,
    eos_id: u32,
    /// Jetons spéciaux reconnus tels quels dans le texte (`<s>`, `<mask>`…).
    specials: Vec<(String, u32)>,
    normalizer: Option<Precompiled>,
    /// Table de normalisation brute, pour la forme compacte.
    charsmap: Vec<u8>,
}

impl std::fmt::Debug for Tokenizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tokenizer")
            .field("pieces", &self.scores.len())
            .finish_non_exhaustive()
    }
}

// ------------------------------------------------------------------ tokenizer.json

#[derive(Deserialize)]
struct Json {
    normalizer: Option<JsonNormalizer>,
    pre_tokenizer: Option<serde_json::Value>,
    post_processor: Option<JsonPostProcessor>,
    model: JsonModel,
    #[serde(default)]
    added_tokens: Vec<JsonAdded>,
}

#[derive(Deserialize)]
struct JsonNormalizer {
    #[serde(rename = "type")]
    kind: String,
    precompiled_charsmap: Option<String>,
}

#[derive(Deserialize)]
struct JsonPostProcessor {
    special_tokens: BTreeMap<String, JsonSpecial>,
}

#[derive(Deserialize)]
struct JsonSpecial {
    ids: Vec<u32>,
}

#[derive(Deserialize)]
struct JsonModel {
    #[serde(rename = "type", default)]
    kind: Option<String>,
    unk_id: Option<u32>,
    vocab: Vec<(String, f64)>,
    #[serde(default)]
    byte_fallback: bool,
}

#[derive(Deserialize)]
struct JsonAdded {
    id: u32,
    content: String,
    #[serde(default)]
    special: bool,
    #[serde(default)]
    normalized: bool,
}

fn unsupported(what: impl std::fmt::Display) -> Error {
    Error::Tokenizer(format!("tokenizer.json non pris en charge : {what}"))
}

/// Seule la chaîne d'XLM-R (et des modèles SentencePiece qui la partagent) est reproduite.
fn check_pre_tokenizer(v: &serde_json::Value) -> Result<()> {
    let seq = v
        .get("pretokenizers")
        .and_then(|p| p.as_array())
        .ok_or_else(|| unsupported("pré-découpage"))?;
    let kinds: Vec<&str> = seq
        .iter()
        .filter_map(|p| p.get("type").and_then(|t| t.as_str()))
        .collect();
    let meta = &seq[seq.len().saturating_sub(1)];
    let prepend = meta.get("prepend_scheme").and_then(|p| p.as_str());
    let add_prefix = meta.get("add_prefix_space").and_then(|p| p.as_bool());
    let always = prepend == Some("always") || (prepend.is_none() && add_prefix != Some(false));
    let replacement = meta.get("replacement").and_then(|r| r.as_str());
    if kinds != ["WhitespaceSplit", "Metaspace"] || !always || replacement != Some("▁") {
        return Err(unsupported(format!("pré-découpage {kinds:?}")));
    }
    Ok(())
}

impl Tokenizer {
    /// Charge `tokenizer.json`, ou sa forme compacte `tokenizer.nkt` si elle est à jour.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let cache = path.with_file_name(CACHE);
        let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
        if let (Some(c), Some(j)) = (modified(&cache), modified(path))
            && c >= j
            && let Ok(t) = Self::read_compact(&cache)
        {
            return Ok(t);
        }
        let t = Self::from_json(&std::fs::read(path)?)?;
        if let Err(e) = t.write_compact(&cache) {
            // Simple cache : sans lui, tokenizer.json sera relu la prochaine fois.
            eprintln!("{} : {e}", cache.display());
        }
        Ok(t)
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        let json: Json = serde_json::from_slice(bytes)?;
        if json.model.kind.as_deref().is_some_and(|k| k != "Unigram") || json.model.byte_fallback {
            return Err(unsupported(
                "modèle autre qu'Unigram sans repli sur les octets",
            ));
        }
        if let Some(p) = &json.pre_tokenizer {
            check_pre_tokenizer(p)?;
        }
        let normalizer = match json.normalizer {
            None => None,
            Some(n) if n.kind == "Precompiled" => {
                let map = n.precompiled_charsmap.unwrap_or_default();
                let bytes = base64_decode(&map)?;
                Some(bytes)
            }
            Some(n) => return Err(unsupported(format!("normalisation {}", n.kind))),
        };
        let special_id = |name: &str| -> Result<u32> {
            json.post_processor
                .as_ref()
                .and_then(|p| p.special_tokens.get(name))
                .and_then(|s| s.ids.first().copied())
                .ok_or_else(|| unsupported(format!("jeton {name} absent")))
        };
        let (bos_id, eos_id) = (special_id("<s>")?, special_id("</s>")?);
        let unk_id = json
            .model
            .unk_id
            .ok_or_else(|| unsupported("pas d'identifiant pour <unk>"))?;
        let specials = json
            .added_tokens
            .into_iter()
            .filter(|a| a.special && !a.normalized)
            .map(|a| (a.content, a.id))
            .collect();

        let vocab = json.model.vocab;
        let min_score = vocab.iter().map(|(_, s)| *s).fold(f64::INFINITY, f64::min);
        let scores: Vec<f64> = vocab.iter().map(|(_, s)| *s).collect();
        // Un sous-mot présent deux fois garde son dernier identifiant (comme `tokenizers`).
        let mut sorted: Vec<(String, u32)> = vocab
            .into_iter()
            .enumerate()
            .map(|(id, (piece, _))| (piece, id as u32))
            .collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
        sorted.dedup_by(|later, kept| later.0 == kept.0);
        let pieces = fst::Map::from_iter(sorted.into_iter().map(|(p, id)| (p, u64::from(id))))
            .map_err(|e| Error::Tokenizer(e.to_string()))?;
        Self::assemble(
            pieces,
            scores,
            min_score,
            [unk_id, bos_id, eos_id],
            specials,
            normalizer,
        )
    }

    fn assemble(
        pieces: fst::Map<Vec<u8>>,
        scores: Vec<f64>,
        min_score: f64,
        [unk_id, bos_id, eos_id]: [u32; 3],
        specials: Vec<(String, u32)>,
        normalizer: Option<Vec<u8>>,
    ) -> Result<Self> {
        let charsmap = normalizer.unwrap_or_default();
        let normalizer = (!charsmap.is_empty())
            .then(|| Precompiled::from(&charsmap).map_err(|e| Error::Tokenizer(e.to_string())))
            .transpose()?;
        Ok(Self {
            pieces,
            scores,
            unk_score: min_score - UNK_PENALTY,
            unk_id,
            bos_id,
            eos_id,
            specials,
            normalizer,
            charsmap,
        })
    }

    // ------------------------------------------------------------------ forme compacte

    fn write_compact(&self, path: &Path) -> Result<()> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        for v in [self.unk_id, self.bos_id, self.eos_id] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&(self.unk_score + UNK_PENALTY).to_le_bytes());
        out.extend_from_slice(&(self.specials.len() as u32).to_le_bytes());
        for (s, id) in &self.specials {
            out.extend_from_slice(&id.to_le_bytes());
            put_bytes(&mut out, s.as_bytes());
        }
        put_bytes(&mut out, &self.charsmap);
        out.extend_from_slice(&(self.scores.len() as u32).to_le_bytes());
        for s in &self.scores {
            out.extend_from_slice(&s.to_le_bytes());
        }
        put_bytes(&mut out, self.pieces.as_fst().as_bytes());
        // Écriture atomique : un fichier interrompu ne passe jamais pour le cache.
        let tmp = path.with_extension("nkt.tmp");
        std::fs::File::create(&tmp)?.write_all(&out)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    fn read_compact(path: &Path) -> Result<Self> {
        let data = std::fs::read(path)?;
        let mut r = Reader {
            data: &data,
            pos: 0,
        };
        if r.take(MAGIC.len())? != MAGIC {
            return Err(Error::Format(format!(
                "{} : format inconnu",
                path.display()
            )));
        }
        let ids = [r.u32()?, r.u32()?, r.u32()?];
        let min_score = r.f64()?;
        let specials = (0..r.u32()?)
            .map(|_| {
                let id = r.u32()?;
                let s = String::from_utf8(r.bytes()?.to_vec())
                    .map_err(|e| Error::Format(e.to_string()))?;
                Ok((s, id))
            })
            .collect::<Result<Vec<_>>>()?;
        let charsmap = r.bytes()?.to_vec();
        let scores = (0..r.u32()?).map(|_| r.f64()).collect::<Result<Vec<_>>>()?;
        let pieces =
            fst::Map::new(r.bytes()?.to_vec()).map_err(|e| Error::Format(e.to_string()))?;
        Self::assemble(pieces, scores, min_score, ids, specials, Some(charsmap))
    }

    // ------------------------------------------------------------------ découpage

    /// Sous-mots d'une phrase déjà découpée en mots, entre `<s>` et `</s>`.
    pub fn encode_words(&self, words: &[&str]) -> Encoding {
        let mut enc = Encoding {
            ids: vec![self.bos_id],
            word_ids: vec![None],
        };
        let mut ids = Vec::new();
        for (w, word) in words.iter().enumerate() {
            ids.clear();
            self.encode_word(word, &mut ids);
            enc.word_ids
                .extend(std::iter::repeat_n(Some(w as u32), ids.len()));
            enc.ids.extend_from_slice(&ids);
        }
        enc.ids.push(self.eos_id);
        enc.word_ids.push(None);
        enc
    }

    fn encode_word(&self, word: &str, out: &mut Vec<u32>) {
        let mut rest = word;
        while !rest.is_empty() {
            // Jeton spécial le plus à gauche (le plus long à égalité), reconnu avant toute
            // normalisation.
            let found = self
                .specials
                .iter()
                .filter_map(|(s, id)| rest.find(s.as_str()).map(|at| (at, s.len(), *id)))
                .min_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
            let (text, special) = match found {
                Some((at, len, id)) => {
                    let text = &rest[..at];
                    rest = &rest[at + len..];
                    (text, Some(id))
                }
                None => (std::mem::take(&mut rest), None),
            };
            self.encode_text(text, out);
            out.extend(special);
        }
    }

    fn encode_text(&self, text: &str, out: &mut Vec<u32>) {
        if text.is_empty() {
            return;
        }
        let normalized;
        let text = match &self.normalizer {
            Some(n) => {
                normalized = n.normalize_string(text);
                normalized.as_str()
            }
            None => text,
        };
        let mut piece = String::new();
        for chunk in text.split(char::is_whitespace).filter(|c| !c.is_empty()) {
            piece.clear();
            if !chunk.starts_with(SPACE) {
                piece.push(SPACE);
            }
            piece.push_str(chunk);
            // Coupure sur `▁`, la marque restant collée à ce qui la suit.
            let mut start = 0;
            for (i, _) in piece.match_indices(SPACE).filter(|&(i, _)| i > 0) {
                self.viterbi(&piece[start..i], out);
                start = i;
            }
            self.viterbi(&piece[start..], out);
        }
    }

    /// Segmentation de meilleur score (algorithme et ordre de parcours de `tokenizers`, donc
    /// mêmes choix à égalité). Les inconnus consécutifs forment un seul `<unk>`.
    fn viterbi(&self, s: &str, out: &mut Vec<u32>) {
        #[derive(Clone, Copy)]
        struct Best {
            id: u32,
            score: f64,
            from: Option<usize>,
        }
        let bytes = s.as_bytes();
        let mut best = vec![
            Best {
                id: 0,
                score: 0.0,
                from: None,
            };
            bytes.len() + 1
        ];
        let fst = self.pieces.as_fst();
        let mut start = 0;
        while start < bytes.len() {
            let here = best[start].score;
            let char_len = s[start..].chars().next().map_or(1, char::len_utf8);
            let mut single = false;
            let (mut node, mut output) = (fst.root(), Output::zero());
            for (k, &b) in bytes[start..].iter().enumerate() {
                let Some(t) = node.find_input(b).map(|i| node.transition(i)) else {
                    break;
                };
                output = output.cat(t.out);
                node = fst.node(t.addr);
                if node.is_final() {
                    let id = output.cat(node.final_output()).value() as u32;
                    let end = start + k + 1;
                    let score = here + self.scores[id as usize];
                    let target = &mut best[end];
                    if target.from.is_none() || score > target.score {
                        *target = Best {
                            id,
                            score,
                            from: Some(start),
                        };
                    }
                    single |= k + 1 == char_len;
                }
            }
            if !single {
                let target = &mut best[start + char_len];
                let score = here + self.unk_score;
                if target.from.is_none() || score > target.score {
                    *target = Best {
                        id: self.unk_id,
                        score,
                        from: Some(start),
                    };
                }
            }
            start += char_len;
        }
        let first = out.len();
        let mut end = bytes.len();
        while end > 0 {
            let node = best[end];
            // Inconnus consécutifs fusionnés : la chaîne fusionnée, si c'est un sous-mot
            // connu, en prend l'identifiant (comme `tokenizers`).
            if node.id == self.unk_id {
                let mut begin = node.from.unwrap_or(0);
                while begin > 0 && best[begin].id == self.unk_id {
                    begin = best[begin].from.unwrap_or(0);
                }
                let fused = self
                    .pieces
                    .get(&s[begin..end])
                    .map_or(self.unk_id, |id| id as u32);
                out.push(fused);
                end = begin;
            } else {
                out.push(node.id);
                end = node.from.unwrap_or(0);
            }
        }
        out[first..].reverse();
    }
}

fn put_bytes(out: &mut Vec<u8>, b: &[u8]) {
    out.extend_from_slice(&(b.len() as u64).to_le_bytes());
    out.extend_from_slice(b);
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let b = self
            .data
            .get(self.pos..self.pos + n)
            .ok_or_else(|| Error::Format("tokenizer.nkt tronqué".into()))?;
        self.pos += n;
        Ok(b)
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().unwrap_or_default(),
        ))
    }
    fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_le_bytes(
            self.take(8)?.try_into().unwrap_or_default(),
        ))
    }
    fn bytes(&mut self) -> Result<&'a [u8]> {
        let n = u64::from_le_bytes(self.take(8)?.try_into().unwrap_or_default());
        self.take(n as usize)
    }
}

/// Base64 standard (avec remplissage), pour la table de normalisation de `tokenizer.json`.
fn base64_decode(s: &str) -> Result<Vec<u8>> {
    let value = |c: u8| -> Result<u32> {
        Ok(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return Err(Error::Format(format!("base64 : caractère {c:#x}"))),
        }
        .into())
    };
    let s = s.trim_end_matches('=').as_bytes();
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    for chunk in s.chunks(4) {
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= value(c)? << (18 - 6 * i);
        }
        let bytes = n.to_be_bytes();
        out.extend_from_slice(&bytes[1..chunk.len()]);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home(rel: &str) -> std::path::PathBuf {
        Path::new(&std::env::var("HOME").unwrap_or_default()).join(rel)
    }

    fn reference(words: &[&str], hf: &tokenizers::Tokenizer) -> Encoding {
        use tokenizers::{EncodeInput, InputSequence};
        let e = hf
            .encode(
                EncodeInput::Single(InputSequence::from(words.to_vec())),
                true,
            )
            .unwrap();
        Encoding {
            ids: e.get_ids().to_vec(),
            word_ids: e.get_word_ids().to_vec(),
        }
    }

    #[test]
    fn base64_round_trip() {
        assert_eq!(base64_decode("TWFu").unwrap(), b"Man");
        assert_eq!(base64_decode("TWE=").unwrap(), b"Ma");
        assert_eq!(base64_decode("TQ==").unwrap(), b"M");
    }

    /// Mêmes sous-mots que `tokenizers` sur des cas limites et sur chaque phrase des corpus UD
    /// (`cargo test --release -p nknown-neural same_as_huggingface -- --ignored`).
    #[test]
    #[ignore = "lit tokenizer.json et les corpus UD du dossier de données"]
    fn same_as_huggingface() {
        let json = home(".local/share/nknown/xlm-roberta-base/tokenizer.json");
        let hf = tokenizers::Tokenizer::from_file(&json).unwrap();
        let ours = Tokenizer::from_json(&std::fs::read(&json).unwrap()).unwrap();
        let tricky: &[&[&str]] = &[
            &["Bonjour", "\u{200b}", "monde"],
            &["a\u{a0}b", "c d", " ", ""],
            &["▁", "▁▁x", "x▁y", "x▁"],
            &["<s>", "a<mask>b", "</s></s>", "<unk>"],
            &["ﬁnale", "Ⅻ", "①", "ｆｕｌｌ", "e\u{301}té", "🇫🇷", "👩‍👩‍👧"],
            &["𝔘𝔫𝔦𝔠𝔬𝔡𝔢", "\u{1}\u{7f}", "ŉ", "ǅ", "ﷺ"],
            &["中华人民共和国", "Москва́", "ёлка", "日本語のテキスト"],
            &["fn", "main()", "{", "x.iter().map(|v|", "v*2)", "}"],
        ];
        for words in tricky {
            assert_eq!(ours.encode_words(words), reference(words, &hf), "{words:?}");
        }
        let ud = home(".local/share/nknown/raw/ud");
        let (mut sentences, mut subwords) = (0, 0);
        for entry in std::fs::read_dir(&ud).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if !name.ends_with(".conllu") || name.starts_with("multi") || name.starts_with("ru_all")
            {
                continue;
            }
            let file = std::io::BufReader::new(std::fs::File::open(&path).unwrap());
            for s in nknown_nlp::conllu::parse(file).unwrap() {
                let words = s.words();
                let expected = reference(&words, &hf);
                assert_eq!(ours.encode_words(&words), expected, "{name} : {words:?}");
                sentences += 1;
                subwords += expected.ids.len();
            }
        }
        eprintln!("{sentences} phrases, {subwords} sous-mots identiques");
        assert!(sentences > 50_000);
    }

    #[test]
    #[ignore = "lit tokenizer.json du dossier de données"]
    fn compact_form_round_trip() {
        let json = home(".local/share/nknown/xlm-roberta-base/tokenizer.json");
        let t = Tokenizer::from_json(&std::fs::read(&json).unwrap()).unwrap();
        let dir = std::env::temp_dir().join(format!("nkt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(CACHE);
        t.write_compact(&path).unwrap();
        let back = Tokenizer::read_compact(&path).unwrap();
        let words = ["L'été", "à", "Paris", "<s>", "ﬁn", "中华"];
        assert_eq!(t.encode_words(&words), back.encode_words(&words));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
