//! Le réseau : encodeur XLM-RoBERTa et têtes de prédiction (candle).
//!
//! Arcs : score(d, h) = dep_d · W · head_h + head_h · b (attention biaffine).
//! Étiquettes : pour le gouverneur h choisi, score_l(d, h) = dep_d · U_l · head_h + V_l · [dep_d ; head_h].

use std::collections::HashMap;
use std::path::Path;

use candle_core::{DType, Device, Module, Shape, Tensor, Var};
use candle_nn::var_builder::SimpleBackend;
use candle_nn::{Init, Linear, VarBuilder, VarMap, linear, ops};

use crate::config::ParserConfig;
use crate::data::{Batch, GENDERS, N_UPOS, NUMBERS};
use crate::encoder::XlmrEncoder;
use crate::error::Result;

/// Préfixe des poids gelés pendant l'entraînement : la table d'embeddings
/// (192 M de paramètres sur 278 M), dont le gradient coûterait cher pour peu de gain.
pub const FROZEN_PREFIX: &str = "embeddings.";

#[derive(Debug)]
pub struct Heads {
    upos: Linear,
    gender: Linear,
    number: Linear,
    arc_dep: Linear,
    arc_head: Linear,
    arc_w: Tensor,
    arc_b: Tensor,
    lab_dep: Linear,
    lab_head: Linear,
    lab_u: Tensor,
    lab_lin: Linear,
}

impl Heads {
    pub fn new(cfg: &ParserConfig, vb: VarBuilder) -> Result<Self> {
        let h = cfg.hidden_size();
        let (a, l, nl) = (cfg.arc_dim, cfg.label_dim, cfg.deprels.len());
        Ok(Self {
            upos: linear(h, N_UPOS, vb.pp("upos"))?,
            gender: linear(h, GENDERS.len(), vb.pp("gender"))?,
            number: linear(h, NUMBERS.len(), vb.pp("number"))?,
            arc_dep: linear(h, a, vb.pp("arc_dep"))?,
            arc_head: linear(h, a, vb.pp("arc_head"))?,
            arc_w: vb.get_with_hints((a, a), "arc_w", Init::Const(0.0))?,
            arc_b: vb.get_with_hints((a, 1), "arc_b", Init::Const(0.0))?,
            lab_dep: linear(h, l, vb.pp("lab_dep"))?,
            lab_head: linear(h, l, vb.pp("lab_head"))?,
            lab_u: vb.get_with_hints((l, nl * l), "lab_u", Init::Const(0.0))?,
            lab_lin: linear(2 * l, nl, vb.pp("lab_lin"))?,
        })
    }
}

/// Sorties du réseau pour un lot. N = 1 + nombre de mots (position 0 = racine).
#[derive(Debug)]
pub struct Scores {
    /// [B, N, 17]
    pub upos: Tensor,
    /// [B, N, 4]
    pub gender: Tensor,
    /// [B, N, 3]
    pub number: Tensor,
    /// [B, N (dépendant), N (gouverneur)]
    pub arcs: Tensor,
    lab_dep: Tensor,
    lab_head: Tensor,
}

pub struct ParserModel {
    encoder: XlmrEncoder,
    heads: Heads,
    n_labels: usize,
}

impl std::fmt::Debug for ParserModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParserModel")
            .field("n_labels", &self.n_labels)
            .finish_non_exhaustive()
    }
}

impl ParserModel {
    pub fn new(cfg: &ParserConfig, encoder: VarBuilder, heads: VarBuilder) -> Result<Self> {
        Ok(Self {
            encoder: XlmrEncoder::new(&cfg.encoder_config()?, encoder)?,
            heads: Heads::new(cfg, heads)?,
            n_labels: cfg.deprels.len(),
        })
    }

