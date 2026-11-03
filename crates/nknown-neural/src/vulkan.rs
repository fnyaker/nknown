//! Moteur Vulkan (burn + wgpu) : le même réseau que la version candle, sur GPU.
//!
//! Les poids sont lus et écrits dans le même format que la version CPU (safetensors,
//! noms Hugging Face, disposition PyTorch) : un modèle entraîné sur l'un s'exécute sur l'autre.
//! Correspondance des noms : `self` → `self_` (mot réservé), `LayerNorm` → `layer_norm`,
//! et l'adaptateur PyTorch ↔ burn transpose les `Linear` et renomme weight/bias → gamma/beta.

use std::path::Path;
use std::time::Instant;

use crate::tokenizer::Tokenizer;
use burn::module::{Module, Param};
use burn::nn::loss::CrossEntropyLossConfig;
use burn::nn::{
    Dropout, DropoutConfig, Embedding, EmbeddingConfig, LayerNorm, LayerNormConfig, Linear,
    LinearConfig,
};
use burn::optim::{AdamWConfig, GradientsParams};
use burn::store::{BurnToPyTorchAdapter, ModuleSnapshot, PyTorchToBurnAdapter, SafetensorsStore};
use burn::tensor::activation::{gelu, log_softmax, softmax};
use burn::tensor::{Device, Int, Tensor, TensorData};
use nknown_core::lang::Lang;
use nknown_nlp::conllu::GoldSentence;
use nknown_nlp::{ParsedSentence, SentenceParser};

use crate::config::ParserConfig;
use crate::data::{self, Encoded, Example, GENDERS, HostBatch, IGNORE, N_UPOS, NUMBERS};
use crate::encoder::EncoderConfig;
use crate::error::{Error, Result};
use crate::train::{DevScores, TrainOptions};

// ---------------------------------------------------------------- réseau

#[derive(Module, Debug)]
struct SelfAttention {
    query: Linear,
    key: Linear,
    value: Linear,
}

#[derive(Module, Debug)]
struct DenseNorm {
    dense: Linear,
    layer_norm: LayerNorm,
}

#[derive(Module, Debug)]
struct Attention {
    self_: SelfAttention,
    output: DenseNorm,
}

#[derive(Module, Debug)]
struct Intermediate {
    dense: Linear,
}

#[derive(Module, Debug)]
struct Layer {
    attention: Attention,
    intermediate: Intermediate,
    output: DenseNorm,
}

#[derive(Module, Debug)]
struct Encoder {
    layer: Vec<Layer>,
}

#[derive(Module, Debug)]
struct Embeddings {
    word_embeddings: Embedding,
    position_embeddings: Embedding,
    token_type_embeddings: Embedding,
    layer_norm: LayerNorm,
}

#[derive(Module, Debug)]
struct Heads {
    upos: Linear,
    gender: Linear,
    number: Linear,
    arc_dep: Linear,
    arc_head: Linear,
    arc_w: Param<Tensor<2>>,
    arc_b: Param<Tensor<2>>,
    lab_dep: Linear,
    lab_head: Linear,
    lab_u: Param<Tensor<2>>,
    lab_lin: Linear,
}

#[derive(Module, Debug)]
pub struct Network {
    embeddings: Embeddings,
    encoder: Encoder,
    heads: Heads,
    dropout: Dropout,
    head_dropout: Dropout,
}

fn dense_norm(i: usize, o: usize, eps: f64, device: &Device) -> DenseNorm {
    DenseNorm {
        dense: LinearConfig::new(i, o).init(device),
        layer_norm: LayerNormConfig::new(o).with_epsilon(eps).init(device),
    }
}

impl Network {
    fn init(enc: &EncoderConfig, cfg: &ParserConfig, device: &Device) -> Self {
        Self::init_with_vocab(enc, cfg, enc.vocab_size, device)
    }

