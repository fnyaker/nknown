//! La carte des résultats en SVG. Disposition : `nknown-graph` (la même que l'app) ; les éléments
//! restent en place d'un état à l'autre (clés stables), si bien que les transitions CSS animent
//! les déplacements, et qu'un nœud nouveau éclôt.

use std::collections::{HashMap, HashSet};

use leptos::prelude::*;
use nknown_api::{EdgeKind, Graph, NodeKind};

#[derive(Debug, Clone, PartialEq)]
pub struct NodeView {
    pub id: String,
    pub kind: &'static str,
    pub label: String,
    pub x: f32,
    pub y: f32,
    pub r: f32,
    pub hit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EdgeView {
    pub id: String,
    pub from: String,
    pub to: String,
    pub kind: &'static str,
    pub weight: f32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MapState {
    pub nodes: HashMap<String, NodeView>,
    pub node_ids: Vec<String>,
    pub edges: HashMap<String, EdgeView>,
    pub edge_ids: Vec<String>,
    /// (x0, y0, x1, y1).
    pub bounds: (f32, f32, f32, f32),
}

pub fn kind_name(k: NodeKind) -> &'static str {
    match k {
        NodeKind::Query => "query",
        NodeKind::Term => "term",
        NodeKind::Qualifier => "qualifier",
        NodeKind::Prose => "prose",
        NodeKind::Code => "code",
        NodeKind::Source => "source",
        NodeKind::Related => "related",
    }
}

fn edge_name(k: EdgeKind) -> &'static str {
    match k {
        EdgeKind::QueryTerm => "query_term",
        EdgeKind::TermHit => "term_hit",
        EdgeKind::Qualifies => "qualifies",
        EdgeKind::HitSource => "hit_source",
        EdgeKind::Similar => "similar",
    }
}

pub fn kind_label(kind: &str) -> &'static str {
    match kind {
        "query" => "requête",
        "term" => "mot",
        "qualifier" => "qualifiant",
        "prose" => "texte",
        "code" => "code",
        "source" => "source",
        _ => "apparenté",
    }
}

impl MapState {
    pub fn from_graph(graph: &Graph) -> Self {
        let placed = nknown_graph::layout(graph);
        let mut s = Self::default();
        let (mut x0, mut y0, mut x1, mut y1) = (0f32, 0f32, 0f32, 0f32);
        for n in &graph.nodes {
            let p = placed[&n.id];
            x0 = x0.min(p.x - p.radius);
            y0 = y0.min(p.y - p.radius);
            x1 = x1.max(p.x + p.radius);
            y1 = y1.max(p.y + p.radius);
            s.node_ids.push(n.id.clone());
            s.nodes.insert(
                n.id.clone(),
                NodeView {
                    id: n.id.clone(),
                    kind: kind_name(n.kind),
                    label: n.label.clone(),
                    x: p.x,
                    y: p.y,
                    r: p.radius,
                    hit: n.hit,
                },
            );
        }
        for e in &graph.edges {
            // Liens mot → résultat négligeables : non tracés (charte, hiérarchie des liens).
            if e.kind == EdgeKind::TermHit && e.weight < 0.12 {
                continue;
            }
            let id = format!("{}>{}:{}", e.from, e.to, edge_name(e.kind));
            s.edge_ids.push(id.clone());
            s.edges.insert(
                id.clone(),
                EdgeView {
                    id,
                    from: e.from.clone(),
                    to: e.to.clone(),
                    kind: edge_name(e.kind),
                    weight: e.weight,
                },
            );
        }
        s.bounds = (x0, y0, x1, y1);
        s
    }

    /// Le nœud, ses voisins et, pour un résultat, les mots et la requête qui y mènent.
    pub fn lit(&self, selected: &str) -> HashSet<String> {
        let mut lit = HashSet::new();
        if selected.is_empty() {
            return lit;
        }
        lit.insert(selected.to_owned());
        for e in self.edges.values() {
            let other = if e.from == selected {
                &e.to
            } else if e.to == selected {
                &e.from
            } else {
                continue;
            };
            lit.insert(other.clone());
            if e.kind == "term_hit" || e.kind == "qualifies" {
                lit.insert("q".into());
            }
        }
        lit
    }

