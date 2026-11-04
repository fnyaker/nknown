//! Traduction locale : modèles Marian (OPUS-MT, Helsinki-NLP), une paire de langues par modèle.
//!
//! Un modèle est un dossier : `config.json`, `model.safetensors` (poids en f16, voir
//! [`convert`]), `source.spm` (tokeniseur SentencePiece de la langue source) et `vocab.json`
//! (sous-mots communs aux deux langues). La traduction se fait phrase par phrase, en recherche
//! en faisceau ([`BEAMS`] hypothèses), sur CPU (candle) ; le GPU passe par `vulkan` (burn).

use std::collections::HashMap;
use std::path::Path;

use candle_core::{D, DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::marian::{Config, MTModel};

use crate::error::{Error, Result};
use crate::tokenizer::Tokenizer;

/// Hypothèses gardées par la recherche en faisceau : 4 sur GPU (comme
/// `generation_config.json`, les hypothèses passent ensemble), 2 sur CPU (chacune y coûte un
/// passage ; 2 donnent déjà la traduction de 4 sur nos essais, pour moitié moins de temps).
pub const BEAMS: usize = 4;
pub const CPU_BEAMS: usize = 2;
/// Sous-mots d'une phrase source, au plus (au-delà, la phrase est coupée).
pub(crate) const MAX_SOURCE: usize = 256;

/// Fichiers d'un modèle de traduction prêt à l'emploi.
pub const FILES: [&str; 4] = [
    "config.json",
    "model.safetensors",
    "source.spm",
    "vocab.json",
];

/// Vocabulaire de sortie : identifiant → sous-mot.
pub(crate) struct Detokenizer {
    pieces: Vec<String>,
    skip: Vec<u32>,
}

impl Detokenizer {
    pub(crate) fn decode(&self, ids: &[u32]) -> String {
        let mut s = String::new();
        for &id in ids {
            if self.skip.contains(&id) {
                continue;
            }
            if let Some(p) = self.pieces.get(id as usize) {
                s.push_str(p);
            }
        }
        s.replace('▁', " ").trim().to_owned()
    }
}

pub struct Translator {
    model: MTModel,
    config: Config,
    source: Tokenizer,
    target: Detokenizer,
    device: Device,
    beams: usize,
}

impl std::fmt::Debug for Translator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Translator").finish_non_exhaustive()
    }
}

fn read_vocab(dir: &Path) -> Result<HashMap<String, u32>> {
    Ok(serde_json::from_slice(&std::fs::read(
        dir.join("vocab.json"),
    )?)?)
}

/// Ce qui ne dépend pas du moteur de calcul : configuration, tokeniseur source, vocabulaire
/// de sortie.
pub(crate) fn text_side(dir: &Path) -> Result<(Config, Tokenizer, Detokenizer)> {
    let config: Config = serde_json::from_slice(&std::fs::read(dir.join("config.json"))?)?;
    let vocab = read_vocab(dir)?;
    let source = Tokenizer::from_sentencepiece(&std::fs::read(dir.join("source.spm"))?, &vocab)?;
    let mut pieces = vec![String::new(); vocab.values().copied().max().unwrap_or(0) as usize + 1];
    for (p, &id) in &vocab {
        pieces[id as usize] = p.clone();
    }
    let skip: Vec<u32> = ["<pad>", "</s>", "<unk>"]
        .iter()
        .filter_map(|t| vocab.get(*t).copied())
        .chain([config.pad_token_id, config.eos_token_id])
        .collect();
    Ok((config, source, Detokenizer { pieces, skip }))
}

/// Sous-mots source d'une phrase, coupés à [`MAX_SOURCE`].
pub(crate) fn source_ids(source: &Tokenizer, config: &Config, sentence: &str) -> Vec<u32> {
    let mut ids = source.encode_sentence(sentence);
    if ids.len() > MAX_SOURCE {
        ids.truncate(MAX_SOURCE - 1);
        ids.push(config.eos_token_id);
    }
    ids
}

/// Hypothèses du faisceau : celles de `NKNOWN_BEAMS`, sinon `default`.
pub(crate) fn beams_or(default: usize) -> usize {
    std::env::var("NKNOWN_BEAMS")
        .ok()
        .and_then(|b| b.parse().ok())
        .filter(|&b| b >= 1)
        .unwrap_or(default)
}

impl Translator {
    /// Charge le modèle du dossier `dir` sur CPU.
    pub fn load(dir: &Path) -> Result<Self> {
        let (config, source, target) = text_side(dir)?;
        let device = Device::Cpu;
        let weights = std::fs::read(dir.join("model.safetensors"))?;
        let vb = VarBuilder::from_buffered_safetensors(weights, DType::F32, &device)?;
        let model = MTModel::new(&config, vb)?;
        Ok(Self {
            model,
            config,
            source,
            target,
            device,
            beams: beams_or(CPU_BEAMS),
        })
    }

    /// Traduit chaque phrase de `sentences` (une phrase vide reste vide).
    pub fn translate(&self, sentences: &[&str]) -> Result<Vec<String>> {
        sentences
            .iter()
            .map(|s| {
                if s.trim().is_empty() {
                    Ok(String::new())
                } else {
                    self.translate_one(s)
                }
            })
            .collect()
    }