    /// `vocab` à 1 : réseau d'inférence dont la table des mots reste sur le CPU.
    fn init_with_vocab(
        enc: &EncoderConfig,
        cfg: &ParserConfig,
        vocab: usize,
        device: &Device,
    ) -> Self {
        let (h, eps) = (enc.hidden_size, enc.layer_norm_eps);
        let (a, l, nl) = (cfg.arc_dim, cfg.label_dim, cfg.deprels.len());
        let zeros = |shape: [usize; 2]| Param::from_tensor(Tensor::<2>::zeros(shape, device));
        Network {
            embeddings: Embeddings {
                word_embeddings: EmbeddingConfig::new(vocab, h).init(device),
                position_embeddings: EmbeddingConfig::new(enc.max_position_embeddings, h)
                    .init(device),
                token_type_embeddings: EmbeddingConfig::new(1, h).init(device),
                layer_norm: LayerNormConfig::new(h).with_epsilon(eps).init(device),
            },
            encoder: Encoder {
                layer: (0..enc.num_hidden_layers)
                    .map(|_| Layer {
                        attention: Attention {
                            self_: SelfAttention {
                                query: LinearConfig::new(h, h).init(device),
                                key: LinearConfig::new(h, h).init(device),
                                value: LinearConfig::new(h, h).init(device),
                            },
                            output: dense_norm(h, h, eps, device),
                        },
                        intermediate: Intermediate {
                            dense: LinearConfig::new(h, enc.intermediate_size).init(device),
                        },
                        output: dense_norm(enc.intermediate_size, h, eps, device),
                    })
                    .collect(),
            },
            heads: Heads {
                upos: LinearConfig::new(h, N_UPOS).init(device),
                gender: LinearConfig::new(h, GENDERS.len()).init(device),
                number: LinearConfig::new(h, NUMBERS.len()).init(device),
                arc_dep: LinearConfig::new(h, a).init(device),
                arc_head: LinearConfig::new(h, a).init(device),
                arc_w: zeros([a, a]),
                arc_b: zeros([a, 1]),
                lab_dep: LinearConfig::new(h, l).init(device),
                lab_head: LinearConfig::new(h, l).init(device),
                lab_u: zeros([l, nl * l]),
                lab_lin: LinearConfig::new(2 * l, nl).init(device),
            },
            dropout: DropoutConfig::new(0.1).init(),
            head_dropout: DropoutConfig::new(0.33).init(),
        }
    }
}

/// Sorties pour un lot (mêmes conventions que la version candle).
struct Scores {
    upos: Tensor<3>,
    gender: Tensor<3>,
    number: Tensor<3>,
    arcs: Tensor<3>,
    lab_dep: Tensor<3>,
    lab_head: Tensor<3>,
}

struct Inputs {
    ids: Tensor<2, Int>,
    /// Vecteurs des sous-mots, lus dans la table gardée sur le CPU (inférence) ; sinon la
    /// table du réseau (entraînement).
    words: Option<Tensor<3>>,
    positions: Tensor<2, Int>,
    mask: Tensor<2>,
    first: Tensor<2, Int>,
    lengths: Vec<usize>,
    width: usize,
}

fn inputs(
    encoded: &[&Encoded],
    pad_id: u32,
    table: Option<&crate::encoder::Table>,
    device: &Device,
) -> Result<Inputs> {
    let h = HostBatch::new(encoded);
    let (b, t) = (h.batch_size, h.seq_len);
    let int = |v: Vec<u32>, shape: [usize; 2]| {
        let v: Vec<i64> = v.into_iter().map(i64::from).collect();
        Tensor::<2, Int>::from_data(TensorData::new(v, shape), device)
    };
    let mask: Vec<f32> = h.mask.iter().map(|&m| m as f32).collect();
    let words = match table {
        Some(table) => {
            let ids = candle_core::Tensor::from_slice(&h.ids, b * t, &candle_core::Device::Cpu)?;
            let dim = table.dim()?;
            let rows: Vec<f32> = table.lookup(&ids)?.flatten_all()?.to_vec1()?;
            Some(Tensor::<3>::from_data(
                TensorData::new(rows, [b, t, dim]),
                device,
            ))
        }
        None => None,
    };
    Ok(Inputs {
        words,
        positions: int(h.positions(pad_id), [b, t]),
        ids: int(h.ids, [b, t]),
        mask: Tensor::<2>::from_data(TensorData::new(mask, [b, t]), device),
        first: int(h.first, [b, h.width]),
        lengths: h.lengths,
        width: h.width,
    })
}

