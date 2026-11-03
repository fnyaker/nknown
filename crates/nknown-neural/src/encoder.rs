//! Encodeur XLM-RoBERTa (architecture RoBERTa, post-normalisation) en candle.
//!
//! Réécrit ici plutôt que repris de candle-transformers : celui-ci utilise des noyaux
//! fusionnés (softmax, normalisation) sans dérivée, ce qui coupe le gradient et empêche
//! l'affinage. En entraînement, on utilise des opérations dérivables ; en inférence,
//! les noyaux fusionnés, plus rapides. Noms des poids identiques à Hugging Face.
//!
//! En inférence, l'encodeur peut être quantifié ([`Quantized`]) : matrices des couches et
//! table des plongements quantifiées (table en Q4_K, couches en Q8_0 ou Q4_K). Les
//! couches calculent en entiers ; seules les lignes lues de la table sont décodées.

use std::collections::HashMap;
use std::sync::Arc;

use candle_core::quantized::{GgmlDType, QMatMul, QTensor};
use candle_core::{D, DType, Module, Tensor};
use candle_nn::{Linear, VarBuilder, linear, ops};
use serde::Deserialize;

use crate::error::Result;

/// Sous-ensemble utile du `config.json` Hugging Face.
#[derive(Debug, Clone, Deserialize)]
pub struct EncoderConfig {
    pub hidden_size: usize,
    pub num_hidden_layers: usize,
    pub num_attention_heads: usize,
    pub intermediate_size: usize,
    pub layer_norm_eps: f64,
    pub pad_token_id: u32,
    pub vocab_size: usize,
    pub max_position_embeddings: usize,
}

#[derive(Debug)]
struct Norm {
    weight: Tensor,
    bias: Tensor,
    eps: f64,
}

impl Norm {
    fn new(size: usize, eps: f64, vb: VarBuilder) -> Result<Self> {
        Ok(Self {
            weight: vb.get(size, "weight")?,
            bias: vb.get(size, "bias")?,
            eps,
        })
    }

    fn forward(&self, x: &Tensor, train: bool) -> Result<Tensor> {
        if !train {
            return Ok(ops::layer_norm(
                &x.contiguous()?,
                &self.weight,
                &self.bias,
                self.eps as f32,
            )?);
        }
        let mean = x.mean_keepdim(D::Minus1)?;
        let centered = x.broadcast_sub(&mean)?;
        let var = centered.sqr()?.mean_keepdim(D::Minus1)?;
        let normed = centered.broadcast_div(&(var + self.eps)?.sqrt()?)?;
        Ok(normed
            .broadcast_mul(&self.weight)?
            .broadcast_add(&self.bias)?)
    }
}

fn softmax(x: &Tensor, train: bool) -> Result<Tensor> {
    if !train {
        return Ok(ops::softmax_last_dim(&x.contiguous()?)?);
    }
    let max = x.max_keepdim(D::Minus1)?.detach();
    let e = x.broadcast_sub(&max)?.exp()?;
    Ok(e.broadcast_div(&e.sum_keepdim(D::Minus1)?)?)
}

fn dropout(x: Tensor, p: f32, train: bool) -> Result<Tensor> {
    Ok(if train { ops::dropout(&x, p)? } else { x })
}

/// Projection affine, en f32 (entraînement) ou quantifiée en Q8_0 (inférence).
#[derive(Debug)]
enum Proj {
    Full(Linear),
    Q8 {
        weight: QMatMul,
        bias: Option<Tensor>,
    },
}

impl Proj {
    /// Matrice quantifiée si `quant` la contient (même nom que dans `vb`), sinon f32.
    fn new(input: usize, output: usize, vb: VarBuilder, quant: Option<&Quantized>) -> Result<Self> {
        let name = format!("{}.weight", vb.prefix());
        match quant.and_then(|q| q.get(&name)) {
            Some(w) => Ok(Self::Q8 {
                weight: QMatMul::from_arc(w.clone())?,
                bias: Some(vb.get(output, "bias")?),
            }),
            None => Ok(Self::Full(linear(input, output, vb)?)),
        }
    }

    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        Ok(match self {
            Self::Full(l) => l.forward(x)?,
            Self::Q8 { weight, bias } => {
                let y = weight.forward(&x.contiguous()?)?;
                match bias {
                    Some(b) => y.broadcast_add(b)?,
                    None => y,
                }
            }
        })
    }
}

