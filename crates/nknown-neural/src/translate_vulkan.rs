//! Traduction sur GPU (Vulkan, burn) : le même modèle Marian que [`crate::translate`].
//!
//! Les hypothèses du faisceau passent ensemble (un lot de `beams` lignes) : chaque pas de
//! décodage est un seul calcul, quel que soit le nombre d'hypothèses. Les clés et valeurs déjà
//! vues (cache de l'auto-attention) sont réordonnées à chaque pas selon l'hypothèse dont chaque
//! ligne descend ; celles de l'attention vers la source sont calculées une fois par phrase.
//!
//! Noms des modules = noms des poids Hugging Face (`model.encoder.layers.0.self_attn.q_proj`) ;
//! l'adaptateur PyTorch transpose les `Linear` et renomme weight/bias des `LayerNorm`.

use std::path::Path;

use burn::module::{Module, Param};
use burn::nn::{Embedding, EmbeddingConfig, LayerNorm, LayerNormConfig, Linear, LinearConfig};
use burn::store::{ModuleSnapshot, PyTorchToBurnAdapter, SafetensorsStore};
use burn::tensor::activation::{log_softmax, silu, softmax};
use burn::tensor::{Device, FloatDType, Int, Tensor, TensorData};
use candle_transformers::models::marian::Config;

use crate::error::{Error, Result};
use crate::tokenizer::Tokenizer;
use crate::translate::{BEAMS, Detokenizer, beams_or, source_ids, text_side};

#[derive(Module, Debug)]
struct Attn {
    q_proj: Linear,
    k_proj: Linear,
    v_proj: Linear,
    out_proj: Linear,
}

#[derive(Module, Debug)]
struct EncLayer {
    self_attn: Attn,
    self_attn_layer_norm: LayerNorm,
    fc1: Linear,
    fc2: Linear,
    final_layer_norm: LayerNorm,
}

#[derive(Module, Debug)]
struct DecLayer {
    self_attn: Attn,
    self_attn_layer_norm: LayerNorm,
    encoder_attn: Attn,
    encoder_attn_layer_norm: LayerNorm,
    fc1: Linear,
    fc2: Linear,
    final_layer_norm: LayerNorm,
}

#[derive(Module, Debug)]
struct EncStack {
    embed_positions: Embedding,
    layers: Vec<EncLayer>,
}

#[derive(Module, Debug)]
struct DecStack {
    embed_positions: Embedding,
    layers: Vec<DecLayer>,
}

#[derive(Module, Debug)]
struct Inner {
    shared: Embedding,
    encoder: EncStack,
    decoder: DecStack,
}

#[derive(Module, Debug)]
struct Net {
    model: Inner,
    final_logits_bias: Param<Tensor<2>>,
}

fn attn(d: usize, device: &Device) -> Attn {
    let l = || LinearConfig::new(d, d).init(device);
    Attn {
        q_proj: l(),
        k_proj: l(),
        v_proj: l(),
        out_proj: l(),
    }
}

impl Net {
    fn init(c: &Config, device: &Device) -> Self {
        let d = c.d_model;
        let norm = || LayerNormConfig::new(d).init(device);
        let vocab = c.decoder_vocab_size.unwrap_or(c.vocab_size);
        let positions = || EmbeddingConfig::new(c.max_position_embeddings, d).init(device);
        Net {
            model: Inner {
                shared: EmbeddingConfig::new(c.vocab_size, d).init(device),
                encoder: EncStack {
                    embed_positions: positions(),
                    layers: (0..c.encoder_layers)
                        .map(|_| EncLayer {
                            self_attn: attn(d, device),
                            self_attn_layer_norm: norm(),
                            fc1: LinearConfig::new(d, c.encoder_ffn_dim).init(device),
                            fc2: LinearConfig::new(c.encoder_ffn_dim, d).init(device),
                            final_layer_norm: norm(),
                        })
                        .collect(),
                },
                decoder: DecStack {
                    embed_positions: positions(),
                    layers: (0..c.decoder_layers)
                        .map(|_| DecLayer {
                            self_attn: attn(d, device),
                            self_attn_layer_norm: norm(),
                            encoder_attn: attn(d, device),
                            encoder_attn_layer_norm: norm(),
                            fc1: LinearConfig::new(d, c.decoder_ffn_dim).init(device),
                            fc2: LinearConfig::new(c.decoder_ffn_dim, d).init(device),
                            final_layer_norm: norm(),
                        })
                        .collect(),
                },
            },
            final_logits_bias: Param::from_tensor(Tensor::zeros([1, vocab], device)),
        }
    }
}

/// Têtes d'attention : [B, T, H·D] → [B, H, T, D].
fn heads(x: Tensor<3>, h: usize) -> Tensor<4> {
    let [b, t, d] = x.dims();
    x.reshape([b, t, h, d / h]).swap_dims(1, 2)
}

