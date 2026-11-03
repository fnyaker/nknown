//! Carte d'une recherche : requête → mots → résultats → sources, documents apparentés.
//! Sémantique des nœuds et des liens : `docs/ui-guidelines.md`, section 4.

use std::collections::HashMap;

use nknown_api::{Edge, EdgeKind, Graph, GraphPatch, Hit, Node, NodeKind, QueryTermView};

/// Documents apparentés montrés par résultat : au-delà, la carte devient illisible.
const RELATED_PER_HIT: usize = 2;

/// Entrées propres au graphe pour un résultat, en plus de [`Hit`].
#[derive(Debug, Clone, Default)]
pub struct HitLinks {
    /// Contribution de chaque mot de la requête (même ordre que les termes).
    pub term_scores: Vec<f64>,
    /// Paires (adjectif, nom) de la requête, en indices de termes, qualifiées dans le passage.
    pub qualified: Vec<(usize, usize)>,
    /// Documents apparentés : (document, titre, similarité).
    pub related: Vec<(u64, Option<String>, f32)>,
    /// Source brute (`files:/…`, `rss:…`).
    pub source: String,
}

pub fn build(terms: &[QueryTermView], hits: &[Hit], links: &[HitLinks]) -> Graph {
    let mut g = Graph::default();
    g.nodes.push(Node {
        id: "q".into(),
        kind: NodeKind::Query,
        label: String::new(),
        weight: 1.0,
        hit: None,
        doc: None,
        source_kind: None,
    });

    // Contributions, rapportées à la plus forte de la carte : un lien épais apporte beaucoup.
    let max_contrib = links
        .iter()
        .flat_map(|l| l.term_scores.iter().copied())
        .fold(0.0_f64, f64::max);
    let contrib = |h: usize, t: usize| -> f32 {
        let v = links[h].term_scores.get(t).copied().unwrap_or(0.0);
        if max_contrib > 0.0 {
            (v / max_contrib) as f32
        } else {
            0.0
        }
    };

    // Mots ; un adjectif qui qualifie un autre mot de la requête devient un qualifiant.
    let qualifiers: Vec<usize> = links
        .iter()
        .flat_map(|l| l.qualified.iter().map(|&(a, _)| a))
        .collect();
    for (t, term) in terms.iter().enumerate() {
        let total: f32 = (0..hits.len()).map(|h| contrib(h, t)).sum();
        let weight = if hits.is_empty() {
            0.5
        } else {
            total / hits.len() as f32
        };
        g.nodes.push(Node {
            id: term_id(t),
            kind: if qualifiers.contains(&t) {
                NodeKind::Qualifier
            } else {
                NodeKind::Term
            },
            label: term.text.clone(),
            weight,
            hit: None,
            doc: None,
            source_kind: None,
        });
        g.edges.push(Edge {
            from: "q".into(),
            to: term_id(t),
            kind: EdgeKind::QueryTerm,
            weight: 1.0,
        });
    }

    // Qualifications : part des résultats où la paire est attestée.
    let mut pairs: HashMap<(usize, usize), usize> = HashMap::new();
    for l in links {
        for &p in &l.qualified {
            *pairs.entry(p).or_default() += 1;
        }
    }
    let mut pairs: Vec<_> = pairs.into_iter().collect();
    pairs.sort();
    for ((a, b), n) in pairs {
        g.edges.push(Edge {
            from: term_id(a),
            to: term_id(b),
            kind: EdgeKind::Qualifies,
            weight: n as f32 / hits.len().max(1) as f32,
        });
    }

    // Résultats, et leurs liens vers les mots qui les ont trouvés.
    let mut source_hits: Vec<(String, usize, nknown_api::SourceKind)> = Vec::new();
    for (h, hit) in hits.iter().enumerate() {
        let id = doc_id(hit.doc);
        g.nodes.push(Node {
            id: id.clone(),
            kind: if hit.is_code {
                NodeKind::Code
            } else {
                NodeKind::Prose
            },
            label: hit.title.clone().unwrap_or_default(),
            weight: hit.relative,
            hit: Some(h),
            doc: Some(hit.doc),
            source_kind: Some(hit.source_kind),
        });
        for t in 0..terms.len() {
            let w = contrib(h, t);
            if w > 0.0 {
                g.edges.push(Edge {
                    from: term_id(t),
                    to: id.clone(),
                    kind: EdgeKind::TermHit,
                    weight: w,
                });
            }
        }
        let source = &links[h].source;
        match source_hits.iter_mut().find(|(s, _, _)| s == source) {
            Some((_, n, _)) => *n += 1,
            None => source_hits.push((source.clone(), 1, hit.source_kind)),
        }
    }

    // Sources : taille selon la part des résultats qui en viennent.
    for (i, (source, n, kind)) in source_hits.iter().enumerate() {
        g.nodes.push(Node {
            id: format!("s{i}"),
            kind: NodeKind::Source,
            label: crate::sources::label(source),
            weight: *n as f32 / hits.len() as f32,
            hit: None,
            doc: None,
            source_kind: Some(*kind),
        });
    }
    for (h, hit) in hits.iter().enumerate() {
        let i = source_hits
            .iter()
            .position(|(s, _, _)| *s == links[h].source)
            .expect("source recensée");
        g.edges.push(Edge {
            from: doc_id(hit.doc),
            to: format!("s{i}"),
            kind: EdgeKind::HitSource,
            weight: 1.0,
        });
    }

    // Documents apparentés : lien entre deux résultats, ou nœud fantôme.
    let mut ghosts: HashMap<u64, f32> = HashMap::new();
    let mut linked = std::collections::HashSet::new();
    for (h, hit) in hits.iter().enumerate() {
        for (doc, title, sim) in links[h].related.iter().take(RELATED_PER_HIT) {
            let is_hit = hits.iter().any(|o| o.doc == *doc);
            if !linked.insert((hit.doc.min(*doc), hit.doc.max(*doc))) {
                continue; // lien déjà posé depuis l'autre document
            }
            if !is_hit {
                let best = ghosts.entry(*doc).or_insert(0.0);
                if *best == 0.0 {
                    g.nodes.push(Node {
                        id: doc_id(*doc),
                        kind: NodeKind::Related,
                        label: title.clone().unwrap_or_default(),
                        weight: *sim,
                        hit: None,
                        doc: Some(*doc),
                        source_kind: None,
                    });
                }
                *best = best.max(*sim);
            }
            g.edges.push(Edge {
                from: doc_id(hit.doc),
                to: doc_id(*doc),
                kind: EdgeKind::Similar,
                weight: *sim,
            });
        }
    }
    for n in &mut g.nodes {
        if n.kind == NodeKind::Related {
            n.weight = ghosts[&n.doc.expect("document")];
        }
    }
    g
}