/// Table des plongements de mots (250 002 × 768 pour XLM-R base : 70 % des poids).
#[derive(Debug)]
enum Table {
    Full(Tensor),
    /// Lignes en blocs Q4_K, décodées à la lecture.
    Quant(Arc<QTensor>),
}

/// Q4_K : échelle et minimum f16, 8 couples (échelle, minimum) sur 6 bits, 256 valeurs de 4 bits.
const Q4_BLOCK: usize = 256;
const Q4_BLOCK_BYTES: usize = 2 + 2 + 12 + 128;

/// Échelle et minimum du sous-bloc `j` de Q4_K, sur 6 bits (ggml `get_scale_min_k4`).
fn scale_min_k4(j: usize, q: &[u8]) -> (u8, u8) {
    if j < 4 {
        (q[j] & 63, q[j + 4] & 63)
    } else {
        (
            (q[j + 4] & 0xF) | ((q[j - 4] >> 6) << 4),
            (q[j + 4] >> 4) | ((q[j] >> 6) << 4),
        )
    }
}

/// Même calcul que ggml (`dequantize_row_q4_K`).
fn decode_q4_k(block: &[u8], out: &mut Vec<f32>) {
    let d = half::f16::from_ne_bytes([block[0], block[1]]).to_f32();
    let min = half::f16::from_ne_bytes([block[2], block[3]]).to_f32();
    let (scales, qs) = block[4..].split_at(12);
    for (chunk, q) in qs.chunks_exact(32).enumerate() {
        let (sc, m) = scale_min_k4(2 * chunk, scales);
        let (d1, m1) = (d * f32::from(sc), min * f32::from(m));
        let (sc, m) = scale_min_k4(2 * chunk + 1, scales);
        let (d2, m2) = (d * f32::from(sc), min * f32::from(m));
        out.extend(q.iter().map(|&v| d1 * f32::from(v & 0xF) - m1));
        out.extend(q.iter().map(|&v| d2 * f32::from(v >> 4) - m2));
    }
}

impl Table {
    fn dim(&self) -> Result<usize> {
        Ok(match self {
            Self::Full(t) => t.dim(1)?,
            Self::Quant(q) => q.shape().dims2()?.1,
        })
    }

    fn lookup(&self, ids: &Tensor) -> Result<Tensor> {
        let q = match self {
            Self::Full(t) => return Ok(t.index_select(ids, 0)?),
            Self::Quant(q) => q,
        };
        if q.dtype() != TABLE_DTYPE {
            return Err(candle_core::Error::Msg(format!(
                "table des plongements en {:?} : Q4_K attendu",
                q.dtype()
            ))
            .into());
        }
        let (rows, dim) = q.shape().dims2()?;
        let row_bytes = dim / Q4_BLOCK * Q4_BLOCK_BYTES;
        let data = q.data()?;
        let ids = ids.to_vec1::<u32>()?;
        let mut out = Vec::with_capacity(ids.len() * dim);
        for &id in &ids {
            let row = id as usize;
            if row >= rows {
                return Err(candle_core::Error::Msg(format!("sous-mot hors table : {id}")).into());
            }
            for b in data[row * row_bytes..][..row_bytes].chunks_exact(Q4_BLOCK_BYTES) {
                decode_q4_k(b, &mut out);
            }
        }
        Ok(Tensor::from_vec(
            out,
            (ids.len(), dim),
            &candle_core::Device::Cpu,
        )?)
    }
}

/// Format de la table des plongements : lue ligne à ligne puis normalisée, elle supporte le
/// 4 bits sans perte mesurable (contrairement aux matrices des couches).
pub const TABLE_DTYPE: GgmlDType = GgmlDType::Q4K;

const WORD_EMBEDDINGS: &str = "embeddings.word_embeddings.weight";

/// Poids de l'encodeur gardés quantifiés, par nom Hugging Face.
pub type Quantized = HashMap<String, Arc<QTensor>>;

/// Poids quantifiés en inférence : la table des plongements et les matrices des couches.
/// Biais, normalisations, positions et têtes, petits, restent en f32.
pub fn is_quantizable(name: &str) -> bool {
    name == WORD_EMBEDDINGS
        || (name.starts_with("encoder.layer.")
            && [
                "self.query.weight",
                "self.key.weight",
                "self.value.weight",
                "dense.weight",
            ]
            .iter()
            .any(|s| name.ends_with(s)))
}