impl Layer {
    fn forward(&self, x: Tensor<3>, bias: Tensor<4>, heads: usize, drop: &Dropout) -> Tensor<3> {
        let [b, t, h] = x.dims();
        let d = h / heads;
        let split = |y: Tensor<3>| y.reshape([b, t, heads, d]).swap_dims(1, 2);
        let q = split(self.attention.self_.query.forward(x.clone()));
        let k = split(self.attention.self_.key.forward(x.clone()));
        let v = split(self.attention.self_.value.forward(x.clone()));
        let scores = q.matmul(k.swap_dims(2, 3)).div_scalar((d as f64).sqrt()) + bias;
        let att = drop.forward(softmax(scores, 3));
        let ctx = att.matmul(v).swap_dims(1, 2).reshape([b, t, h]);
        let out = &self.attention.output;
        let h1 = out
            .layer_norm
            .forward(drop.forward(out.dense.forward(ctx)) + x);
        let ff = self
            .output
            .dense
            .forward(gelu(self.intermediate.dense.forward(h1.clone())));
        self.output.layer_norm.forward(drop.forward(ff) + h1)
    }
}

impl Network {
    fn forward(&self, x: &Inputs, heads: usize) -> Scores {
        let e = &self.embeddings;
        let [b, t] = x.ids.dims();
        let types = Tensor::<2, Int>::zeros([b, t], &x.ids.device());
        let words = match &x.words {
            Some(w) => w.clone(),
            None => e.word_embeddings.forward(x.ids.clone()),
        };
        let emb = words
            + e.position_embeddings.forward(x.positions.clone())
            + e.token_type_embeddings.forward(types);
        let mut hidden = self.dropout.forward(e.layer_norm.forward(emb));
        let bias = (x.mask.ones_like() - x.mask.clone())
            .mul_scalar(-1e9)
            .reshape([b, 1, 1, t]);
        for layer in &self.encoder.layer {
            hidden = layer.forward(hidden, bias.clone(), heads, &self.dropout);
        }
        let h = hidden.dims()[2];
        let idx = x
            .first
            .clone()
            .reshape([b, x.width, 1])
            .expand([b, x.width, h]);
        let words = self.dropout.forward(hidden.gather(1, idx));

        let hd = &self.heads;
        let mlp = |l: &Linear| self.head_dropout.forward(gelu(l.forward(words.clone())));
        let (arc_dep, arc_head) = (mlp(&hd.arc_dep), mlp(&hd.arc_head));
        let arcs = arc_dep
            .matmul(hd.arc_w.val().unsqueeze::<3>())
            .matmul(arc_head.clone().swap_dims(1, 2));
        let bias = arc_head
            .matmul(hd.arc_b.val().unsqueeze::<3>())
            .swap_dims(1, 2);
        Scores {
            upos: hd.upos.forward(words.clone()),
            gender: hd.gender.forward(words.clone()),
            number: hd.number.forward(words.clone()),
            arcs: arcs + bias,
            lab_dep: mlp(&hd.lab_dep),
            lab_head: mlp(&hd.lab_head),
        }
    }

    /// Scores d'étiquettes [B, N, L] pour les gouverneurs `heads` [B, N].
    fn labels(&self, s: &Scores, heads: Tensor<2, Int>) -> Tensor<3> {
        let [b, n, l] = s.lab_dep.dims();
        let nl = self.heads.lab_lin.weight.dims()[1];
        let idx = heads.reshape([b, n, 1]).expand([b, n, l]);
        let head = s.lab_head.clone().gather(1, idx);
        let bilinear = s
            .lab_dep
            .clone()
            .matmul(self.heads.lab_u.val().unsqueeze::<3>())
            .reshape([b, n, nl, l])
            .mul(head.clone().unsqueeze_dim::<4>(2))
            .sum_dim(3)
            .reshape([b, n, nl]);
        let linear = self
            .heads
            .lab_lin
            .forward(Tensor::cat(vec![s.lab_dep.clone(), head], 2));
        bilinear + linear
    }
}

// ---------------------------------------------------------------- poids

fn load_store(path: &Path, partial: bool) -> SafetensorsStore {
    adapt(SafetensorsStore::from_file(path), partial)
}