/// [B, H, T, D] → [B, T, H·D].
fn merge(x: Tensor<4>) -> Tensor<3> {
    let [b, h, t, d] = x.dims();
    x.swap_dims(1, 2).reshape([b, t, h * d])
}

fn attend(q: Tensor<4>, k: Tensor<4>, v: Tensor<4>) -> Tensor<4> {
    let d = q.dims()[3];
    let scores = q.matmul(k.swap_dims(2, 3)).div_scalar((d as f64).sqrt());
    softmax(scores, 3).matmul(v)
}

/// Cache d'une couche du décodeur : clés et valeurs de l'auto-attention (croissent d'un pas à
/// l'autre) et de l'attention vers la source (fixes).
struct LayerCache {
    k: Option<Tensor<4>>,
    v: Option<Tensor<4>>,
    cross_k: Tensor<4>,
    cross_v: Tensor<4>,
}

pub struct VulkanTranslator {
    net: Net,
    config: Config,
    source: Tokenizer,
    target: Detokenizer,
    device: Device,
    beams: usize,
    scale: f64,
}

impl std::fmt::Debug for VulkanTranslator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VulkanTranslator").finish_non_exhaustive()
    }
}

impl VulkanTranslator {
    pub fn load(dir: &Path) -> Result<Self> {
        let (config, source, target) = text_side(dir)?;
        let device = crate::vulkan::vulkan_device();
        let mut net = Net::init(&config, &device);
        let mut store = SafetensorsStore::from_file(dir.join("model.safetensors"))
            .with_from_adapter(PyTorchToBurnAdapter)
            // Copies de `shared` (mêmes poids) : non lues.
            .allow_partial(true);
        let result = net
            .load_from(&mut store)
            .map_err(|e| Error::Format(format!("poids : {e}")))?;
        if !result.missing.is_empty() {
            return Err(Error::Format(format!(
                "poids manquants : {:?}",
                &result.missing[..result.missing.len().min(5)]
            )));
        }
        let dtype = crate::vulkan::compute_dtype(&device);
        if dtype != FloatDType::F32 {
            net = net.map(&mut crate::vulkan::CastFloats(dtype));
        }
        let scale = if config.scale_embedding {
            (config.d_model as f64).sqrt()
        } else {
            1.0
        };
        Ok(Self {
            net,
            config,
            source,
            target,
            device,
            beams: beams_or(BEAMS),
            scale,
        })
    }

    pub fn translate(&self, sentences: &[&str]) -> Result<Vec<String>> {
        sentences
            .iter()
            .map(|s| {
                if s.trim().is_empty() {
                    Ok(String::new())
                } else {
                    let ids = source_ids(&self.source, &self.config, s);
                    let out = self.beam_search(&ids)?;
                    Ok(self.target.decode(&out))
                }
            })
            .collect()
    }

    fn ids(&self, v: Vec<i64>, shape: [usize; 2]) -> Tensor<2, Int> {
        Tensor::<2, Int>::from_data(TensorData::new(v, shape), &self.device)
    }

    fn encode(&self, ids: &[u32]) -> Tensor<3> {
        let enc = &self.net.model.encoder;
        let t = ids.len();
        let tok = self.ids(ids.iter().map(|&i| i64::from(i)).collect(), [1, t]);
        let pos = self.ids((0..t as i64).collect(), [1, t]);
        let h = self.config.encoder_attention_heads;
        let mut x = self.net.model.shared.forward(tok).mul_scalar(self.scale)
            + enc.embed_positions.forward(pos);
        for l in &enc.layers {
            let a = &l.self_attn;
            let ctx = attend(
                heads(a.q_proj.forward(x.clone()), h),
                heads(a.k_proj.forward(x.clone()), h),
                heads(a.v_proj.forward(x.clone()), h),
            );
            x = l
                .self_attn_layer_norm
                .forward(x + a.out_proj.forward(merge(ctx)));
            let ff = l.fc2.forward(silu(l.fc1.forward(x.clone())));
            x = l.final_layer_norm.forward(x + ff);
        }
        x
    }

