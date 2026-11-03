//! Disposition de la carte des résultats (`docs/ui-guidelines.md`, section 4).
//!
//! Radiale et stable : la requête au centre, les mots sur un premier anneau, chaque résultat
//! près des mots qui l'ont trouvé et d'autant plus près du centre qu'il est pertinent, les
//! sources en périphérie, les documents apparentés juste derrière leur résultat. Une relaxation
//! à nombre de pas fixe écarte les nœuds qui se chevauchent. Même carte, même disposition : le
//! calcul est déterministe (aucun hasard, ordre des nœuds fixé par leur identifiant).
//!
//! Les coordonnées sont en pixels logiques, centrées sur la requête.

use std::collections::HashMap;
use std::f32::consts::{PI, TAU};

use nknown_api::{EdgeKind, Graph, NodeKind};

/// Rayon d'un nœud (px) : de 6 (minimum lisible) à 28, aire proportionnelle à la valeur.
pub const MIN_RADIUS: f32 = 6.0;
pub const MAX_RADIUS: f32 = 28.0;

/// Anneaux (px depuis le centre).
const TERM_RING: f32 = 110.0;
/// Le meilleur résultat est à `HIT_NEAR`, le plus faible à `HIT_FAR`.
const HIT_NEAR: f32 = 220.0;
const HIT_FAR: f32 = 320.0;
const SOURCE_RING: f32 = 410.0;
/// Distance d'un document apparenté à son résultat.
const RELATED_OFFSET: f32 = 64.0;
/// Espace minimal entre deux nœuds.
const GAP: f32 = 10.0;
/// Espace entre deux résultats voisins sur leur anneau (place pour leurs étiquettes).
const HIT_GAP: f32 = 34.0;
const RELAX_STEPS: usize = 80;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    pub x: f32,
    pub y: f32,
    pub radius: f32,
}

/// Rayon d'un nœud selon sa nature et son poids (`[0, 1]`).
pub fn radius(kind: NodeKind, weight: f32) -> f32 {
    let w = weight.clamp(0.0, 1.0);
    let scaled = |lo: f32, hi: f32| (lo * lo + (hi * hi - lo * lo) * w).sqrt();
    match kind {
        NodeKind::Query => 22.0,
        NodeKind::Qualifier => 7.0,
        NodeKind::Term => scaled(10.0, 22.0),
        NodeKind::Prose | NodeKind::Code => scaled(MIN_RADIUS + 2.0, MAX_RADIUS),
        NodeKind::Source => scaled(MIN_RADIUS, 14.0),
        NodeKind::Related => scaled(MIN_RADIUS, 12.0),
    }
}

fn polar(angle: f32, r: f32) -> (f32, f32) {
    (r * angle.cos(), r * angle.sin())
}

/// Angle moyen pondéré (moyenne des vecteurs unitaires) ; `None` si les poids s'annulent.
fn mean_angle(angles: &[(f32, f32)]) -> Option<f32> {
    let (x, y) = angles.iter().fold((0.0, 0.0), |(x, y), &(a, w)| {
        (x + w * a.cos(), y + w * a.sin())
    });
    (x.abs() + y.abs() > 1e-6).then(|| y.atan2(x))
}

/// Répartit des nœuds à intervalles égaux sur un cercle, dans l'ordre de leur angle voulu, tournés
/// pour coller au mieux à ces angles. Peu nombreux (moins de 3), ils gardent leurs angles voulus,
/// simplement écartés au besoin (voir [`spread`]).
fn even_ring(
    items: &mut [(usize, f32)],
    id: impl Fn(usize) -> String,
    half: impl Fn(usize) -> f32,
) -> Vec<(usize, f32)> {
    let n = items.len();
    if n < 3 {
        return spread(items, id, half);
    }
    let sorted = spread(items, id, |_| 0.0); // tri seulement
    let step = TAU / n as f32;
    // Rotation : moyenne circulaire des écarts entre angle voulu et place régulière.
    let offset = mean_angle(
        &sorted
            .iter()
            .enumerate()
            .map(|(k, &(_, a))| (a - step * k as f32, 1.0))
            .collect::<Vec<_>>(),
    )
    .unwrap_or(0.0);
    sorted
        .iter()
        .enumerate()
        .map(|(k, &(i, _))| (i, offset + step * k as f32))
        .collect()
}