/// Sépare des poids f32 en poids gardés en f32 et poids quantifiés (table en [`TABLE_DTYPE`],
/// matrices des couches en `layers`).
/// Chaque tenseur f32 quantifié est libéré aussitôt.
pub fn quantize_weights(
    tensors: HashMap<String, Tensor>,
    layers: GgmlDType,
) -> Result<(HashMap<String, Tensor>, Quantized)> {
    let mut full = HashMap::new();
    let mut quant = Quantized::new();
    for (name, t) in tensors {
        let dtype = if name == WORD_EMBEDDINGS {
            TABLE_DTYPE
        } else {
            layers
        };
        if is_quantizable(&name) && t.rank() == 2 && t.dim(1)? % dtype.block_size() == 0 {
            let q = QTensor::quantize(&t.to_dtype(DType::F32)?, dtype)?;
            quant.insert(name, Arc::new(q));
        } else {
            full.insert(name, t);
        }
    }
    Ok((full, quant))
}

#[derive(Debug)]
struct Layer {
    query: Proj,
    key: Proj,
    value: Proj,
    output: Proj,
    attention_norm: Norm,
    intermediate: Proj,
    ffn_output: Proj,
    output_norm: Norm,
}

impl Layer {
    fn new(cfg: &EncoderConfig, vb: VarBuilder, q: Option<&Quantized>) -> Result<Self> {
        let (h, i, eps) = (cfg.hidden_size, cfg.intermediate_size, cfg.layer_norm_eps);
        let att = vb.pp("attention");
        Ok(Self {
            query: Proj::new(h, h, att.pp("self.query"), q)?,
            key: Proj::new(h, h, att.pp("self.key"), q)?,
            value: Proj::new(h, h, att.pp("self.value"), q)?,
            output: Proj::new(h, h, att.pp("output.dense"), q)?,
            attention_norm: Norm::new(h, eps, att.pp("output.LayerNorm"))?,
            intermediate: Proj::new(h, i, vb.pp("intermediate.dense"), q)?,
            ffn_output: Proj::new(i, h, vb.pp("output.dense"), q)?,
            output_norm: Norm::new(h, eps, vb.pp("output.LayerNorm"))?,
        })
    }

    fn forward(&self, x: &Tensor, mask_bias: &Tensor, heads: usize, train: bool) -> Result<Tensor> {
        let (b, t, h) = x.dims3()?;
        let d = h / heads;
        let split = |y: Tensor| -> Result<Tensor> {
            Ok(y.reshape((b, t, heads, d))?.transpose(1, 2)?.contiguous()?)
        };
        let q = split(self.query.forward(x)?)?;
        let k = split(self.key.forward(x)?)?;
        let v = split(self.value.forward(x)?)?;
        let scores =
            (q.matmul(&k.t()?.contiguous()?)? / (d as f64).sqrt())?.broadcast_add(mask_bias)?;
        let att = dropout(softmax(&scores, train)?, 0.1, train)?;
        let ctx = att.matmul(&v)?.transpose(1, 2)?.reshape((b, t, h))?;
        let attended = dropout(self.output.forward(&ctx)?, 0.1, train)?;
        let h1 = self.attention_norm.forward(&(attended + x)?, train)?;
        let ff = self
            .ffn_output
            .forward(&self.intermediate.forward(&h1)?.gelu_erf()?)?;
        let ff = dropout(ff, 0.1, train)?;
        self.output_norm.forward(&(ff + h1)?, train)
    }
}

#[derive(Debug)]
pub struct XlmrEncoder {
    word_embeddings: Table,
    position_embeddings: Tensor,
    token_type_embedding: Tensor,
    embedding_norm: Norm,
    layers: Vec<Layer>,
    heads: usize,
    pad_id: u32,
}

impl XlmrEncoder {
    pub fn new(cfg: &EncoderConfig, vb: VarBuilder) -> Result<Self> {
        Self::with_quantized(cfg, vb, None)
    }

    /// Comme [`Self::new`], les poids présents dans `quant` étant pris en 8 bits.
    pub fn with_quantized(
        cfg: &EncoderConfig,
        vb: VarBuilder,
        quant: Option<&Quantized>,
    ) -> Result<Self> {
        let h = cfg.hidden_size;
        let emb = vb.pp("embeddings");
        Ok(Self {
            word_embeddings: match quant.and_then(|q| q.get("embeddings.word_embeddings.weight")) {
                Some(q) => Table::Quant(q.clone()),
                None => Table::Full(emb.get((cfg.vocab_size, h), "word_embeddings.weight")?),
            },
            position_embeddings: emb.get(
                (cfg.max_position_embeddings, h),
                "position_embeddings.weight",
            )?,
            token_type_embedding: emb.get((1, h), "token_type_embeddings.weight")?,
            embedding_norm: Norm::new(h, cfg.layer_norm_eps, emb.pp("LayerNorm"))?,
            layers: (0..cfg.num_hidden_layers)
                .map(|i| Layer::new(cfg, vb.pp(format!("encoder.layer.{i}")), quant))
                .collect::<Result<_>>()?,
            heads: cfg.num_attention_heads,
            pad_id: cfg.pad_token_id,
        })
    }