    /// Un pas de décodage pour `tokens` [B] à la position `step` ; renvoie les
    /// log-probabilités [B, V] (f32).
    fn step(&self, tokens: &[u32], step: usize, cache: &mut [LayerCache]) -> Tensor<2> {
        let dec = &self.net.model.decoder;
        let b = tokens.len();
        let h = self.config.decoder_attention_heads;
        let tok = self.ids(tokens.iter().map(|&i| i64::from(i)).collect(), [b, 1]);
        let pos = self.ids(vec![step as i64; b], [b, 1]);
        let mut x = self.net.model.shared.forward(tok).mul_scalar(self.scale)
            + dec.embed_positions.forward(pos);
        for (l, c) in dec.layers.iter().zip(cache.iter_mut()) {
            let a = &l.self_attn;
            let k_new = heads(a.k_proj.forward(x.clone()), h);
            let v_new = heads(a.v_proj.forward(x.clone()), h);
            let k = match c.k.take() {
                Some(k) => Tensor::cat(vec![k, k_new], 2),
                None => k_new,
            };
            let v = match c.v.take() {
                Some(v) => Tensor::cat(vec![v, v_new], 2),
                None => v_new,
            };
            let ctx = attend(heads(a.q_proj.forward(x.clone()), h), k.clone(), v.clone());
            c.k = Some(k);
            c.v = Some(v);
            x = l
                .self_attn_layer_norm
                .forward(x + a.out_proj.forward(merge(ctx)));
            let e = &l.encoder_attn;
            let ctx = attend(
                heads(e.q_proj.forward(x.clone()), h),
                c.cross_k.clone(),
                c.cross_v.clone(),
            );
            x = l
                .encoder_attn_layer_norm
                .forward(x + e.out_proj.forward(merge(ctx)));
            let ff = l.fc2.forward(silu(l.fc1.forward(x.clone())));
            x = l.final_layer_norm.forward(x + ff);
        }
        let d = self.config.d_model;
        let x = x.reshape([b, d]);
        let logits = x.matmul(self.net.model.shared.weight.val().transpose())
            + self.net.final_logits_bias.val();
        log_softmax(logits.cast(FloatDType::F32), 1)
    }

    fn beam_search(&self, ids: &[u32]) -> Result<Vec<u32>> {
        let c = &self.config;
        let beams = self.beams;
        let max_len = (ids.len() * 2 + 10).min(c.max_position_embeddings - 1);
        let encoded = self.encode(ids);
        let h = c.decoder_attention_heads;
        let mut cache: Vec<LayerCache> = self
            .net
            .model
            .decoder
            .layers
            .iter()
            .map(|l| LayerCache {
                k: None,
                v: None,
                cross_k: heads(l.encoder_attn.k_proj.forward(encoded.clone()), h),
                cross_v: heads(l.encoder_attn.v_proj.forward(encoded.clone()), h),
            })
            .collect();
        // Hypothèses vivantes : (sous-mots, score) ; ligne i du lot = hypothèse i.
        let mut live: Vec<(Vec<u32>, f32)> = vec![(vec![c.decoder_start_token_id], 0.0)];
        let mut done: Vec<(Vec<u32>, f32)> = Vec::new();
        for step in 0..max_len {
            let last: Vec<u32> = live
                .iter()
                .map(|(t, _)| *t.last().expect("jamais vide"))
                .collect();
            let logp = self.step(&last, step, &mut cache);
            let (values, indices) = logp.topk_with_indices(beams + 1, 1);
            let values: Vec<f32> = values
                .into_data()
                .try_to_vec_as::<f32>()
                .map_err(|e| Error::Format(format!("{e:?}")))?;
            let indices: Vec<i64> = indices
                .into_data()
                .try_to_vec_as::<i64>()
                .map_err(|e| Error::Format(format!("{e:?}")))?;
            let k = beams + 1;
            let mut cand: Vec<(usize, u32, f32)> = Vec::new();
            for (row, (_, score)) in live.iter().enumerate() {
                for j in 0..k {
                    let t = indices[row * k + j] as u32;
                    if t != c.pad_token_id {
                        cand.push((row, t, score + values[row * k + j]));
                    }
                }
            }
            cand.sort_by(|a, b| b.2.total_cmp(&a.2));
            let mut next: Vec<(Vec<u32>, f32)> = Vec::new();
            let mut parents: Vec<i64> = Vec::new();
            for (row, t, score) in cand {
                if next.len() == beams {
                    break;
                }
                let tokens = &live[row].0;
                if t == c.eos_token_id {
                    done.push((tokens.clone(), score / tokens.len() as f32));
                    continue;
                }
                let mut tokens = tokens.clone();
                tokens.push(t);
                next.push((tokens, score));
                parents.push(row as i64);
            }
            if next.is_empty() || done.len() >= beams {
                break;
            }
            // Le cache suit les hypothèses retenues.
            let n = parents.len();
            let idx = Tensor::<1, Int>::from_data(TensorData::new(parents, [n]), &self.device);
            for lc in &mut cache {
                lc.k = lc.k.take().map(|k| k.select(0, idx.clone()));
                lc.v = lc.v.take().map(|v| v.select(0, idx.clone()));
                let [_, hh, s, dd] = lc.cross_k.dims();
                if lc.cross_k.dims()[0] != n {
                    let first =
                        |t: Tensor<4>| t.slice([0..1, 0..hh, 0..s, 0..dd]).expand([n, hh, s, dd]);
                    lc.cross_k = first(lc.cross_k.clone());
                    lc.cross_v = first(lc.cross_v.clone());
                }
            }
            live = next;
        }
        for (t, s) in live {
            let len = t.len() as f32;
            done.push((t, s / len));
        }
        Ok(done
            .into_iter()
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(t, _)| t.into_iter().skip(1).collect())
            .unwrap_or_default())
    }
}