/// Répartit des nœuds sur un cercle dans l'ordre de leur angle voulu, en garantissant à chacun
/// un demi-écart angulaire `half(i)` de chaque côté : les voisins trop proches s'écartent
/// symétriquement, au plus près des angles voulus. Déterministe.
fn spread(
    items: &mut [(usize, f32)],
    id: impl Fn(usize) -> String,
    half: impl Fn(usize) -> f32,
) -> Vec<(usize, f32)> {
    let n = items.len();
    if n < 2 {
        return items.to_vec();
    }
    // Angles arrondis (les sommes flottantes varient au dernier bit selon l'ordre des liens),
    // égalités départagées par l'identifiant : l'ordre des nœuds en entrée ne compte pas.
    let key = |a: f32| (a * 1e4).round() as i64;
    items.sort_by(|a, b| key(a.1).cmp(&key(b.1)).then_with(|| id(a.0).cmp(&id(b.0))));
    let need: Vec<f32> = items.iter().map(|&(i, _)| half(i)).collect();
    let total: f32 = need.iter().sum::<f32>() * 2.0;
    // Trop nombreux pour le cercle : on resserre proportionnellement.
    let squeeze = if total > TAU { TAU / total } else { 1.0 };
    let mut a: Vec<f32> = items.iter().map(|&(_, a)| a).collect();
    for _ in 0..200 {
        let mut moved = false;
        for k in 0..n {
            let next = (k + 1) % n;
            let mut gap = a[next] - a[k];
            if next == 0 {
                gap += TAU;
            }
            let min = (need[k] + need[next]) * squeeze;
            if gap < min - 1e-4 {
                let push = (min - gap) / 2.0;
                a[k] -= push;
                a[next] += push;
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
    items.iter().zip(a).map(|(&(i, _), a)| (i, a)).collect()
}

/// Disposition de toute la carte, par identifiant de nœud.
pub fn layout(graph: &Graph) -> HashMap<String, Placed> {
    let mut order: Vec<usize> = (0..graph.nodes.len()).collect();
    order.sort_by(|&a, &b| graph.nodes[a].id.cmp(&graph.nodes[b].id));
    let by_id: HashMap<&str, usize> = graph
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect();
    let edges_of = |kind: EdgeKind| graph.edges.iter().filter(move |e| e.kind == kind);
    let mut angle: HashMap<usize, f32> = HashMap::new();
    let mut pos: Vec<Option<(f32, f32)>> = vec![None; graph.nodes.len()];

    // Mots (et qualifiants) : répartis sur l'anneau dans l'ordre de la requête, à partir du haut.
    let mut terms: Vec<usize> = order
        .iter()
        .copied()
        .filter(|&i| matches!(graph.nodes[i].kind, NodeKind::Term | NodeKind::Qualifier))
        .collect();
    terms.sort_by_key(|&i| {
        let id = &graph.nodes[i].id;
        (id.len(), id.clone()) // t2 avant t10
    });
    let n_terms = terms.len().max(1) as f32;
    for (k, &i) in terms.iter().enumerate() {
        let a = -PI / 2.0 + TAU * k as f32 / n_terms;
        angle.insert(i, a);
        pos[i] = Some(polar(a, TERM_RING));
    }
    for (i, n) in graph.nodes.iter().enumerate() {
        if n.kind == NodeKind::Query {
            pos[i] = Some((0.0, 0.0));
        }
    }

    // Résultats : chacun veut la direction des mots qui l'ont trouvé (le mot dominant
    // l'emporte) ; ils sont répartis à intervalles égaux sur leur couronne, dans l'ordre de ces
    // directions, avec la rotation qui colle le mieux à elles : la carte reste équilibrée même
    // quand un mot domine tous les résultats. Distance au centre : selon le score.
    let mut hits: Vec<usize> = order
        .iter()
        .copied()
        .filter(|&i| matches!(graph.nodes[i].kind, NodeKind::Prose | NodeKind::Code))
        .collect();
    let mut preferred: Vec<(usize, f32)> = Vec::with_capacity(hits.len());
    hits.sort_by(|&a, &b| {
        graph.nodes[b]
            .weight
            .total_cmp(&graph.nodes[a].weight)
            .then(graph.nodes[a].id.cmp(&graph.nodes[b].id))
    });
    for (rank, &i) in hits.iter().enumerate() {
        let id = graph.nodes[i].id.as_str();
        // Poids au carré : un mot qui pèse deux fois plus tire quatre fois plus.
        let toward: Vec<(f32, f32)> = edges_of(EdgeKind::TermHit)
            .filter(|e| e.to == id)
            .filter_map(|e| {
                Some((
                    *angle.get(by_id.get(e.from.as_str())?)?,
                    e.weight * e.weight,
                ))
            })
            .collect();
        // Sans mot (cas limite) : réparti selon le rang, en spirale dorée.
        let a = mean_angle(&toward).unwrap_or(rank as f32 * 2.399_963);
        preferred.push((i, a));
    }
    let node_id = |i: usize| graph.nodes[i].id.clone();
    let radii_hit = |i: usize| radius(graph.nodes[i].kind, graph.nodes[i].weight);
    let ring = (HIT_NEAR + HIT_FAR) / 2.0;
    for (i, a) in even_ring(&mut preferred, node_id, |i| {
        (radii_hit(i) + HIT_GAP / 2.0) / ring
    }) {
        let r = HIT_NEAR + (HIT_FAR - HIT_NEAR) * (1.0 - graph.nodes[i].weight.clamp(0.0, 1.0));
        angle.insert(i, a);
        pos[i] = Some(polar(a, r));
    }

    // Sources : en périphérie, vers leurs résultats.
    let mut sources: Vec<(usize, f32)> = Vec::new();
    for &i in order
        .iter()
        .filter(|&&i| graph.nodes[i].kind == NodeKind::Source)
    {
        let id = graph.nodes[i].id.as_str();
        let toward: Vec<(f32, f32)> = edges_of(EdgeKind::HitSource)
            .filter(|e| e.to == id)
            .filter_map(|e| Some((*angle.get(by_id.get(e.from.as_str())?)?, 1.0)))
            .collect();
        sources.push((i, mean_angle(&toward).unwrap_or(PI / 2.0)));
    }
    for (i, a) in spread(&mut sources, node_id, |_| 0.16) {
        angle.insert(i, a);
        pos[i] = Some(polar(a, SOURCE_RING));
    }

    // Apparentés : derrière leur résultat (le plus proche), légèrement de côté.
    let mut side = 1.0;
    for &i in order
        .iter()
        .filter(|&&i| graph.nodes[i].kind == NodeKind::Related)
    {
        let id = graph.nodes[i].id.as_str();
        let anchor = edges_of(EdgeKind::Similar)
            .filter(|e| e.to == id || e.from == id)
            .max_by(|a, b| a.weight.total_cmp(&b.weight))
            .and_then(|e| {
                by_id.get(if e.to == id {
                    e.from.as_str()
                } else {
                    e.to.as_str()
                })
            })
            .copied();
        let (ax, ay) = anchor.and_then(|a| pos[a]).unwrap_or((0.0, HIT_FAR));
        let a = ay.atan2(ax) + side * 0.35;
        side = -side;
        let r = (ax * ax + ay * ay).sqrt() + RELATED_OFFSET;
        pos[i] = Some(polar(a, r));
    }

    let radii: Vec<f32> = graph
        .nodes
        .iter()
        .map(|n| radius(n.kind, n.weight))
        .collect();
    let mut xy: Vec<(f32, f32)> = pos.into_iter().map(|p| p.unwrap_or((0.0, 0.0))).collect();
    relax(graph, &order, &radii, &mut xy);

    graph
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            (
                n.id.clone(),
                Placed {
                    x: xy[i].0,
                    y: xy[i].1,
                    radius: radii[i],
                },
            )
        })
        .collect()
}

/// Écarte les nœuds qui se chevauchent ; la requête et les mots restent fixes (repères).
fn relax(graph: &Graph, order: &[usize], radii: &[f32], xy: &mut [(f32, f32)]) {
    let fixed = |i: usize| {
        matches!(
            graph.nodes[i].kind,
            NodeKind::Query | NodeKind::Term | NodeKind::Qualifier
        )
    };
    for _ in 0..RELAX_STEPS {
        let mut moved = false;
        for (k, &a) in order.iter().enumerate() {
            for &b in &order[k + 1..] {
                let (dx, dy) = (xy[b].0 - xy[a].0, xy[b].1 - xy[a].1);
                let min = radii[a] + radii[b] + GAP;
                let d2 = dx * dx + dy * dy;
                if d2 >= min * min {
                    continue;
                }
                let d = d2.sqrt();
                // Superposition exacte : direction fixée par l'ordre, pas par le hasard.
                let (ux, uy) = if d > 1e-3 {
                    (dx / d, dy / d)
                } else {
                    polar(k as f32 * 2.399_963, 1.0)
                };
                let push = min - d;
                let (fa, fb) = match (fixed(a), fixed(b)) {
                    (true, true) => continue,
                    (true, false) => (0.0, 1.0),
                    (false, true) => (1.0, 0.0),
                    (false, false) => (0.5, 0.5),
                };
                xy[a].0 -= ux * push * fa;
                xy[a].1 -= uy * push * fa;
                xy[b].0 += ux * push * fb;
                xy[b].1 += uy * push * fb;
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nknown_api::{Edge, Node};

    fn node(id: &str, kind: NodeKind, weight: f32) -> Node {
        Node {
            id: id.into(),
            kind,
            label: id.into(),
            weight,
            hit: None,
            doc: None,
            source_kind: None,
        }
    }

    fn edge(from: &str, to: &str, kind: EdgeKind, weight: f32) -> Edge {
        Edge {
            from: from.into(),
            to: to.into(),
            kind,
            weight,
        }
    }

    fn sample() -> Graph {
        let mut g = Graph::default();
        g.nodes.push(node("q", NodeKind::Query, 1.0));
        for t in ["t0", "t1", "t2"] {
            g.nodes.push(node(t, NodeKind::Term, 0.5));
            g.edges.push(edge("q", t, EdgeKind::QueryTerm, 1.0));
        }
        for (k, w) in [1.0, 0.8, 0.6, 0.5, 0.4, 0.3, 0.3, 0.2].iter().enumerate() {
            let id = format!("d{k}");
            g.nodes.push(node(&id, NodeKind::Prose, *w));
            g.edges
                .push(edge(&format!("t{}", k % 3), &id, EdgeKind::TermHit, 0.9));
            g.edges.push(edge(&id, "s0", EdgeKind::HitSource, 1.0));
        }
        g.nodes.push(node("s0", NodeKind::Source, 1.0));
        g.nodes.push(node("d99", NodeKind::Related, 0.6));
        g.edges.push(edge("d0", "d99", EdgeKind::Similar, 0.6));
        g
    }

    fn dist(p: &Placed) -> f32 {
        (p.x * p.x + p.y * p.y).sqrt()
    }

    #[test]
    fn deterministic_and_order_independent() {
        let g = sample();
        let a = layout(&g);
        assert_eq!(a, layout(&g));
        let mut shuffled = g.clone();
        shuffled.nodes.reverse();
        shuffled.edges.reverse();
        let b = layout(&shuffled);
        for (id, p) in &a {
            let q = b[id];
            assert!((p.x - q.x).abs() < 1e-3 && (p.y - q.y).abs() < 1e-3, "{id}");
        }
    }

    #[test]
    fn rings_and_relevance() {
        let p = layout(&sample());
        assert_eq!((p["q"].x, p["q"].y), (0.0, 0.0));
        // Le premier mot en haut, sur son anneau.
        assert!((p["t0"].y + TERM_RING).abs() < 1e-3);
        // Le meilleur résultat est plus près que le plus faible ; la source est au bord.
        assert!(dist(&p["d0"]) < dist(&p["d7"]));
        assert!(dist(&p["s0"]) > dist(&p["d0"]));
        // L'apparenté est derrière son résultat.
        assert!(dist(&p["d99"]) > dist(&p["d0"]));
        // Un résultat est du côté du mot qui l'a trouvé.
        let (t, d) = (p["t1"], p["d1"]);
        assert!(t.x * d.x + t.y * d.y > 0.0);
    }

    #[test]
    fn nodes_do_not_overlap() {
        let mut g = sample();
        // Beaucoup de résultats au même endroit.
        for k in 10..40 {
            let id = format!("d{k}");
            g.nodes.push(node(&id, NodeKind::Code, 0.5));
            g.edges.push(edge("t0", &id, EdgeKind::TermHit, 1.0));
        }
        let p = layout(&g);
        let ids: Vec<&String> = p.keys().collect();
        for (k, a) in ids.iter().enumerate() {
            for b in &ids[k + 1..] {
                let (pa, pb) = (p[*a], p[*b]);
                let d = ((pa.x - pb.x).powi(2) + (pa.y - pb.y).powi(2)).sqrt();
                assert!(
                    d >= pa.radius + pb.radius - 0.5,
                    "{a} et {b} se chevauchent"
                );
            }
        }
    }

    #[test]
    fn sizes_follow_weight() {
        assert_eq!(radius(NodeKind::Prose, 0.0), MIN_RADIUS + 2.0);
        assert_eq!(radius(NodeKind::Prose, 1.0), MAX_RADIUS);
        assert!(radius(NodeKind::Code, 0.25) < radius(NodeKind::Code, 0.5));
    }
}
