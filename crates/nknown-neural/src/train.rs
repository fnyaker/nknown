//! Entraînement (affinage de XLM-RoBERTa + têtes) sur un corpus UD, sur CPU avec candle.
//!
//! Deux groupes de paramètres : l'encodeur (taux d'apprentissage faible) et les têtes
//! (taux plus élevé), montée linéaire puis décroissance linéaire. La table d'embeddings
//! reste gelée. Le meilleur modèle sur le corpus de développement (LAS) est sauvegardé.

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use candle_core::{DType, Device, Tensor};
use candle_nn::loss::cross_entropy;
use candle_nn::{AdamW, Optimizer, ParamsAdamW, VarBuilder, VarMap};
use nknown_nlp::conllu::GoldSentence;
use tokenizers::Tokenizer;

use crate::config::ParserConfig;
use crate::data::{self, Example, GENDERS, IGNORE, N_UPOS, NUMBERS};
use crate::error::Result;
use crate::model::{self, EncoderWeights, FROZEN_PREFIX, ParserModel, Scores};
use crate::parser;

#[derive(Debug, Clone)]
pub struct TrainOptions {
    pub epochs: usize,
    pub batch_size: usize,
    pub encoder_lr: f64,
    pub head_lr: f64,
    /// Part des pas consacrée à la montée du taux d'apprentissage.
    pub warmup: f64,
    pub max_subwords: usize,
    /// Arrêt après ce nombre de pas (essais, mesure de vitesse), suivi de l'évaluation.
    pub max_steps: Option<usize>,
    pub langs: Vec<String>,
}

impl Default for TrainOptions {
    fn default() -> Self {
        Self {
            epochs: 3,
            batch_size: 16,
            encoder_lr: 3e-5,
            head_lr: 1e-3,
            warmup: 0.1,
            max_subwords: 256,
            max_steps: None,
            langs: vec!["fra".into()],
        }
    }
}

/// Scores sur le corpus de développement.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DevScores {
    pub upos: f64,
    pub uas: f64,
    pub las: f64,
}