    /// `input_ids`, `attention_mask` : [B, T] (u32, sous-mots alignés à gauche).
    /// Renvoie les états cachés [B, T, H].
    pub fn forward(
        &self,
        input_ids: &Tensor,
        attention_mask: &Tensor,
        train: bool,
    ) -> Result<Tensor> {
        let (b, t) = input_ids.dims2()?;
        let device = input_ids.device();
        // Positions RoBERTa : pad_id + 1 + rang pour les vrais tokens, pad_id pour le remplissage.
        let positions: Vec<u32> = attention_mask
            .to_vec2::<u32>()?
            .iter()
            .flat_map(|row| {
                row.iter().enumerate().map(|(i, &m)| {
                    if m == 1 {
                        self.pad_id + 1 + i as u32
                    } else {
                        self.pad_id
                    }
                })
            })
            .collect();
        let positions = Tensor::from_vec(positions, b * t, device)?;
        let h = self.word_embeddings.dim()?;
        let x = self
            .word_embeddings
            .lookup(&input_ids.flatten_all()?)?
            .add(&self.position_embeddings.index_select(&positions, 0)?)?
            .reshape((b, t, h))?
            .broadcast_add(&self.token_type_embedding)?;
        let mut x = dropout(self.embedding_norm.forward(&x, train)?, 0.1, train)?;

        let mask_bias =
            ((1.0 - attention_mask.to_dtype(DType::F32)?)? * -1e9)?.reshape((b, 1, 1, t))?;
        for layer in &self.layers {
            x = layer.forward(&x, &mask_bias, self.heads, train)?;
        }
        Ok(x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use candle_core::Device;

    #[test]
    fn quantized_table_matches_full_table() {
        let dev = Device::Cpu;
        let t = Tensor::randn(0f32, 1.0, (50, 512), &dev).unwrap();
        let (dtype, tolerance) = (TABLE_DTYPE, 0.5);
        {
            let mut weights = HashMap::new();
            weights.insert("embeddings.word_embeddings.weight".to_owned(), t.clone());
            weights.insert("embeddings.LayerNorm.weight".to_owned(), t.clone());
            let (full, quant) = quantize_weights(weights, GgmlDType::Q8_0).unwrap();
            // Seule la table est quantifiée ; la normalisation reste en f32.
            assert!(full.contains_key("embeddings.LayerNorm.weight"));
            let qt = quant["embeddings.word_embeddings.weight"].clone();
            // Décodage ligne à ligne identique à celui de candle.
            let reference = qt.dequantize(&dev).unwrap();
            let q = Table::Quant(qt);
            let ids = Tensor::new(&[0u32, 7, 49], &dev).unwrap();
            let rows = q.lookup(&ids).unwrap();
            let same: f32 = (reference.index_select(&ids, 0).unwrap() - &rows)
                .unwrap()
                .abs()
                .unwrap()
                .max_all()
                .unwrap()
                .to_scalar()
                .unwrap();
            assert!(
                same < 1e-6,
                "{dtype:?} : décodage différent de candle ({same})"
            );
            let exact = Table::Full(t.clone()).lookup(&ids).unwrap();
            let err: f32 = (exact - rows)
                .unwrap()
                .abs()
                .unwrap()
                .max_all()
                .unwrap()
                .to_scalar()
                .unwrap();
            assert!(err < tolerance, "{dtype:?} : erreur {err}");
            assert!(q.lookup(&Tensor::new(&[50u32], &dev).unwrap()).is_err());
        }
    }

    #[test]
    fn quantizable_names() {
        assert!(is_quantizable(
            "encoder.layer.3.attention.self.query.weight"
        ));
        assert!(is_quantizable("encoder.layer.3.intermediate.dense.weight"));
        assert!(!is_quantizable("encoder.layer.3.attention.self.query.bias"));
        assert!(!is_quantizable("encoder.layer.3.output.LayerNorm.weight"));
        assert!(!is_quantizable("heads.arc_dep.weight"));
    }
}
