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
    /// Identifiant du document (en texte : le QML n'a que des nombres flottants).
    pub doc: String,
    /// Passage trouvé (le lecteur s'y place).
    pub passage: String,
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
    /// Fichier local : chemin complet, et sa fin pour l'affichage (`…/src/lib.rs`).
    pub path: Option<String>,
    pub location: Option<String>,
}

/// Fin d'un chemin : les `n` derniers éléments, précédés de `…/` s'il en manque.
pub fn path_tail(path: &str, n: usize) -> String {
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let tail = parts[parts.len().saturating_sub(n)..].join("/");
    if parts.len() > n {
        format!("…/{tail}")
    } else {
        format!("/{tail}")
    }
}

pub fn hits(resp: &api::SearchResponse) -> Vec<HitView> {
    resp.hits
        .iter()
        .map(|h| HitView {
            node: format!("d{}", h.doc),
            doc: h.doc.to_string(),
            passage: h.passage.to_string(),
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
            path: h.path.clone(),
            location: h.path.as_deref().map(|p| path_tail(p, 3)),
        })
        .collect()
}

/// Fichiers lus en entier par le lecteur jusqu'à cette taille.
const FULL_FILE_MAX: u64 = 2 << 20;

/// Contenu d'un fichier local, s'il est lisible, en texte et pas trop gros.
pub fn read_file(path: &str) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > FULL_FILE_MAX {
        return None;
    }
    String::from_utf8(std::fs::read(path).ok()?).ok()
}

/// Document pour le lecteur. Un fichier encore lisible est montré tel qu'il est sur le disque
/// (les passages indexés le découpent) : `full`, avec `focus`, la position (en unités UTF-16,
/// celles de Qt) du passage trouvé.
pub fn document(
    d: &api::DocumentView,
    passage: &str,
    read: impl Fn(&str) -> Option<String>,
) -> serde_json::Value {
    let is_code = d.passages.iter().filter(|p| p.is_code).count() * 2 > d.passages.len();
    let full = d.path.as_deref().and_then(read);
    let focus = full.as_deref().and_then(|text| {
        let p = d.passages.iter().find(|p| p.id.to_string() == passage)?;
        // Première ligne parlante du passage, retrouvée dans le fichier.
        let needle = p
            .text
            .lines()
            .map(str::trim)
            .find(|l| l.chars().count() >= 12)?;
        let at = text.find(needle)?;
        Some(text[..at].encode_utf16().count())
    });
    serde_json::json!({
        "doc": d.id.to_string(),
        "title": d.title.clone().unwrap_or_else(|| d.source.clone()),
        "source": d.source,
        "source_kind": format!("{:?}", d.source_kind).to_lowercase(),
        "url": d.url,
        "path": d.path,
        "location": d.path.as_deref().map(|p| path_tail(p, 3)),
        "date": d.date,
        "lang": d.lang,
        "is_code": is_code,
        "code_lang": d.passages.iter().find_map(|p| p.code_lang.clone()),
        "full": full,
        "focus": focus,
        "passages": d.passages.iter().map(|p| serde_json::json!({
            "id": p.id.to_string(),
            "text": p.text,
            "is_code": p.is_code,
            "code_lang": p.code_lang,
        })).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn path_tails() {
        use super::path_tail;
        assert_eq!(path_tail("/home/a/dev/x/src/lib.rs", 3), "…/x/src/lib.rs");
        assert_eq!(path_tail("/a/b.txt", 3), "/a/b.txt");
    }

    #[test]
    fn full_file_and_focus() {
        use nknown_api as api;
        let d = api::DocumentView {
            id: 7,
            title: Some("a.rs".into()),
            source: "x".into(),
            source_kind: api::SourceKind::File,
            url: None,
            path: Some("/x/a.rs".into()),
            date: 0,
            lang: "fra".into(),
            nature: api::Nature::Reference,
            passages: vec![api::PassageView {
                id: 3,
                text: "fn deuxième_fonction() {".into(),
                is_code: true,
                code_lang: Some("rust".into()),
            }],
        };
        let file = "// é\nfn deuxième_fonction() {}\n";
        let j = super::document(&d, "3", |_| Some(file.into()));
        assert_eq!(j["full"], file);
        assert_eq!(j["focus"], 5); // « // é\n » : 5 unités UTF-16
        assert_eq!(j["is_code"], true);
        let j = super::document(&d, "3", |_| None);
        assert!(j["full"].is_null() && j["focus"].is_null());
    }
}
