//! Encodeur XLM-RoBERTa (architecture RoBERTa, post-normalisation) en candle.
//!
//! Réécrit ici plutôt que repris de candle-transformers : celui-ci utilise des noyaux
//! fusionnés (softmax, normalisation) sans dérivée, ce qui coupe le gradient et empêche
//! l'affinage. En entraînement, on utilise des opérations dérivables ; en inférence,
//! les noyaux fusionnés, plus rapides. Noms des poids identiques à Hugging Face.

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

#[derive(Debug)]
struct Layer {
    query: Linear,
    key: Linear,
    value: Linear,
    output: Linear,
    attention_norm: Norm,
    intermediate: Linear,
    ffn_output: Linear,
    output_norm: Norm,
}

impl Layer {
    fn new(cfg: &EncoderConfig, vb: VarBuilder) -> Result<Self> {
        let (h, i, eps) = (cfg.hidden_size, cfg.intermediate_size, cfg.layer_norm_eps);
        let att = vb.pp("attention");
        Ok(Self {
            query: linear(h, h, att.pp("self.query"))?,
            key: linear(h, h, att.pp("self.key"))?,
            value: linear(h, h, att.pp("self.value"))?,
            output: linear(h, h, att.pp("output.dense"))?,
            attention_norm: Norm::new(h, eps, att.pp("output.LayerNorm"))?,
            intermediate: linear(h, i, vb.pp("intermediate.dense"))?,
            ffn_output: linear(i, h, vb.pp("output.dense"))?,
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
    word_embeddings: Tensor,
    position_embeddings: Tensor,
    token_type_embedding: Tensor,
    embedding_norm: Norm,
    layers: Vec<Layer>,
    heads: usize,
    pad_id: u32,
}

impl XlmrEncoder {
    pub fn new(cfg: &EncoderConfig, vb: VarBuilder) -> Result<Self> {
        let h = cfg.hidden_size;
        let emb = vb.pp("embeddings");
        Ok(Self {
            word_embeddings: emb.get((cfg.vocab_size, h), "word_embeddings.weight")?,
            position_embeddings: emb.get(
                (cfg.max_position_embeddings, h),
                "position_embeddings.weight",
            )?,
            token_type_embedding: emb.get((1, h), "token_type_embeddings.weight")?,
            embedding_norm: Norm::new(h, cfg.layer_norm_eps, emb.pp("LayerNorm"))?,
            layers: (0..cfg.num_hidden_layers)
                .map(|i| Layer::new(cfg, vb.pp(format!("encoder.layer.{i}"))))
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
        let mask = attention_mask.to_vec2::<u32>()?;
        let positions: Vec<u32> = mask
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
        let h = self.word_embeddings.dim(1)?;
        let x = self
            .word_embeddings
            .index_select(&input_ids.flatten_all()?, 0)?
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