fn adapt(store: SafetensorsStore, partial: bool) -> SafetensorsStore {
    store
        .with_from_adapter(PyTorchToBurnAdapter)
        .with_key_remapping(r"^roberta\.", "")
        .with_key_remapping(r"\.self\.", ".self_.")
        .with_key_remapping(r"LayerNorm", "layer_norm")
        .allow_partial(partial)
}

fn save_store(path: &Path) -> SafetensorsStore {
    // Écrase le modèle de la passe précédente quand une passe fait mieux.
    SafetensorsStore::from_file(path)
        .overwrite(true)
        .with_to_adapter(BurnToPyTorchAdapter)
        .with_key_remapping(r"\.self_\.", ".self.")
        .with_key_remapping(r"layer_norm", "LayerNorm")
}

fn store_err(e: impl std::fmt::Display) -> Error {
    Error::Format(format!("poids : {e}"))
}

/// Poids d'inférence et table des mots. Depuis les poids quantifiés distribués
/// (`model.q4k-q8.gguf`, ou `model.q4k.gguf`), sinon depuis le modèle f32 (entraînement, dépôt) :
/// la table reste sur le CPU en 4 bits ; le reste, décompressé en f32, est sérialisé en mémoire
/// pour le GPU (couches, positions, têtes : ~345 Mo au lieu de 1,1 Go).
fn inference_weights(dir: &Path) -> Result<(SafetensorsStore, crate::encoder::Table)> {
    use crate::encoder::{TABLE_DTYPE, Table, WORD_EMBEDDINGS};
    let cpu = candle_core::Device::Cpu;
    let gguf = ["model.q4k-q8.gguf", "model.q4k.gguf"]
        .iter()
        .map(|f| dir.join(f))
        .find(|p| p.is_file());
    let (mut tensors, mut quant) = match gguf {
        Some(path) => crate::parser::read_gguf(&path, &cpu)?,
        None => (
            candle_core::safetensors::load(dir.join("model.safetensors"), &cpu)?,
            crate::encoder::Quantized::new(),
        ),
    };
    let table = match (
        quant.remove(WORD_EMBEDDINGS),
        tensors.remove(WORD_EMBEDDINGS),
    ) {
        (Some(q), _) if q.dtype() == TABLE_DTYPE => Table::Quant(q),
        (Some(q), _) => Table::Quant(std::sync::Arc::new(
            candle_core::quantized::QTensor::quantize(&q.dequantize(&cpu)?, TABLE_DTYPE)?,
        )),
        (None, Some(t)) => Table::Quant(std::sync::Arc::new(
            candle_core::quantized::QTensor::quantize(&t, TABLE_DTYPE)?,
        )),
        (None, None) => return Err(Error::Format("table des mots absente".into())),
    };
    for (name, q) in quant {
        tensors.insert(name, q.dequantize(&cpu)?);
    }
    let bytes = safetensors::serialize(tensors.iter().map(|(k, v)| (k.as_str(), v)), None)
        .map_err(store_err)?;
    drop(tensors);
    // Partiel : la table des mots n'est pas chargée dans le réseau.
    Ok((
        adapt(SafetensorsStore::from_bytes(Some(bytes)), true),
        table,
    ))
}

// ---------------------------------------------------------------- inférence

/// Analyseur exécuté sur GPU via Vulkan.
pub struct VulkanParser {
    net: Network,
    /// Table des mots (70 % des poids), gardée sur le CPU en 4 bits : le GPU ne reçoit que
    /// les vecteurs des sous-mots de chaque lot.
    table: crate::encoder::Table,
    tokenizer: Tokenizer,
    cfg: ParserConfig,
    enc: EncoderConfig,
    device: Device,
}

impl std::fmt::Debug for VulkanParser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VulkanParser")
            .field("langs", &self.cfg.langs)
            .finish_non_exhaustive()
    }
}

/// Un GPU Vulkan répond : un petit calcul y aboutit. Sans pilote ni périphérique, wgpu panique
/// à la création du périphérique : la panique est rattrapée.
pub fn available() -> bool {
    std::panic::catch_unwind(|| {
        let device = vulkan_device();
        let t = Tensor::<1>::from_floats([1.0f32, 2.0], &device);
        t.sum()
            .into_data()
            .try_to_vec::<f32>()
            .is_ok_and(|v| v == [3.0])
    })
    .unwrap_or(false)
}