    /// Liens d'un nœud, du plus lourd au plus léger : (autre nœud, poids).
    pub fn links(&self, id: &str) -> Vec<(NodeView, f32)> {
        let mut out: Vec<(NodeView, f32)> = self
            .edges
            .values()
            .filter_map(|e| {
                let other = if e.from == id {
                    &e.to
                } else if e.to == id {
                    &e.from
                } else {
                    return None;
                };
                Some((self.nodes.get(other)?.clone(), e.weight))
            })
            .collect();
        out.sort_by(|a, b| b.1.total_cmp(&a.1));
        out
    }

    /// Tracé d'un lien : courbe incurvée vers l'extérieur (contourne le centre) ; droit entre la
    /// requête et un mot.
    fn path(&self, e: &EdgeView) -> String {
        let (Some(a), Some(b)) = (self.nodes.get(&e.from), self.nodes.get(&e.to)) else {
            return String::new();
        };
        let (mx, my) = ((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
        let bend = if e.kind == "query_term" {
            0.0
        } else {
            ((b.x - a.x).hypot(b.y - a.y) / 900.0).min(0.45)
        };
        let (cx, cy) = if mx.hypot(my) > 30.0 {
            (mx * (1.0 + bend), my * (1.0 + bend))
        } else {
            (mx - (b.y - a.y) * bend, my + (b.x - a.x) * bend)
        };
        format!(
            "M{:.1},{:.1} Q{cx:.1},{cy:.1} {:.1},{:.1}",
            a.x, a.y, b.x, b.y
        )
    }
}

/// La carte. `selected` : nœud mis en valeur ; `on_pick` : clic sur un nœud.
#[component]
pub fn Map(
    state: RwSignal<MapState>,
    selected: RwSignal<String>,
    busy: Signal<bool>,
    on_pick: Callback<String>,
) -> impl IntoView {
    let lit = Memo::new(move |_| state.with(|s| s.lit(&selected.get())));
    // Caméra : toute la carte (sans grossir une petite carte), ou le nœud sélectionné.
    let camera = move || {
        state.with(|s| {
            let (x0, y0, x1, y1) = s.bounds;
            let fit = (920.0 / (x1 - x0 + 60.0))
                .min(920.0 / (y1 - y0 + 60.0))
                .min(1.0);
            let sel = selected.get();
            match s.nodes.get(&sel) {
                Some(n) => {
                    let z = fit.max(0.9);
                    format!(
                        "transform: translate({}px, {}px) scale({z})",
                        -n.x * z,
                        -n.y * z
                    )
                }
                None => format!(
                    "transform: translate({}px, {}px) scale({fit})",
                    -(x0 + x1) / 2.0 * fit,
                    -(y0 + y1) / 2.0 * fit
                ),
            }
        })
    };
    view! {
        <svg viewBox="-500 -500 1000 1000" preserveAspectRatio="xMidYMid meet"
            class:busy=move || busy.get()>
            <g class="camera" style=camera>
                <For
                    each=move || state.with(|s| s.edge_ids.clone())
                    key=|id| id.clone()
                    children=move |id: String| {
                        let id2 = id.clone();
                        let edge = Signal::derive(move || state.with(|s| s.edges.get(&id2).cloned()));
                        let id3 = id.clone();
                        view! {
                            <path
                                class=move || {
                                    let e = edge.get();
                                    let kind = e.as_ref().map_or("", |e| e.kind);
                                    let dim = e.as_ref().is_some_and(|e| {
                                        !selected.get().is_empty()
                                            && !lit.with(|l| l.contains(&e.from) && l.contains(&e.to))
                                    });
                                    format!("edge {kind}{}", if dim { " dim" } else { "" })
                                }
                                d=move || state.with(|s| s.edges.get(&id3).map(|e| s.path(e)).unwrap_or_default())
                                stroke-width=move || edge.get().map_or(1.0, |e| match e.kind {
                                    "query_term" => 1.2,
                                    "hit_source" | "similar" => 1.0,
                                    _ => 0.5 + 3.5 * e.weight,
                                })
                                stroke-opacity=move || edge.get().map_or(0.0, |e| match e.kind {
                                    "query_term" => 0.7,
                                    "term_hit" => 0.15 + 0.7 * e.weight,
                                    "qualifies" => 0.9,
                                    "hit_source" => 0.3,
                                    _ => 0.18 + 0.3 * e.weight,
                                })
                            />
                        }
                    }
                />
                <For
                    each=move || state.with(|s| s.node_ids.clone())
                    key=|id| id.clone()
                    children=move |id: String| {
                        let id2 = id.clone();
                        let node = Signal::derive(move || state.with(|s| s.nodes.get(&id2).cloned()));
                        let pick = id.clone();
                        let id_sel = id.clone();
                        let id_lit = id.clone();
                        let is_sel = Signal::derive(move || selected.get() == id_sel);
                        // Forme redessinée seulement si nature, taille ou étiquette changent : un
                        // simple déplacement est animé par la transition, sans nouvelle éclosion.
                        let id_look = id.clone();
                        let look = Memo::new(move |_| {
                            state.with(|s| {
                                s.nodes.get(&id_look).map(|n| NodeView {
                                    x: 0.0,
                                    y: 0.0,
                                    r: (n.r * 2.0).round() / 2.0,
                                    ..n.clone()
                                })
                            })
                        });
                        view! {
                            <g
                                class=move || {
                                    let n = node.get();
                                    let kind = n.as_ref().map_or("", |n| n.kind);
                                    let dim = !selected.get().is_empty() && !lit.with(|l| l.contains(&id_lit));
                                    format!("node {kind}{}{}", if dim { " dim" } else { "" }, if is_sel.get() { " selected" } else { "" })
                                }
                                style=move || node.get().map(|n| format!("transform: translate({:.1}px, {:.1}px)", n.x, n.y)).unwrap_or_default()
                                on:click=move |ev| {
                                    ev.stop_propagation();
                                    on_pick.run(pick.clone());
                                }
                            >
                                {move || look.get().map(|n| shape(&n, is_sel.get()))}
                            </g>
                        }
                    }
                />
            </g>
        </svg>
    }
}

/// Forme d'un nœud (charte, section 4) et son étiquette.
fn shape(n: &NodeView, selected: bool) -> impl IntoView + use<> {
    let r = n.r;
    let class = format!("shape s-{}", n.kind);
    let body = match n.kind {
        "query" => view! { <circle class=class r=r - 1.5 stroke-width=3 /> }.into_any(),
        "term" | "source" => view! { <circle class=class r=r /> }.into_any(),
        "qualifier" => {
            let s = r * 1.4;
            view! { <rect class=class x=-s / 2.0 y=-s / 2.0 width=s height=s rx=2 transform="rotate(45)" /> }
                .into_any()
        }
        _ => {
            let group = class.clone();
            view! {
            <g class=group>
                <rect class=class x=-r y=-r width=2.0 * r height=2.0 * r rx=r * 0.56 />
                {(n.kind == "code" && r >= 7.0).then(|| view! { <text class="code-mark">"</>"</text> })}
            </g>
            }
            .into_any()
        }
    };
    // Étiquette : mots et sources toujours ; résultats parmi les trois premiers, ou sélectionnés.
    let labelled = matches!(n.kind, "term" | "qualifier" | "source")
        || selected
        || n.hit.is_some_and(|h| h < 3);
    let label: String = n.label.chars().take(28).collect();
    view! {
        {selected.then(|| view! { <circle class="halo" r=r + 7.0 /> })}
        {body}
        {(labelled && !label.is_empty()).then(|| view! { <text class="label" y=r + 15.0>{label}</text> })}
    }
}