pub fn train(
    xlmr_dir: &Path,
    train_set: &[GoldSentence],
    dev_set: &[GoldSentence],
    out_dir: &Path,
    opts: &TrainOptions,
    mut log: impl FnMut(&str),
) -> Result<DevScores> {
    let device = Device::Cpu;
    let tokenizer = Tokenizer::from_file(xlmr_dir.join("tokenizer.json"))?;
    let cfg = ParserConfig {
        encoder: serde_json::from_slice(&std::fs::read(xlmr_dir.join("config.json"))?)?,
        deprels: data::deprel_vocab(train_set),
        arc_dim: 512,
        label_dim: 128,
        max_subwords: opts.max_subwords,
        langs: opts.langs.clone(),
    };
    let (examples, skipped) =
        data::examples(&tokenizer, train_set, &cfg.deprels, opts.max_subwords)?;
    log(&format!(
        "{} phrases d'entraînement ({} trop longues écartées), {} étiquettes",
        examples.len(),
        skipped,
        cfg.deprels.len()
    ));

    let pretrained = model::load_pretrained(xlmr_dir.join("model.safetensors"), &device)?;
    let encoder_vars = VarMap::new();
    let encoder_vb = VarBuilder::from_backend(
        Box::new(EncoderWeights {
            tensors: pretrained.clone(),
            trainable: encoder_vars.clone(),
        }),
        DType::F32,
        device.clone(),
    );
    let head_vars = VarMap::new();
    let head_vb = VarBuilder::from_varmap(&head_vars, DType::F32, &device).pp("heads");
    let model = ParserModel::new(&cfg, encoder_vb, head_vb)?;
    log(&format!(
        "paramètres entraînables : encodeur {} M, têtes {:.1} M",
        count(&encoder_vars) / 1_000_000,
        count(&head_vars) as f64 / 1e6
    ));

    let mut opt_encoder = AdamW::new(
        encoder_vars.all_vars(),
        ParamsAdamW {
            lr: 0.0,
            weight_decay: 0.01,
            ..Default::default()
        },
    )?;
    let mut opt_heads = AdamW::new(
        head_vars.all_vars(),
        ParamsAdamW {
            lr: 0.0,
            beta2: 0.9,
            weight_decay: 0.0,
            ..Default::default()
        },
    )?;

    // Lots de phrases de longueurs voisines (moins de remplissage), dans un ordre mélangé.
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
        shuffle(&mut batches, &mut rng);
        let mut running = 0.0;
        for (k, idx) in batches.iter().enumerate() {
            step += 1;
            let factor = if step <= warmup {
                step as f64 / warmup as f64
            } else {
                (total.saturating_sub(step)) as f64 / (total - warmup).max(1) as f64
            };
            opt_encoder.set_learning_rate(opts.encoder_lr * factor);
            opt_heads.set_learning_rate(opts.head_lr * factor);

            let exs: Vec<&Example> = idx.iter().map(|&i| &examples[i]).collect();
            let encoded: Vec<&data::Encoded> = exs.iter().map(|e| &e.encoded).collect();
            let batch = data::batch(&encoded, &device)?;
            let scores = model.forward(&batch, true)?;
            let loss = loss(&model, &batch, &exs, &scores, &device)?;
            let grads = loss.backward()?;
            if step == 1 {
                let with_grad = |vars: &VarMap| {
                    let all = vars.all_vars();
                    let n = all
                        .iter()
                        .filter(|v| grads.get(v.as_tensor()).is_some())
                        .count();
                    format!("{n}/{}", all.len())
                };
                log(&format!(
                    "variables avec gradient : encodeur {}, têtes {}",
                    with_grad(&encoder_vars),
                    with_grad(&head_vars)
                ));
            }
            opt_encoder.step(&grads)?;
            opt_heads.step(&grads)?;
            running += loss.to_scalar::<f32>()? as f64;

            if step % 20 == 0 || k + 1 == per_epoch || opts.max_steps.is_some() {
                let per_step = start.elapsed().as_secs_f64() / step as f64;
                log(&format!(
                    "passe {epoch} pas {}/{per_epoch} : perte {:.3}, {:.2} s/pas, reste ~{:.0} min",
                    k + 1,
                    running / (k + 1) as f64,
                    per_step,
                    per_step * (total - step) as f64 / 60.0
                ));
            }
            if opts.max_steps.is_some_and(|m| step >= m) {
                break;
            }
        }

        let dev = evaluate(&model, &tokenizer, &cfg, &device, dev_set)?;
        log(&format!(
            "passe {epoch} : dev UPOS {:.2} %, UAS {:.2} %, LAS {:.2} %",
            dev.upos, dev.uas, dev.las
        ));
        if best.is_none_or(|b| dev.las > b.las) {
            save(
                out_dir,
                &cfg,
                &pretrained,
                &encoder_vars,
                &head_vars,
                xlmr_dir,
            )?;
            log(&format!("modèle sauvegardé dans {}", out_dir.display()));
            best = Some(dev);
        }
        if opts.max_steps.is_some_and(|m| step >= m) {
            break;
        }
    }
    Ok(best.unwrap_or_default())
}

fn count(vars: &VarMap) -> usize {
    vars.all_vars().iter().map(|v| v.elem_count()).sum()
}

fn shuffle<T>(v: &mut [T], state: &mut u64) {
    for i in (1..v.len()).rev() {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        v.swap(i, (*state % (i as u64 + 1)) as usize);
    }
}

