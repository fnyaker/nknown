//! La carte des résultats en SVG. Disposition : `nknown-graph` (la même que l'app) ; les éléments
//! restent en place d'un état à l'autre (clés stables), si bien que les transitions CSS animent
//! les déplacements, et qu'un nœud nouveau éclôt.
//!
//! Le contact est unique : le pointeur l'emporte sur le focus clavier. Un lien exact s'allume seul ;
//! un nœud allume ses liens et ses voisins. La sélection reste indépendante et diminue le reste.

use std::collections::{HashMap, HashSet};

use leptos::prelude::*;
use nknown_api::{Graph, NodeKind};

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

/// Un lien vu depuis un nœud : l'autre extrémité et l'arête exacte.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkView {
    pub other: NodeView,
    pub kind: &'static str,
    pub weight: f32,
    pub from: String,
    pub to: String,
    pub edge_id: String,
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

/// Contact courant : deux sources, le pointeur l'emporte sur le focus.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Contact {
    pub pointer_edge: String,
    pub pointer_node: String,
    pub focus_edge: String,
    pub focus_node: String,
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

/// Seuil unique des liens mot → résultat non tracés (même règle que `Theme.edgeHidden`).
pub const HIDDEN_TERM_WEIGHT: f32 = 0.12;

pub fn edge_hidden(kind: &str, weight: f32) -> bool {
    kind == "term_hit" && weight < HIDDEN_TERM_WEIGHT
}

/// Épaisseur d'un lien (charte, section 4) ; le contact ne la change jamais.
pub fn edge_width(kind: &str, weight: f32) -> f32 {
    match kind {
        "query_term" => 1.2,
        "hit_source" => 1.0,
        _ => 0.5 + 3.5 * weight.clamp(0.0, 1.0),
    }
}

/// Opacité de base d'un lien.
pub fn edge_opacity(kind: &str, weight: f32) -> f32 {
    match kind {
        "query_term" => 0.70,
        "hit_source" => 0.30,
        _ => 0.30 + 0.70 * weight.clamp(0.0, 1.0),
    }
}

/// Opacité rendue : +0,30 au contact, ×0,25 hors sélection (le contact est exempté).
pub fn edge_render_opacity(e: &EdgeView, contact: bool, dimmed: bool) -> f32 {
    let base = edge_opacity(e.kind, e.weight);
    if contact {
        (base + 0.30).min(1.0)
    } else if dimmed {
        base * 0.25
    } else {
        base
    }
}

impl Contact {
    pub fn set_pointer_edge(&mut self, id: impl Into<String>) {
        self.pointer_edge = id.into();
        self.pointer_node.clear();
    }
    pub fn set_pointer_node(&mut self, id: impl Into<String>) {
        self.pointer_node = id.into();
        self.pointer_edge.clear();
    }
    pub fn set_focus_edge(&mut self, id: impl Into<String>) {
        self.focus_edge = id.into();
        self.focus_node.clear();
    }
    pub fn set_focus_node(&mut self, id: impl Into<String>) {
        self.focus_node = id.into();
        self.focus_edge.clear();
    }
    pub fn clear_pointer(&mut self) {
        self.pointer_edge.clear();
        self.pointer_node.clear();
    }
    pub fn clear_focus(&mut self) {
        self.focus_edge.clear();
        self.focus_node.clear();
    }
    pub fn clear_pointer_edge(&mut self, id: &str) {
        if self.pointer_edge == id {
            self.pointer_edge.clear()
        }
    }
    pub fn clear_pointer_node(&mut self, id: &str) {
        if self.pointer_node == id {
            self.pointer_node.clear()
        }
    }
    pub fn clear_focus_edge(&mut self, id: &str) {
        if self.focus_edge == id {
            self.focus_edge.clear()
        }
    }
    pub fn clear_focus_node(&mut self, id: &str) {
        if self.focus_node == id {
            self.focus_node.clear()
        }
    }
    /// Retire les identifiants disparus après une mise à jour de la carte.
    pub fn prune(&mut self, state: &MapState) {
        if !state.edges.contains_key(&self.pointer_edge) {
            self.pointer_edge.clear()
        }
        if !state.edges.contains_key(&self.focus_edge) {
            self.focus_edge.clear()
        }
        if !state.nodes.contains_key(&self.pointer_node) {
            self.pointer_node.clear()
        }
        if !state.nodes.contains_key(&self.focus_node) {
            self.focus_node.clear()
        }
    }

    pub fn edge(&self) -> &str {
        if !self.pointer_edge.is_empty() {
            &self.pointer_edge
        } else if !self.pointer_node.is_empty() {
            ""
        } else {
            &self.focus_edge
        }
    }

