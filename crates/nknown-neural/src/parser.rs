//! Analyse avec un modèle entraîné (CPU, candle).

use std::path::Path;

use candle_core::{D, DType, Device, Tensor};
use candle_nn::{VarBuilder, ops};
use nknown_core::lang::Lang;
use nknown_nlp::{ParsedSentence, SentenceParser};
use tokenizers::Tokenizer;

use crate::config::ParserConfig;
use crate::data::{self, Batch, Encoded};
use crate::error::Result;
use crate::model::{ParserModel, Scores};

pub struct NeuralParser {
    model: ParserModel,
    tokenizer: Tokenizer,
    cfg: ParserConfig,
    device: Device,
}

impl std::fmt::Debug for NeuralParser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NeuralParser")
            .field("deprels", &self.cfg.deprels.len())
            .field("langs", &self.cfg.langs)
            .finish()
    }
}

impl NeuralParser {
    /// Charge un modèle depuis son répertoire (`parser.json`, `model.safetensors`, `tokenizer.json`).
    pub fn load(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref();
        let cfg = ParserConfig::load(dir.join("parser.json"))?;
        let device = Device::Cpu;
        let tensors = candle_core::safetensors::load(dir.join("model.safetensors"), &device)?;
        let vb = VarBuilder::from_tensors(tensors, DType::F32, &device);
        let model = ParserModel::new(&cfg, vb.clone(), vb.pp("heads"))?;
        let tokenizer = Tokenizer::from_file(dir.join("tokenizer.json"))?;
        Ok(Self {
            model,
            tokenizer,
            cfg,
            device,
        })
    }

    /// Langues d'entraînement du modèle.
    pub fn langs(&self) -> Vec<Lang> {
        self.cfg
            .langs
            .iter()
            .filter_map(|l| Lang::from_code(l))
            .collect()
    }

    pub fn parse_words(&self, sentences: &[Vec<&str>]) -> Result<Vec<Option<ParsedSentence>>> {
        parse_with(
            &self.model,
            &self.tokenizer,
            &self.cfg,
            &self.device,
            sentences,
            32,
        )
    }
}

impl SentenceParser for NeuralParser {
    fn supports(&self, lang: Lang) -> bool {
        self.cfg.langs.is_empty() || self.cfg.langs.iter().any(|l| l == lang.as_str())
    }

    fn parse(&self, _lang: Lang, sentences: &[Vec<&str>]) -> Vec<Option<ParsedSentence>> {
        self.parse_words(sentences)
            .unwrap_or_else(|_| vec![None; sentences.len()])
    }
}

/// Analyse par lots, phrases triées par longueur pour limiter le remplissage.
pub(crate) fn parse_with(
    model: &ParserModel,
    tokenizer: &Tokenizer,
    cfg: &ParserConfig,
    device: &Device,
    sentences: &[Vec<&str>],
    batch_size: usize,
) -> Result<Vec<Option<ParsedSentence>>> {
    let mut encoded: Vec<(usize, Encoded)> = Vec::new();
    for (i, words) in sentences.iter().enumerate() {
        if words.is_empty() {
            continue;
        }
        if let Some(e) = data::encode(tokenizer, words, cfg.max_subwords)? {
            encoded.push((i, e));
        }
    }
    encoded.sort_by_key(|(_, e)| e.ids.len());
    let mut out = vec![None; sentences.len()];
    for chunk in encoded.chunks(batch_size) {
        let refs: Vec<&Encoded> = chunk.iter().map(|(_, e)| e).collect();
        let batch = data::batch(&refs, device)?;
        let scores = model.forward(&batch, false)?;
        let parsed = decode(model, &batch, &scores, &cfg.deprels)?;
        for ((i, _), p) in chunk.iter().zip(parsed) {
            out[*i] = Some(p);
        }
    }
    Ok(out)
}

/// Sorties du réseau → analyses : catégories et traits par argmax, arbre par Chu-Liu-Edmonds,
/// étiquettes pour les gouverneurs de l'arbre.
pub(crate) fn decode(
    model: &ParserModel,
    batch: &Batch,
    s: &Scores,
    deprels: &[String],
) -> Result<Vec<ParsedSentence>> {
    let upos = s.upos.argmax(D::Minus1)?.to_vec2::<u32>()?;
    let gender = s.gender.argmax(D::Minus1)?.to_vec2::<u32>()?;
    let number = s.number.argmax(D::Minus1)?.to_vec2::<u32>()?;
    let arcs = ops::log_softmax(&s.arcs, D::Minus1)?.to_vec3::<f32>()?;
    let trees = data::decode_trees(&arcs, &batch.lengths);

    let width = batch.width;
    let mut head_ids = vec![0u32; batch.lengths.len() * width];
    for (b, tree) in trees.iter().enumerate() {
        for (d, &h) in tree.iter().enumerate().skip(1) {
            head_ids[b * width + d] = h as u32;
        }
    }
    let heads = Tensor::from_vec(head_ids, (batch.lengths.len(), width), s.arcs.device())?;
    let labels = model
        .labels(s, &heads)?
        .argmax(D::Minus1)?
        .to_vec2::<u32>()?;
    Ok(data::assemble(
        &batch.lengths,
        &upos,
        &gender,
        &number,
        &trees,
        &labels,
        deprels,
    ))
}
