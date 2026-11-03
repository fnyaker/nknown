//! Ce que reçoit le QML : la carte disposée (positions calculées ici, l'interface n'a plus qu'à
//! animer) et la liste des résultats.

use nknown_api::{self as api, EdgeKind, Graph, NodeKind};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct MapView {
    pub nodes: Vec<NodeView>,
    pub edges: Vec<EdgeView>,
}

#[derive(Debug, Serialize)]
pub struct NodeView {
    pub id: String,
    pub kind: &'static str,
    pub label: String,
    pub weight: f32,
    pub x: f32,
    pub y: f32,
    pub r: f32,
    /// Index dans la liste des résultats définitifs.
    pub hit: i32,
    /// Lien du document (ouvrir), s'il en a un.
    pub doc: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct EdgeView {
    pub id: String,
    pub from: String,
    pub to: String,
    pub kind: &'static str,
    pub weight: f32,
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
}

pub fn node_kind(k: NodeKind) -> &'static str {
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

fn edge_kind(k: EdgeKind) -> &'static str {
    match k {
        EdgeKind::QueryTerm => "query_term",
        EdgeKind::TermHit => "term_hit",
        EdgeKind::Qualifies => "qualifies",
        EdgeKind::HitSource => "hit_source",
        EdgeKind::Similar => "similar",
    }
}

pub fn map(graph: &Graph) -> MapView {
    let placed = nknown_graph::layout(graph);
    let at = |id: &str| placed.get(id).map_or((0.0, 0.0), |p| (p.x, p.y));
    MapView {
        nodes: graph
            .nodes
            .iter()
            .map(|n| {
                let p = placed[&n.id];
                NodeView {
                    id: n.id.clone(),
                    kind: node_kind(n.kind),
                    label: n.label.clone(),
                    weight: n.weight,
                    x: p.x,
                    y: p.y,
                    r: p.radius,
                    hit: n.hit.map_or(-1, |h| h as i32),
                    doc: n.doc,
                }
            })
            .collect(),
        edges: graph
            .edges
            .iter()
            .map(|e| {
                let ((x1, y1), (x2, y2)) = (at(&e.from), at(&e.to));
                EdgeView {
                    id: format!("{}>{}:{}", e.from, e.to, edge_kind(e.kind)),
                    from: e.from.clone(),
                    to: e.to.clone(),
                    kind: edge_kind(e.kind),
                    weight: e.weight,
                    x1,
                    y1,
                    x2,
                    y2,
                }
            })
            .collect(),
    }
}

#[derive(Debug, Serialize)]
pub struct HitView {
    /// Nœud de la carte (`d<document>`).
    pub node: String,
    pub title: String,
    pub source: String,
    pub source_kind: String,
    pub date: i64,
    pub relative: f32,
    pub snippet: String,
    pub matched: Vec<String>,
    pub is_code: bool,
    pub code_lang: Option<String>,
    pub url: Option<String>,
}

pub fn hits(resp: &api::SearchResponse) -> Vec<HitView> {
    resp.hits
        .iter()
        .map(|h| HitView {
            node: format!("d{}", h.doc),
            title: h.title.clone().unwrap_or_else(|| h.source.clone()),
            source: h.source.clone(),
            source_kind: format!("{:?}", h.source_kind).to_lowercase(),
            date: h.date,
            relative: h.relative,
            snippet: h.snippet.clone(),
            matched: h.matched.clone(),
            is_code: h.is_code,
            code_lang: h.code_lang.clone(),
            url: h.url.clone(),
        })
        .collect()
}