/// Somme des pertes : catégorie, genre et nombre (pondérés 0,5), arcs, étiquettes.
fn loss(
    model: &ParserModel,
    batch: &data::Batch,
    exs: &[&Example],
    s: &Scores,
    device: &Device,
) -> Result<Tensor> {
    let (b, n) = (exs.len(), batch.width);
    let (mut rows, mut upos, mut gender, mut number) = (vec![], vec![], vec![], vec![]);
    let (mut arc_rows, mut arc_targets, mut lab_rows, mut lab_targets) =
        (vec![], vec![], vec![], vec![]);
    let mut gold_heads = vec![0u32; b * n];
    let mut col_mask = vec![0f32; b * n];
    for (bi, e) in exs.iter().enumerate() {
        let t = &e.targets;
        let len = t.upos.len();
        for c in len + 1..n {
            col_mask[bi * n + c] = -1e9;
        }
        for k in 0..len {
            let row = (bi * n + k + 1) as u32;
            rows.push(row);
            upos.push(t.upos[k]);
            gender.push(t.gender[k]);
            number.push(t.number[k]);
            if t.heads[k] != IGNORE {
                arc_rows.push(row);
                arc_targets.push(t.heads[k] as u32);
                gold_heads[row as usize] = t.heads[k] as u32;
                if t.labels[k] != IGNORE {
                    lab_rows.push(row);
                    lab_targets.push(t.labels[k] as u32);
                }
            }
        }
    }
    let ce = |logits: &Tensor, classes: usize, rows: &[u32], targets: Vec<u32>| -> Result<Tensor> {
        let flat = logits.reshape((b * n, classes))?;
        let picked = flat.index_select(&Tensor::new(rows, device)?, 0)?;
        Ok(cross_entropy(&picked, &Tensor::new(targets, device)?)?)
    };
    let arcs = s
        .arcs
        .broadcast_add(&Tensor::from_vec(col_mask, (b, 1, n), device)?)?;
    let labels = model.labels(s, &Tensor::from_vec(gold_heads, (b, n), device)?)?;
    let n_labels = labels.dim(2)?;

    let total = (ce(&s.upos, N_UPOS, &rows, upos)?
        + (ce(&s.gender, GENDERS.len(), &rows, gender)? * 0.5)?
        + (ce(&s.number, NUMBERS.len(), &rows, number)? * 0.5)?
        + ce(&arcs, n, &arc_rows, arc_targets)?
        + ce(&labels, n_labels, &lab_rows, lab_targets)?)?;
    Ok(total)
}

/// UPOS, UAS et LAS sur un corpus annoté, sans lexique ni ponctuation exclue (comme CoNLL 2018).
fn evaluate(
    model: &ParserModel,
    tokenizer: &Tokenizer,
    cfg: &ParserConfig,
    device: &Device,
    gold: &[GoldSentence],
) -> Result<DevScores> {
    let inputs: Vec<Vec<&str>> = gold.iter().map(|s| s.words()).collect();
    let parsed = parser::parse_with(model, tokenizer, cfg, device, &inputs, 32)?;
    let (mut tokens, mut upos, mut deps, mut uas, mut las) =
        (0usize, 0usize, 0usize, 0usize, 0usize);
    for (s, p) in gold.iter().zip(parsed) {
        for (i, t) in s.tokens.iter().enumerate() {
            tokens += 1;
            let Some(p) = &p else { continue };
            upos += usize::from(p.upos[i] == t.upos);
            if !t.deprel.is_empty() {
                deps += 1;
                if p.heads[i] == t.head {
                    uas += 1;
                    las += usize::from(p.deprels[i] == t.deprel);
                }
            }
        }
    }
    let pct = |a: usize, b: usize| 100.0 * a as f64 / b.max(1) as f64;
    Ok(DevScores {
        upos: pct(upos, tokens),
        uas: pct(uas, deps),
        las: pct(las, deps),
    })
}

fn save(
    out_dir: &Path,
    cfg: &ParserConfig,
    pretrained: &HashMap<String, Tensor>,
    encoder: &VarMap,
    heads: &VarMap,
    xlmr_dir: &Path,
) -> Result<()> {
    std::fs::create_dir_all(out_dir)?;
    let mut all: HashMap<String, Tensor> = pretrained
        .iter()
        .filter(|(k, _)| k.starts_with(FROZEN_PREFIX))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for vars in [encoder, heads] {
        let data = vars
            .data()
            .lock()
            .map_err(|_| crate::Error::Format("verrou empoisonné".into()))?;
        for (k, v) in data.iter() {
            all.insert(k.clone(), v.as_tensor().clone());
        }
    }
    candle_core::safetensors::save(&all, out_dir.join("model.safetensors"))?;
    cfg.save(out_dir.join("parser.json"))?;
    std::fs::copy(
        xlmr_dir.join("tokenizer.json"),
        out_dir.join("tokenizer.json"),
    )?;
    Ok(())
}