pub fn vulkan_device() -> Device {
    burn::backend::wgpu::WgpuDevice::default().into()
}

impl VulkanParser {
    pub fn load(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref();
        let cfg = ParserConfig::load(dir.join("parser.json"))?;
        let enc = cfg.encoder_config()?;
        let device = vulkan_device();
        let (mut store, table) = inference_weights(dir)?;
        let mut net = Network::init_with_vocab(&enc, &cfg, 1, &device);
        net.load_from(&mut store).map_err(store_err)?;
        Ok(Self {
            net,
            table,
            tokenizer: Tokenizer::from_file(dir.join("tokenizer.json"))?,
            cfg,
            enc,
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
            &self.net,
            Some(&self.table),
            &self.tokenizer,
            &self.cfg,
            &self.enc,
            &self.device,
            sentences,
        )
    }
}

impl SentenceParser for VulkanParser {
    fn supports(&self, lang: Lang) -> bool {
        self.cfg.langs.is_empty() || self.cfg.langs.iter().any(|l| l == lang.as_str())
    }

    fn parse(&self, _lang: Lang, sentences: &[Vec<&str>]) -> Vec<Option<ParsedSentence>> {
        self.parse_words(sentences)
            .unwrap_or_else(|_| vec![None; sentences.len()])
    }
}

fn to_vec3(t: Tensor<3>) -> Vec<Vec<Vec<f32>>> {
    let [b, n, m] = t.dims();
    let flat: Vec<f32> = t.into_data().try_to_vec_as::<f32>().unwrap_or_default();
    (0..b)
        .map(|i| {
            (0..n)
                .map(|j| flat[(i * n + j) * m..(i * n + j + 1) * m].to_vec())
                .collect()
        })
        .collect()
}

fn argmax2(t: Tensor<3>) -> Vec<Vec<u32>> {
    let [b, n, _] = t.dims();
    let flat: Vec<i64> = t
        .argmax(2)
        .into_data()
        .try_to_vec_as::<i64>()
        .unwrap_or_default();
    (0..b)
        .map(|i| flat[i * n..(i + 1) * n].iter().map(|&v| v as u32).collect())
        .collect()
}

fn parse_with(
    net: &Network,
    table: Option<&crate::encoder::Table>,
    tokenizer: &Tokenizer,
    cfg: &ParserConfig,
    enc: &EncoderConfig,
    device: &Device,
    sentences: &[Vec<&str>],
) -> Result<Vec<Option<ParsedSentence>>> {
    let mut encoded: Vec<(usize, Encoded)> = Vec::new();
    for (i, words) in sentences.iter().enumerate() {
        if !words.is_empty()
            && let Some(e) = data::encode(tokenizer, words, cfg.max_subwords)?
        {
            encoded.push((i, e));
        }
    }
    encoded.sort_by_key(|(_, e)| e.ids.len());
    let mut out = vec![None; sentences.len()];
    for chunk in encoded.chunks(32) {
        let refs: Vec<&Encoded> = chunk.iter().map(|(_, e)| e).collect();
        let x = inputs(&refs, enc.pad_token_id, table, device)?;
        let s = net.forward(&x, enc.num_attention_heads);
        let trees = data::decode_trees(&to_vec3(log_softmax(s.arcs.clone(), 2)), &x.lengths);
        let mut head_ids = vec![0i64; x.lengths.len() * x.width];
        for (b, tree) in trees.iter().enumerate() {
            for (d, &h) in tree.iter().enumerate().skip(1) {
                head_ids[b * x.width + d] = h as i64;
            }
        }
        let heads = Tensor::<2, Int>::from_data(
            TensorData::new(head_ids, [x.lengths.len(), x.width]),
            device,
        );
        let labels = argmax2(net.labels(&s, heads));
        let parsed = data::assemble(
            &x.lengths,
            &argmax2(s.upos),
            &argmax2(s.gender),
            &argmax2(s.number),
            &trees,
            &labels,
            &cfg.deprels,
        );
        for ((i, _), p) in chunk.iter().zip(parsed) {
            out[*i] = Some(p);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- entraînement

fn loss(net: &Network, x: &Inputs, exs: &[&Example], s: &Scores, device: &Device) -> Tensor<1> {
    let (b, n) = (exs.len(), x.width);
    let (mut rows, mut upos, mut gender, mut number) = (vec![], vec![], vec![], vec![]);
    let (mut arc_rows, mut arc_targets, mut lab_rows, mut lab_targets) =
        (vec![], vec![], vec![], vec![]);
    let mut gold_heads = vec![0i64; b * n];
    let mut col_mask = vec![0f32; b * n];
    for (bi, e) in exs.iter().enumerate() {
        let t = &e.targets;
        let len = t.upos.len();
        for c in len + 1..n {
            col_mask[bi * n + c] = -1e9;
        }
        for k in 0..len {
            let row = (bi * n + k + 1) as i64;
            rows.push(row);
            upos.push(i64::from(t.upos[k]));
            gender.push(i64::from(t.gender[k]));
            number.push(i64::from(t.number[k]));
            if t.heads[k] != IGNORE {
                arc_rows.push(row);
                arc_targets.push(t.heads[k]);
                gold_heads[row as usize] = t.heads[k];
                if t.labels[k] != IGNORE {
                    lab_rows.push(row);
                    lab_targets.push(t.labels[k]);
                }
            }
        }
    }
    let ints = |v: Vec<i64>| {
        let len = v.len();
        Tensor::<1, Int>::from_data(TensorData::new(v, [len]), device)
    };
    let ce = CrossEntropyLossConfig::new().init(device);
    let pick = |logits: Tensor<3>, rows: &[i64], targets: Vec<i64>| {
        let classes = logits.dims()[2];
        let flat = logits
            .reshape([b * n, classes])
            .select(0, ints(rows.to_vec()));
        ce.forward(flat, ints(targets))
    };
    let mask = Tensor::<3>::from_data(TensorData::new(col_mask, [b, 1, n]), device);
    let labels = net.labels(
        s,
        Tensor::<2, Int>::from_data(TensorData::new(gold_heads, [b, n]), device),
    );
    pick(s.upos.clone(), &rows, upos)
        + pick(s.gender.clone(), &rows, gender).mul_scalar(0.5)
        + pick(s.number.clone(), &rows, number).mul_scalar(0.5)
        + pick(s.arcs.clone() + mask, &arc_rows, arc_targets)
        + pick(labels, &lab_rows, lab_targets)
}

/// Entraînement sur GPU (Vulkan). Mêmes options, même format de modèle que [`crate::train`].
pub fn train(
    xlmr_dir: &Path,
    train_set: &[GoldSentence],
    dev_set: &[GoldSentence],
    out_dir: &Path,
    opts: &TrainOptions,
    mut log: impl FnMut(&str),
) -> Result<DevScores> {
    let device = vulkan_device().autodiff();
    let tokenizer = Tokenizer::from_file(xlmr_dir.join("tokenizer.json"))?;
    let cfg = ParserConfig {
        encoder: serde_json::from_slice(&std::fs::read(xlmr_dir.join("config.json"))?)?,
        deprels: data::deprel_vocab(train_set),
        arc_dim: 512,
        label_dim: 128,
        max_subwords: opts.max_subwords,
        langs: opts.langs.clone(),
    };
    let enc = cfg.encoder_config()?;
    let (examples, skipped) =
        data::examples(&tokenizer, train_set, &cfg.deprels, opts.max_subwords)?;
    log(&format!(
        "[vulkan] {} phrases d'entraînement ({skipped} écartées), {} étiquettes",
        examples.len(),
        cfg.deprels.len()
    ));

    let mut net = Network::init(&enc, &cfg, &device);
    let applied = net
        .load_from(&mut load_store(&xlmr_dir.join("model.safetensors"), true))
        .map_err(store_err)?;
    log(&format!("[vulkan] poids pré-entraînés : {applied}"));
    // Table d'embeddings gelée, comme sur CPU.
    net.embeddings = net.embeddings.no_grad();

    let mut opt_encoder = AdamWConfig::new().with_weight_decay(0.01).init();
    let mut opt_heads = AdamWConfig::new()
        .with_beta_2(0.9)
        .with_weight_decay(0.0)
        .init();

    let mut order: Vec<usize> = (0..examples.len()).collect();
    order.sort_by_key(|&i| examples[i].encoded.ids.len());
    let mut batches: Vec<Vec<usize>> = order
        .chunks(opts.batch_size)
        .map(<[usize]>::to_vec)
        .collect();
    let per_epoch = batches.len();
    let total = opts.max_steps.unwrap_or(per_epoch * opts.epochs).max(1);
    let warmup = ((total as f64 * opts.warmup) as usize).max(1);
    let mut rng = 0x9e37_79b9_7f4a_7c15u64;

    let start = Instant::now();
    let mut step = 0usize;
    let mut best: Option<DevScores> = None;
    for epoch in 1..=opts.epochs {
        for i in (1..batches.len()).rev() {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            batches.swap(i, (rng % (i as u64 + 1)) as usize);
        }
        let mut running = 0.0;
        for (k, idx) in batches.iter().enumerate() {
            step += 1;
            let factor = if step <= warmup {
                step as f64 / warmup as f64
            } else {
                total.saturating_sub(step) as f64 / (total - warmup).max(1) as f64
            };
            let exs: Vec<&Example> = idx.iter().map(|&i| &examples[i]).collect();
            let refs: Vec<&Encoded> = exs.iter().map(|e| &e.encoded).collect();
            let x = inputs(&refs, enc.pad_token_id, None, &device)?;
            let s = net.forward(&x, enc.num_attention_heads);
            let l = loss(&net, &x, &exs, &s, &device);
            running += f64::from(l.clone().into_scalar::<f32>());
            let mut grads = l.backward();
            let g_enc = GradientsParams::from_module(&mut grads, &net.encoder);
            let g_heads = GradientsParams::from_module(&mut grads, &net.heads);
            net = opt_encoder.step(opts.encoder_lr * factor, net, g_enc);
            net = opt_heads.step(opts.head_lr * factor, net, g_heads);

            if step % 20 == 0 || k + 1 == per_epoch || opts.max_steps.is_some() {
                let per_step = start.elapsed().as_secs_f64() / step as f64;
                log(&format!(
                    "[vulkan] passe {epoch} pas {}/{per_epoch} : perte {:.3}, {:.2} s/pas, reste ~{:.0} min",
                    k + 1,
                    running / (k + 1) as f64,
                    per_step,
                    per_step * total.saturating_sub(step) as f64 / 60.0
                ));
            }
            if opts.max_steps.is_some_and(|m| step >= m) {
                break;
            }
        }

        let eval_net = net.valid();
        let dev = evaluate(&eval_net, &tokenizer, &cfg, &enc, &vulkan_device(), dev_set)?;
        log(&format!(
            "[vulkan] passe {epoch} : dev UPOS {:.2} %, UAS {:.2} %, LAS {:.2} %",
            dev.upos, dev.uas, dev.las
        ));
        if best.is_none_or(|b| dev.las > b.las) {
            std::fs::create_dir_all(out_dir)?;
            eval_net
                .save_into(&mut save_store(&out_dir.join("model.safetensors")))
                .map_err(store_err)?;
            cfg.save(out_dir.join("parser.json"))?;
            std::fs::copy(
                xlmr_dir.join("tokenizer.json"),
                out_dir.join("tokenizer.json"),
            )?;
            log(&format!(
                "[vulkan] modèle sauvegardé dans {}",
                out_dir.display()
            ));
            best = Some(dev);
        }
        if opts.max_steps.is_some_and(|m| step >= m) {
            break;
        }
    }
    Ok(best.unwrap_or_default())
}

fn evaluate(
    net: &Network,
    tokenizer: &Tokenizer,
    cfg: &ParserConfig,
    enc: &EncoderConfig,
    device: &Device,
    gold: &[GoldSentence],
) -> Result<DevScores> {
    let inputs: Vec<Vec<&str>> = gold.iter().map(|s| s.words()).collect();
    let parsed = parse_with(net, None, tokenizer, cfg, enc, device, &inputs)?;
    Ok(crate::train::score(gold, &parsed))
}