    fn translate_one(&self, sentence: &str) -> Result<String> {
        let ids = source_ids(&self.source, &self.config, sentence);
        let max_len = (ids.len() * 2 + 10).min(self.config.max_position_embeddings - 1);
        let mut model = self.model.clone();
        model.reset_kv_cache();
        let input = Tensor::new(ids.as_slice(), &self.device)?.unsqueeze(0)?;
        let encoded = model.encoder().forward(&input, 0)?;
        let out = beam_search(
            &model,
            &encoded,
            &self.config,
            &self.device,
            self.beams,
            max_len,
        )?;
        Ok(self.target.decode(&out))
    }
}

/// Recherche en faisceau : `beams` hypothèses, chacune avec son propre cache de décodage
/// (copie du modèle), score = log-probabilité moyenne (normalisée par la longueur).
fn beam_search(
    model: &MTModel,
    encoded: &Tensor,
    config: &Config,
    device: &Device,
    beams: usize,
    max_len: usize,
) -> Result<Vec<u32>> {
    struct Hyp {
        model: MTModel,
        tokens: Vec<u32>,
        score: f32,
    }
    let start = config.decoder_start_token_id;
    let eos = config.eos_token_id;
    let pad = config.pad_token_id;
    let mut live = vec![Hyp {
        model: model.clone(),
        tokens: vec![start],
        score: 0.0,
    }];
    let mut done: Vec<(Vec<u32>, f32)> = Vec::new();
    for step in 0..max_len {
        let mut candidates: Vec<(usize, u32, f32)> = Vec::new();
        for (h, hyp) in live.iter_mut().enumerate() {
            let last = *hyp.tokens.last().expect("jamais vide");
            let x = Tensor::new(&[last], device)?.unsqueeze(0)?;
            let logits = hyp.model.decode(&x, encoded, step)?.squeeze(0)?.get(0)?;
            let logp = candle_nn::ops::log_softmax(&logits, D::Minus1)?.to_vec1::<f32>()?;
            // Les `beams` meilleurs prolongements de cette hypothèse.
            let mut idx: Vec<u32> = (0..logp.len() as u32).collect();
            let k = beams.min(idx.len());
            idx.select_nth_unstable_by(k, |&a, &b| logp[b as usize].total_cmp(&logp[a as usize]));
            for &t in &idx[..k] {
                if t == pad {
                    continue;
                }
                candidates.push((h, t, hyp.score + logp[t as usize]));
            }
        }
        candidates.sort_by(|a, b| b.2.total_cmp(&a.2));
        let mut next: Vec<Hyp> = Vec::new();
        for (h, t, score) in candidates {
            if next.len() + done.len() >= beams * 2 || next.len() == beams {
                break;
            }
            let base = &live[h];
            let mut tokens = base.tokens.clone();
            if t == eos {
                let len = tokens.len() as f32;
                done.push((tokens, score / len));
                continue;
            }
            tokens.push(t);
            next.push(Hyp {
                model: base.model.clone(),
                tokens,
                score,
            });
        }
        // Le cache de chaque hypothèse doit suivre son propre préfixe : chaque copie a déjà
        // vu le préfixe de son parent ; le nouveau sous-mot sera vu au pas suivant.
        live = next;
        if live.is_empty() || done.len() >= beams {
            break;
        }
    }
    for h in live {
        let len = h.tokens.len() as f32;
        done.push((h.tokens, h.score / len));
    }
    let best = done
        .into_iter()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(t, _)| t)
        .unwrap_or_default();
    Ok(best.into_iter().skip(1).collect())
}

/// Prépare un modèle Marian de Hugging Face (dossier avec `model.safetensors` ou
/// `pytorch_model.bin`) : poids en
/// safetensors f16 (moitié de la taille), et les fichiers utiles copiés dans `out`.
pub fn convert(hf_dir: &Path, out: &Path) -> Result<()> {
    std::fs::create_dir_all(out)?;
    // `model.safetensors` (conversion de Hugging Face) de préférence ; sinon les poids
    // PyTorch, au format zip (l'ancien format n'est pas lu).
    let safetensors = hf_dir.join("model.safetensors");
    let tensors: Vec<(String, Tensor)> = if safetensors.exists() {
        candle_core::safetensors::load(&safetensors, &Device::Cpu)?
            .into_iter()
            .collect()
    } else {
        candle_core::pickle::read_all(hf_dir.join("pytorch_model.bin"))?
    };
    let mut map: HashMap<String, Tensor> = HashMap::new();
    for (name, t) in tensors {
        let t = if t.dtype() == DType::F32 {
            t.to_dtype(DType::F16)?
        } else {
            t
        };
        map.insert(name, t);
    }
    candle_core::safetensors::save(&map, out.join("model.safetensors"))?;
    for f in ["config.json", "source.spm", "vocab.json"] {
        std::fs::copy(hf_dir.join(f), out.join(f))?;
    }
    if !out.join("model.safetensors").exists() {
        return Err(Error::Format("conversion sans résultat".into()));
    }
    Ok(())
}
