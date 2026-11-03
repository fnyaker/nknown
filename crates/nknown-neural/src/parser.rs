//! Analyse avec un modèle entraîné (CPU, candle).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use candle_core::quantized::{GgmlDType, QTensor, gguf_file};
use candle_core::{D, DType, Device, Tensor};
use candle_nn::{VarBuilder, ops};
use nknown_core::lang::Lang;
use nknown_nlp::{ParsedSentence, SentenceParser};
use tokenizers::Tokenizer;

use crate::config::ParserConfig;
use crate::data::{self, Batch, Encoded};
use crate::encoder::{self, Quantized};
use crate::error::Result;
use crate::model::{ParserModel, Scores};

const WEIGHTS: &str = "model.safetensors";
/// Poids quantifiés : lus depuis le fichier de cache de la précision (`model.q8.gguf`…) s'il
/// est à jour, sinon quantifiés depuis le modèle f32 puis enregistrés pour les fois suivantes
/// (le fichier f32 n'est alors plus lu : bien moins de mémoire au chargement comme à l'usage).
fn quantized_weights(
    dir: &Path,
    device: &Device,
    layers: GgmlDType,
    cache: &str,
) -> Result<(HashMap<String, Tensor>, Quantized)> {
    let (full, cache) = (dir.join(WEIGHTS), dir.join(cache));
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let fresh = match (modified(&cache), modified(&full)) {
        (Some(c), Some(f)) => c >= f,
        (Some(_), None) => true,
        _ => false,
    };
    if fresh {
        return read_gguf(&cache, device);
    }
    let (tensors, quant) =
        encoder::quantize_weights(candle_core::safetensors::load(&full, device)?, layers)?;
    if let Err(e) = write_gguf(&cache, &tensors, &quant) {
        // Simple cache : sans lui, on requantifiera au prochain chargement.
        eprintln!("{} : {e}", cache.display());
    }
    Ok((tensors, quant))
}

fn read_gguf(path: &Path, device: &Device) -> Result<(HashMap<String, Tensor>, Quantized)> {
    let mut file = std::io::BufReader::new(std::fs::File::open(path)?);
    let content = gguf_file::Content::read(&mut file)?;
    let (mut tensors, mut quant) = (HashMap::new(), Quantized::new());
    for name in content.tensor_infos.keys() {
        let q = content.tensor(&mut file, name, device)?;
        if q.dtype() == GgmlDType::F32 {
            tensors.insert(name.clone(), q.dequantize(device)?);
        } else {
            quant.insert(name.clone(), Arc::new(q));
        }
    }
    Ok((tensors, quant))
}

/// Écriture atomique : un fichier interrompu ne passe jamais pour le cache.
fn write_gguf(path: &Path, tensors: &HashMap<String, Tensor>, quant: &Quantized) -> Result<()> {
    let mut all: Vec<(String, Arc<QTensor>)> =
        quant.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    for (name, t) in tensors {
        let t = t.to_dtype(DType::F32)?;
        all.push((
            name.clone(),
            Arc::new(QTensor::quantize(&t, GgmlDType::F32)?),
        ));
    }
    all.sort_by(|a, b| a.0.cmp(&b.0));
    let refs: Vec<(&str, &QTensor)> = all.iter().map(|(k, v)| (k.as_str(), v.as_ref())).collect();
    let tmp = path.with_extension("gguf.tmp");
    {
        let mut out = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        gguf_file::write(&mut out, &[], &refs)?;
        std::io::Write::flush(&mut out)?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Précision des poids de l'encodeur en inférence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Precision {
    /// Poids d'origine (f32) : ~1,1 Go pour XLM-R base. Pour les mesures.
    F32,
    /// Table des plongements en 4 bits (Q4_K), couches en 8 bits (Q8_0) : la précision du
    /// f32 pour ~5× moins de poids (211 Mo).
    #[default]
    Standard,
    /// Tout l'encodeur en 4 bits (Q4_K) : 169 Mo, LAS −0,4 à −1,5 point.
    Light,
}

impl Precision {
    /// Format des matrices des couches et fichier de cache d'une précision quantifiée.
    fn quantized(self) -> Option<(GgmlDType, &'static str)> {
        match self {
            Self::F32 => None,
            Self::Standard => Some((GgmlDType::Q8_0, "model.q4k-q8.gguf")),
            Self::Light => Some((GgmlDType::Q4K, "model.q4k.gguf")),
        }
    }
}

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
        Self::load_with(dir, Precision::F32)
    }

    /// Charge un modèle, en pleine précision ou quantifié.
    pub fn load_with(dir: impl AsRef<Path>, precision: Precision) -> Result<Self> {
        let dir = dir.as_ref();
        let cfg = ParserConfig::load(dir.join("parser.json"))?;
        let device = Device::Cpu;
        let model = match precision.quantized() {
            None => {
                let tensors = candle_core::safetensors::load(dir.join(WEIGHTS), &device)?;
                let vb = VarBuilder::from_tensors(tensors, DType::F32, &device);
                ParserModel::new(&cfg, vb.clone(), vb.pp("heads"))?
            }
            Some((layers, cache)) => {
                let (tensors, quant) = quantized_weights(dir, &device, layers, cache)?;
                let vb = VarBuilder::from_tensors(tensors, DType::F32, &device);
                ParserModel::with_quantized(&cfg, vb.clone(), vb.pp("heads"), &quant)?
            }
        };
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