    pub fn forward(&self, batch: &Batch, train: bool) -> Result<Scores> {
        let hidden = self
            .encoder
            .forward(&batch.input_ids, &batch.attention_mask, train)?;
        let (b, _, h) = hidden.dims3()?;
        let idx = batch
            .first
            .unsqueeze(2)?
            .broadcast_as((b, batch.width, h))?
            .contiguous()?;
        let mut words = hidden.gather(&idx, 1)?;
        if train {
            words = ops::dropout(&words, 0.2)?;
        }
        let mlp = |layer: &Linear| -> Result<Tensor> {
            let y = layer.forward(&words)?.gelu_erf()?;
            Ok(if train { ops::dropout(&y, 0.33)? } else { y })
        };
        let (arc_dep, arc_head) = (mlp(&self.heads.arc_dep)?, mlp(&self.heads.arc_head)?);
        let arcs = arc_dep
            .broadcast_matmul(&self.heads.arc_w)?
            .matmul(&arc_head.transpose(1, 2)?.contiguous()?)?;
        let bias = arc_head
            .broadcast_matmul(&self.heads.arc_b)?
            .transpose(1, 2)?;
        Ok(Scores {
            upos: self.heads.upos.forward(&words)?,
            gender: self.heads.gender.forward(&words)?,
            number: self.heads.number.forward(&words)?,
            arcs: arcs.broadcast_add(&bias)?,
            lab_dep: mlp(&self.heads.lab_dep)?,
            lab_head: mlp(&self.heads.lab_head)?,
        })
    }

    /// Scores d'étiquettes [B, N, L] pour les gouverneurs `heads` ([B, N], u32).
    pub fn labels(&self, s: &Scores, heads: &Tensor) -> Result<Tensor> {
        let (b, n, l) = s.lab_dep.dims3()?;
        let idx = heads.unsqueeze(2)?.broadcast_as((b, n, l))?.contiguous()?;
        let head = s.lab_head.gather(&idx, 1)?;
        let bilinear = s
            .lab_dep
            .broadcast_matmul(&self.heads.lab_u)?
            .reshape((b, n, self.n_labels, l))?
            .broadcast_mul(&head.unsqueeze(2)?)?
            .sum(3)?;
        let linear = self
            .heads
            .lab_lin
            .forward(&Tensor::cat(&[&s.lab_dep, &head], 2)?)?;
        Ok((bilinear + linear)?)
    }
}

/// Poids pré-entraînés de l'encodeur, renommés sans le préfixe `roberta.` et sans les têtes
/// de pré-entraînement (`lm_head`, `pooler`).
pub fn load_pretrained(path: impl AsRef<Path>, device: &Device) -> Result<HashMap<String, Tensor>> {
    Ok(candle_core::safetensors::load(path, device)?
        .into_iter()
        .filter_map(|(k, v)| {
            let k = k.strip_prefix("roberta.").unwrap_or(&k).to_owned();
            (!k.starts_with("lm_head") && !k.starts_with("pooler")).then_some((k, v))
        })
        .collect())
}

/// Fournit les poids de l'encodeur au constructeur : variables entraînables (enregistrées
/// dans `trainable`), sauf celles sous [`FROZEN_PREFIX`], rendues comme constantes.
pub struct EncoderWeights {
    pub tensors: HashMap<String, Tensor>,
    pub trainable: VarMap,
}

impl std::fmt::Debug for EncoderWeights {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EncoderWeights")
            .field("tensors", &self.tensors.len())
            .finish_non_exhaustive()
    }
}

impl EncoderWeights {
    fn fetch(&self, name: &str, dtype: DType, dev: &Device) -> candle_core::Result<Tensor> {
        let t = self
            .tensors
            .get(name)
            .ok_or_else(|| candle_core::Error::Msg(format!("poids absent : {name}")))?
            .to_dtype(dtype)?
            .to_device(dev)?;
        if name.starts_with(FROZEN_PREFIX) {
            return Ok(t);
        }
        let var = Var::from_tensor(&t)?;
        self.trainable
            .data()
            .lock()
            .map_err(|_| candle_core::Error::Msg("verrou empoisonné".into()))?
            .insert(name.to_owned(), var.clone());
        Ok(var.as_tensor().clone())
    }
}

impl SimpleBackend for EncoderWeights {
    fn get(
        &self,
        s: Shape,
        name: &str,
        _h: Init,
        dtype: DType,
        dev: &Device,
    ) -> candle_core::Result<Tensor> {
        let t = self.fetch(name, dtype, dev)?;
        if t.shape() != &s {
            return Err(candle_core::Error::Msg(format!(
                "forme inattendue pour {name} : {:?} au lieu de {s:?}",
                t.shape()
            )));
        }
        Ok(t)
    }

    fn get_unchecked(&self, name: &str, dtype: DType, dev: &Device) -> candle_core::Result<Tensor> {
        self.fetch(name, dtype, dev)
    }

    fn contains_tensor(&self, name: &str) -> bool {
        self.tensors.contains_key(name)
    }
}