    pub fn node(&self) -> &str {
        if !self.pointer_node.is_empty() {
            &self.pointer_node
        } else if !self.pointer_edge.is_empty() || !self.focus_edge.is_empty() {
            ""
        } else {
            &self.focus_node
        }
    }

    /// Extrémités du lien exact au contact (aucune pour un nœud).
    pub fn endpoints(&self, state: &MapState) -> HashSet<String> {
        let mut out = HashSet::new();
        let edge = self.edge();
        if !edge.is_empty()
            && let Some(e) = state.edges.get(edge)
        {
            out.insert(e.from.clone());
            out.insert(e.to.clone());
        }
        out
    }

    /// Nœuds au contact : extrémités d'un lien exact, ou nœud et voisins directs.
    pub fn nodes(&self, state: &MapState) -> HashSet<String> {
        let mut out = self.endpoints(state);
        if !out.is_empty() {
            return out;
        }
        let node = self.node();
        if node.is_empty() {
            return out;
        }
        out.insert(node.to_owned());
        for e in state.edges.values() {
            if e.from == node {
                out.insert(e.to.clone());
            } else if e.to == node {
                out.insert(e.from.clone());
            }
        }
        out
    }

    pub fn hot_edge(&self, e: &EdgeView) -> bool {
        let edge = self.edge();
        if !edge.is_empty() {
            return e.id == edge;
        }
        let node = self.node();
        !node.is_empty() && (e.from == node || e.to == node)
    }

    pub fn hot_link(&self, l: &LinkView) -> bool {
        let edge = self.edge();
        if !edge.is_empty() {
            return l.edge_id == edge;
        }
        let node = self.node();
        !node.is_empty() && (l.from == node || l.to == node)
    }

