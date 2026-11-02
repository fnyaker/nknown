//! Autorité des documents : PageRank sur le graphe des similarités entre documents.
//!
//! Deux documents sont reliés si l'un de leurs passages l'est (poids : la plus forte
//! similarité). Un document relié à beaucoup d'autres, eux-mêmes reliés, est central dans
//! son sujet. Les valeurs sont normalisées (moyenne 1) ; un document jamais classé vaut 1.

use std::collections::HashMap;

use crate::graph::{EDGE_KEYSPACE, Edge, EdgeKind, NodeId};
use crate::schema::{self, PassageRecord};
use crate::storage::{Op, Storage};
use crate::{Engine, Result, decode};

const DAMPING: f64 = 0.85;
const ITERATIONS: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RankStats {
    pub docs: usize,
    /// Paires de documents reliés.
    pub links: usize,
    /// Autorité maximale (moyenne 1).
    pub max: f64,
}

fn id_at(key: &[u8], at: usize) -> Option<u64> {
    key.get(at..at + 8)?.try_into().ok().map(u64::from_be_bytes)
}

impl<S: Storage> Engine<S> {
    /// Autorité d'un document (1 si elle n'a pas été calculée).
    pub fn authority(&self, doc: NodeId) -> Result<f64> {
        Ok(self
            .get(&schema::id_key(schema::AUTHORITY, doc))?
            .map_or(1.0, |v| f64::from(schema::f32_value(&v)).max(f64::EPSILON)))
    }

    /// Recalcule l'autorité de tous les documents.
    pub fn compute_authority(&mut self) -> Result<RankStats> {
        let mut doc_of: HashMap<u64, u64> = HashMap::new();
        for (k, v) in self.scan(&[schema::PASSAGE])? {
            let rec: PassageRecord = decode(&v)?;
            if let Some(pid) = id_at(&k, 1) {
                doc_of.insert(pid, rec.doc);
            }
        }
        let docs: Vec<u64> = self
            .scan(&[schema::DOC])?
            .iter()
            .filter_map(|(k, _)| id_at(k, 1))
            .collect();
        let index: HashMap<u64, usize> = docs.iter().enumerate().map(|(i, &d)| (d, i)).collect();
        let n = docs.len();

        // Graphe orienté symétrique entre documents.
        let mut weights: HashMap<(usize, usize), f64> = HashMap::new();
        for (k, v) in self.scan(&[EDGE_KEYSPACE])? {
            if k.len() != 18 || k[9] != EdgeKind::Similar as u8 {
                continue;
            }
            let (Some(from), Some(to)) = (id_at(&k, 1), Edge::target(&k)) else {
                continue;
            };
            let (Some(a), Some(b)) = (doc_of.get(&from), doc_of.get(&to.0)) else {
                continue;
            };
            let (Some(&a), Some(&b)) = (index.get(a), index.get(b)) else {
                continue;
            };
            if a != b {
                let w = weights.entry((a, b)).or_default();
                *w = w.max(f64::from(schema::f32_value(&v)));
            }
        }
        let mut out_weight = vec![0.0; n];
        let mut incoming: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
        for (&(a, b), &w) in &weights {
            out_weight[a] += w;
            incoming[b].push((a, w));
        }

        let mut rank = vec![1.0 / n.max(1) as f64; n];
        for _ in 0..ITERATIONS {
            // Les documents sans lien redistribuent leur masse uniformément.
            let dangling: f64 = (0..n)
                .filter(|&i| out_weight[i] == 0.0)
                .map(|i| rank[i])
                .sum();
            let base = (1.0 - DAMPING + DAMPING * dangling) / n as f64;
            rank = (0..n)
                .map(|v| {
                    base + DAMPING
                        * incoming[v]
                            .iter()
                            .map(|&(u, w)| rank[u] * w / out_weight[u])
                            .sum::<f64>()
                })
                .collect();
        }

        let mut ops: Vec<Op> = self
            .scan(&[schema::AUTHORITY])?
            .into_iter()
            .map(|(k, _)| Op::Delete(k))
            .collect();
        let mut max: f64 = 0.0;
        for (i, &doc) in docs.iter().enumerate() {
            let a = rank[i] * n as f64;
            max = max.max(a);
            ops.push(Op::Put(
                schema::id_key(schema::AUTHORITY, NodeId(doc)),
                (a as f32).to_le_bytes().to_vec(),
            ));
        }
        self.apply(ops)?;
        Ok(RankStats {
            docs: n,
            links: weights.len() / 2,
            max,
        })
    }
}