/// Ce qui change de `old` à `new` : nœuds et liens nouveaux ou modifiés, nœuds et liens disparus
/// (les liens d'un nœud retiré ne sont pas listés : ils partent avec lui).
pub fn diff(old: &Graph, new: &Graph) -> GraphPatch {
    let mut patch = GraphPatch::default();
    for n in &new.nodes {
        if old.nodes.iter().find(|o| o.id == n.id) != Some(n) {
            patch.nodes.push(n.clone());
        }
    }
    for e in &new.edges {
        if !old.edges.contains(e) {
            patch.edges.push(e.clone());
        }
    }
    patch.removed_nodes = old
        .nodes
        .iter()
        .filter(|o| !new.nodes.iter().any(|n| n.id == o.id))
        .map(|o| o.id.clone())
        .collect();
    let removed = |id: &String| patch.removed_nodes.contains(id);
    patch.removed_edges = old
        .edges
        .iter()
        .filter(|o| !removed(&o.from) && !removed(&o.to))
        .filter(|o| !new.edges.iter().any(|e| e.key() == o.key()))
        .map(Edge::key)
        .collect();
    patch
}

fn term_id(t: usize) -> String {
    format!("t{t}")
}

fn doc_id(doc: u64) -> String {
    format!("d{doc}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use nknown_api::{Nature, ScoreParts, SourceKind};

    fn hit(doc: u64, relative: f32) -> Hit {
        Hit {
            doc,
            passage: doc * 10,
            title: Some(format!("doc {doc}")),
            source: "rss:x".into(),
            source_kind: SourceKind::Feed,
            url: None,
            path: None,
            date: 0,
            score: relative as f64,
            relative,
            snippet: String::new(),
            matched: vec![],
            is_code: doc == 2,
            code_lang: None,
            nature: Nature::Event,
            parts: ScoreParts::default(),
        }
    }

    #[test]
    fn map_structure_and_weights() {
        let terms = vec![
            QueryTermView {
                text: "rues".into(),
                code: false,
            },
            QueryTermView {
                text: "calmes".into(),
                code: false,
            },
        ];
        let hits = vec![hit(1, 1.0), hit(2, 0.5)];
        let links = vec![
            HitLinks {
                term_scores: vec![4.0, 2.0],
                qualified: vec![(1, 0)],
                related: vec![(2, None, 0.8), (9, Some("x".into()), 0.6)],
                source: "rss:x".into(),
            },
            HitLinks {
                term_scores: vec![1.0, 0.0],
                qualified: vec![],
                related: vec![(1, None, 0.8)],
                source: "rss:x".into(),
            },
        ];
        let g = build(&terms, &hits, &links);
        let node = |id: &str| g.nodes.iter().find(|n| n.id == id).unwrap();
        assert_eq!(node("t1").kind, NodeKind::Qualifier);
        assert_eq!(node("d2").kind, NodeKind::Code);
        assert_eq!(node("d9").kind, NodeKind::Related);
        assert_eq!(node("s0").weight, 1.0);
        let edge = |a: &str, b: &str| g.edges.iter().filter(|e| e.from == a && e.to == b).count();
        assert_eq!(edge("t0", "d1"), 1);
        assert_eq!(edge("t1", "d2"), 0, "mot absent : pas de lien");
        assert_eq!(
            edge("d1", "d2") + edge("d2", "d1"),
            1,
            "un seul lien entre résultats"
        );
        let w = g
            .edges
            .iter()
            .find(|e| e.from == "t0" && e.to == "d1")
            .unwrap()
            .weight;
        assert_eq!(w, 1.0);
    }
}