    pub fn direct_node(&self, id: &str) -> bool {
        self.node() == id
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
            // Liens mot → résultat négligeables : non tracés (même prédicat que l'app).
            if edge_hidden(e.kind.as_str(), e.weight) {
                continue;
            }
            let id = e.view_id();
            s.edge_ids.push(id.clone());
            s.edges.insert(
                id.clone(),
                EdgeView {
                    id,
                    from: e.from.clone(),
                    to: e.to.clone(),
                    kind: e.kind.as_str(),
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

    /// Liens d'un nœud, du plus lourd au plus léger, avec l'identité exacte de l'arête.
    pub fn links(&self, id: &str) -> Vec<LinkView> {
        let mut out: Vec<LinkView> = self
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
                Some(LinkView {
                    other: self.nodes.get(other)?.clone(),
                    kind: e.kind,
                    weight: e.weight,
                    from: e.from.clone(),
                    to: e.to.clone(),
                    edge_id: e.id.clone(),
                })
            })
            .collect();
        out.sort_by(|a, b| {
            b.weight
                .total_cmp(&a.weight)
                .then_with(|| a.other.id.cmp(&b.other.id))
                .then_with(|| a.edge_id.cmp(&b.edge_id))
        });
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

/// La carte. `selected` : nœud mis en valeur ; `contact` : survol/focus ; `on_pick` : clic.
#[component]
pub fn Map(
    state: RwSignal<MapState>,
    selected: RwSignal<String>,
    contact: RwSignal<Contact>,
    busy: Signal<bool>,
    on_pick: Callback<String>,
) -> impl IntoView {
    let lit = Memo::new(move |_| state.with(|s| s.lit(&selected.get())));
    let contact_nodes = Memo::new(move |_| contact.with(|c| state.with(|s| c.nodes(s))));
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
                        let id_path = id.clone();
                        let id_class = id.clone();
                        let id_enter = id.clone();
                        let id_leave = id.clone();
                        let id_hit = id.clone();
                        let id_casing = id.clone();
                        let edge = Signal::derive(move || state.with(|s| s.edges.get(&id_path).cloned()));
                        view! {
                            <g class="edge-group">
                                <path
                                    class="edge-hit"
                                    aria-hidden="true"
                                    d=move || state.with(|s| s.edges.get(&id_hit).map(|e| s.path(e)).unwrap_or_default())
                                    on:mouseenter=move |_| contact.update(|c| c.set_pointer_edge(id_enter.clone()))
                                    on:mouseleave=move |_| contact.update(|c| c.clear_pointer_edge(&id_leave))
                                />
                                <path
                                    class="edge-casing"
                                    aria-hidden="true"
                                    d=move || state.with(|s| s.edges.get(&id_casing).map(|e| s.path(e)).unwrap_or_default())
                                    stroke-width=move || edge.get().map_or(0.0, |e| {
                                        if contact.with(|c| c.hot_edge(&e)) {
                                            edge_width(e.kind, e.weight) + 3.0
                                        } else {
                                            0.0
                                        }
                                    })
                                />
                                <path
                                    class=move || {
                                        let e = edge.get();
                                        let kind = e.as_ref().map_or("", |e| e.kind);
                                        let hot = e.as_ref().is_some_and(|e| contact.with(|c| c.hot_edge(e)));
                                        format!("edge {kind}{}", if hot { " hot" } else { "" })
                                    }
                                    d=move || state.with(|s| s.edges.get(&id_class).map(|e| s.path(e)).unwrap_or_default())
                                    stroke-width=move || edge.get().map_or(1.0, |e| edge_width(e.kind, e.weight))
                                    stroke-opacity=move || {
                                        edge.get().map_or(0.0, |e| {
                                            let hot = contact.with(|c| c.hot_edge(&e));
                                            let dim = !selected.get().is_empty()
                                                && !lit.with(|l| l.contains(&e.from) && l.contains(&e.to))
                                                && !hot;
                                            edge_render_opacity(&e, hot, dim)
                                        })
                                    }
                                />
                            </g>
                        }
                    }
                />
                <For
                    each=move || state.with(|s| s.node_ids.clone())
                    key=|id| id.clone()
                    children=move |id: String| {
                        let id_node = id.clone();
                        let node = Signal::derive(move || state.with(|s| s.nodes.get(&id_node).cloned()));
                        let pick = id.clone();
                        let id_sel = id.clone();
                        let id_hot = id.clone();
                        let id_near = id.clone();
                        let id_endpoint = id.clone();
                        let id_enter = id.clone();
                        let id_leave = id.clone();
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
                                    let hot = contact.with(|c| c.direct_node(&id_hot));
                                    let near = contact_nodes.with(|s| s.contains(&id_near));
                                    let endpoint = contact.with(|c| {
                                        state.with(|s| c.endpoints(s).contains(&id_endpoint))
                                    });
                                    let always = n.as_ref().is_some_and(|n| {
                                        matches!(n.kind, "term" | "qualifier" | "source")
                                            || n.hit.is_some_and(|h| h < 3)
                                    });
                                    format!("node {kind}{}{}{}{}{}",
                                        if hot { " hot" } else { "" },
                                        if near { " near" } else { "" },
                                        if endpoint { " endpoint" } else { "" },
                                        if is_sel.get() { " selected" } else { "" },
                                        if always { " always" } else { "" })
                                }
                                style=move || node.get().map(|n| {
                                    let near = contact_nodes.with(|s| s.contains(&n.id));
                                    let dim = !selected.get().is_empty()
                                        && !lit.with(|l| l.contains(&n.id))
                                        && !near;
                                    format!("transform: translate({:.1}px, {:.1}px); opacity: {:.3}",
                                        n.x, n.y, if near { 1.0 } else if dim { 0.25 } else { 1.0 })
                                }).unwrap_or_default()
                                on:mouseenter=move |_| contact.update(|c| c.set_pointer_node(id_enter.clone()))
                                on:mouseleave=move |_| contact.update(|c| c.clear_pointer_node(&id_leave))
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

/// Forme d'un nœud (charte, section 4) et son étiquette. La visibilité de l'étiquette est
/// pilotée par les classes CSS (`always`, `selected`, `hot`).
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
    let label: String = n.label.chars().take(28).collect();
    view! {
        {selected.then(|| view! { <circle class="halo" r=r + 7.0 /> })}
        {body}
        {(!label.is_empty()).then(|| view! { <text class="label" y=r + 15.0>{label}</text> })}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view_node(id: &str, kind: &'static str) -> NodeView {
        NodeView {
            id: id.into(),
            kind,
            label: id.into(),
            x: 0.0,
            y: 0.0,
            r: 10.0,
            hit: None,
        }
    }

    fn view_edge(id: &str, from: &str, to: &str, kind: &'static str, weight: f32) -> EdgeView {
        EdgeView {
            id: id.into(),
            from: from.into(),
            to: to.into(),
            kind,
            weight,
        }
    }

    fn small_state() -> MapState {
        let mut s = MapState::default();
        for n in [
            view_node("q", "query"),
            view_node("t0", "term"),
            view_node("d1", "prose"),
            view_node("d2", "related"),
            view_node("s0", "source"),
        ] {
            s.node_ids.push(n.id.clone());
            s.nodes.insert(n.id.clone(), n);
        }
        for e in [
            view_edge("q>t0:query_term", "q", "t0", "query_term", 1.0),
            view_edge("t0>d1:term_hit", "t0", "d1", "term_hit", 0.8),
            view_edge("d1>d2:similar", "d1", "d2", "similar", 0.5),
            view_edge("d1>s0:hit_source", "d1", "s0", "hit_source", 1.0),
        ] {
            s.edge_ids.push(e.id.clone());
            s.edges.insert(e.id.clone(), e);
        }
        s
    }

    #[test]
    fn contact_priority_and_switching() {
        let state = small_state();
        let mut c = Contact::default();
        c.set_pointer_node("t0");
        c.set_focus_edge("d1>d2:similar");
        assert_eq!(c.node(), "t0");
        assert_eq!(c.edge(), "");
        assert_eq!(
            c.nodes(&state),
            ["t0", "d1", "q"].into_iter().map(String::from).collect()
        );

        c.set_pointer_edge("t0>d1:term_hit");
        assert_eq!(c.edge(), "t0>d1:term_hit");
        assert_eq!(c.node(), "");
        c.clear_pointer();
        assert_eq!(
            c.edge(),
            "d1>d2:similar",
            "le focus reprend après le pointeur"
        );
        assert_eq!(c.node(), "");

        c.set_pointer_node("d1");
        assert_eq!(c.edge(), "");
        assert_eq!(
            c.nodes(&state),
            ["d1", "t0", "d2", "s0"]
                .into_iter()
                .map(String::from)
                .collect()
        );
        assert!(c.endpoints(&state).is_empty());
    }

    #[test]
    fn edge_contact_endpoints_and_hotness() {
        let state = small_state();
        let edge = state.edges["t0>d1:term_hit"].clone();
        let mut c = Contact::default();
        c.set_pointer_edge(edge.id.clone());
        assert!(c.hot_edge(&edge));
        assert_eq!(c.endpoints(&state).len(), 2);
        assert!(c.endpoints(&state).contains("t0"));
        assert!(c.endpoints(&state).contains("d1"));
        let link = state
            .links("d1")
            .into_iter()
            .find(|l| l.edge_id == edge.id)
            .unwrap();
        assert!(c.hot_link(&link));
        c.set_pointer_node("t0");
        assert!(c.hot_edge(&edge), "le nœud allume ses liens");
        assert!(!c.hot_edge(&state.edges["d1>d2:similar"]));
    }

    #[test]
    fn prune_drops_vanished_ids() {
        let mut state = small_state();
        let mut c = Contact::default();
        c.set_pointer_edge("t0>d1:term_hit");
        c.set_focus_node("d2");
        state.edges.remove("t0>d1:term_hit");
        state.nodes.remove("d2");
        c.prune(&state);
        assert!(c.pointer_edge.is_empty());
        assert!(c.focus_node.is_empty());
    }

    #[test]
    fn weights_map_to_width_and_opacity() {
        assert_eq!(edge_width("term_hit", 0.0), 0.5);
        assert_eq!(edge_width("term_hit", 1.0), 4.0);
        assert_eq!(edge_width("query_term", 0.0), 1.2);
        assert_eq!(edge_width("hit_source", 0.0), 1.0);
        assert!((edge_opacity("term_hit", 0.0) - 0.30).abs() < 1e-6);
        assert!((edge_opacity("term_hit", 1.0) - 1.0).abs() < 1e-6);
        assert!((edge_opacity("qualifies", 0.5) - 0.65).abs() < 1e-6);
        assert!((edge_opacity("similar", 0.5) - 0.65).abs() < 1e-6);
        assert!((edge_opacity("query_term", 0.0) - 0.70).abs() < 1e-6);
        assert!((edge_opacity("hit_source", 0.0) - 0.30).abs() < 1e-6);

        let e = view_edge("x", "a", "b", "term_hit", 0.5);
        assert!((edge_render_opacity(&e, true, false) - 0.95).abs() < 1e-6);
        assert!((edge_render_opacity(&e, false, true) - 0.1625).abs() < 1e-6);
    }

    #[test]
    fn hidden_term_edges_are_not_in_the_map() {
        let hidden = view_edge("t0>d1:term_hit", "t0", "d1", "term_hit", 0.05);
        assert!(edge_hidden(hidden.kind, hidden.weight));
        assert!(!edge_hidden("term_hit", HIDDEN_TERM_WEIGHT));
        assert!(!edge_hidden("similar", 0.05));

        use nknown_api::{Edge, EdgeKind, Graph, Node, NodeKind};
        let node = |id: &str, kind| Node {
            id: id.into(),
            kind,
            label: id.into(),
            weight: 1.0,
            hit: None,
            doc: None,
            source_kind: None,
        };
        let edge = |from: &str, to: &str, kind, weight| Edge {
            from: from.into(),
            to: to.into(),
            kind,
            weight,
        };
        let graph = Graph {
            nodes: vec![
                node("q", NodeKind::Query),
                node("t0", NodeKind::Term),
                node("d1", NodeKind::Prose),
            ],
            edges: vec![
                edge("q", "t0", EdgeKind::QueryTerm, 1.0),
                edge("t0", "d1", EdgeKind::TermHit, 0.05),
            ],
        };
        let state = MapState::from_graph(&graph);
        assert!(!state.edges.contains_key("t0>d1:term_hit"));
        assert_eq!(state.edge_ids, ["q>t0:query_term"]);
    }
}
