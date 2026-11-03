//! Types de l'API REST de NKnown, version 1.
//!
//! Ce crate ne dépend pas du cœur : il compile en WASM pour l'interface web. Toute donnée
//! sensible (texte d'une requête, chemin, jeton) voyage dans un corps ou un en-tête, jamais
//! dans une URL ; les chemins ne portent que des identifiants opaques.

use serde::{Deserialize, Serialize};

/// Préfixe de toutes les routes.
pub const PREFIX: &str = "/api/v1";
/// Type MIME du flux d'évènements de `POST /search`.
pub const NDJSON: &str = "application/x-ndjson";
/// Nom du cookie de session de l'interface web.
pub const SESSION_COOKIE: &str = "nknown_session";

pub mod routes {
    pub const STATUS: &str = "/status";
    pub const SEARCH: &str = "/search";
    pub const DOCUMENTS: &str = "/documents";
    pub const SOURCES: &str = "/sources";
    pub const SESSION: &str = "/session";
    pub const SETTINGS: &str = "/settings";
    pub const CATALOG: &str = "/catalog";
}

// ---------------------------------------------------------------- recherche

/// `POST /search`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    20
}

/// Étapes d'une recherche, dans l'ordre. L'interface remplit sa jauge par étape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Analyse de la requête : mots, lectures, intention.
    Analyze,
    /// Recherche dans l'index et calcul des scores.
    Search,
    /// Construction du graphe des résultats.
    Graph,
}

impl Phase {
    pub const ALL: [Phase; 3] = [Phase::Analyze, Phase::Search, Phase::Graph];

    /// Rang de l'étape, de 0 à `ALL.len() - 1`.
    pub fn index(self) -> usize {
        self as usize
    }
}

/// Une ligne du flux NDJSON de `POST /search`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SearchEvent {
    /// Début d'une étape.
    Phase {
        phase: Phase,
    },
    /// Requête analysée : les mots peuvent apparaître dans la carte avant les résultats.
    Terms {
        intent: Option<String>,
        terms: Vec<QueryTermView>,
    },
    /// Modification de la carte : elle se construit en direct, patch après patch (requête et
    /// mots, puis résultats provisoires, puis carte finale). Appliquer les patchs dans l'ordre
    /// donne la carte de [`SearchResponse::graph`].
    GraphPatch(GraphPatch),
    /// Résultat final.
    Done(Box<SearchResponse>),
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryTermView {
    pub text: String,
    /// Identifiant exact cherché dans le code (`read_to_string`) plutôt qu'un mot.
    pub code: bool,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SearchResponse {
    pub query: String,
    pub intent: Option<String>,
    pub terms: Vec<QueryTermView>,
    /// Passages examinés avant les seuils.
    pub candidates: usize,
    pub took_ms: f64,
    pub hits: Vec<Hit>,
    pub graph: Graph,
}

/// Un résultat : un document, représenté par son meilleur passage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hit {
    pub doc: u64,
    pub passage: u64,
    pub title: Option<String>,
    pub source: String,
    pub source_kind: SourceKind,
    pub url: Option<String>,
    /// Date de publication (secondes Unix).
    pub date: i64,
    pub score: f64,
    /// Score rapporté au meilleur résultat, dans `]0, 1]`.
    pub relative: f32,
    pub snippet: String,
    /// Mots de la requête trouvés dans le passage (pour les surligner).
    pub matched: Vec<String>,
    pub is_code: bool,
    pub code_lang: Option<String>,
    pub nature: Nature,
    pub parts: ScoreParts,
}

/// Facteurs du score, dont le produit donne `Hit::score`.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct ScoreParts {
    pub text: f64,
    pub coverage: f64,
    pub qualified: f64,
    pub title: f64,
    pub nature: f64,
    pub freshness: f64,
    pub burst: f64,
    pub graph: f64,
    pub authority: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Feed,
    Encyclopedia,
    Web,
    File,
    Other,
}

/// Nature dominante d'un document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Nature {
    Event,
    Reference,
    Opinion,
    Tutorial,
    Personal,
    Discussion,
    Technical,
}

// ---------------------------------------------------------------- graphe

/// Carte d'une recherche : les chemins de la requête vers ses résultats.
/// Sémantique visuelle : `docs/ui-guidelines.md`, section 4.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    /// Clé stable pour une même requête (`q`, `t0`, `a0-1`, `d42`, `s…`).
    pub id: String,
    pub kind: NodeKind,
    pub label: String,
    /// Grandeur codée par la taille, normalisée dans `[0, 1]` (voir `NodeKind`).
    pub weight: f32,
    /// Indice dans `SearchResponse::hits` pour un résultat.
    pub hit: Option<usize>,
    /// Document cible pour un résultat ou un document apparenté.
    pub doc: Option<u64>,
    pub source_kind: Option<SourceKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    /// Centre de la carte.
    Query,
    /// Mot de la requête ; poids : part moyenne du mot dans les scores.
    Term,
    /// Adjectif de la requête qui qualifie un autre mot.
    Qualifier,
    /// Résultat texte ; poids : score relatif.
    Prose,
    /// Résultat code ; poids : score relatif.
    Code,
    /// Source ; poids : part des résultats qui en viennent.
    Source,
    /// Document apparenté à un résultat ; poids : similarité.
    Related,
}

/// Différence entre deux états de la carte.
///
/// Un nœud ou un lien d'identité connue (`id` ; `from`, `to`, `kind`) remplace l'ancien : poids,
/// libellé ou nature ont changé. Retirer un nœud retire ses liens. Dans une carte provisoire,
/// `Node::hit` est absent : il n'indexe que les résultats définitifs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GraphPatch {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<Node>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<Edge>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed_nodes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed_edges: Vec<EdgeKey>,
}

impl GraphPatch {
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
            && self.edges.is_empty()
            && self.removed_nodes.is_empty()
            && self.removed_edges.is_empty()
    }
}

/// Identité d'un lien.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EdgeKey {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
}

impl Graph {
    /// Applique un patch (côté client).
    pub fn apply(&mut self, patch: &GraphPatch) {
        let gone = |id: &String| patch.removed_nodes.contains(id);
        self.nodes.retain(|n| !gone(&n.id));
        self.edges
            .retain(|e| !gone(&e.from) && !gone(&e.to) && !patch.removed_edges.contains(&e.key()));
        for n in &patch.nodes {
            match self.nodes.iter_mut().find(|o| o.id == n.id) {
                Some(o) => *o = n.clone(),
                None => self.nodes.push(n.clone()),
            }
        }
        for e in &patch.edges {
            let key = e.key();
            match self.edges.iter_mut().find(|o| o.key() == key) {
                Some(o) => *o = e.clone(),
                None => self.edges.push(e.clone()),
            }
        }
    }
}

impl Edge {
    pub fn key(&self) -> EdgeKey {
        EdgeKey {
            from: self.from.clone(),
            to: self.to.clone(),
            kind: self.kind,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
    /// Grandeur codée par l'épaisseur et l'intensité, dans `[0, 1]`.
    pub weight: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// Requête → mot.
    QueryTerm,
    /// Mot → résultat ; poids : part du mot dans le score textuel du résultat.
    TermHit,
    /// Qualifiant → mot qualifié ; poids 1 si la qualification est dans le résultat.
    Qualifies,
    /// Résultat → source.
    HitSource,
    /// Résultat → document apparenté ; poids : similarité.
    Similar,
}

// ---------------------------------------------------------------- documents, sources, état

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentView {
    pub id: u64,
    pub title: Option<String>,
    pub source: String,
    pub source_kind: SourceKind,
    pub url: Option<String>,
    pub date: i64,
    pub lang: String,
    pub nature: Nature,
    pub passages: Vec<PassageView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PassageView {
    pub id: u64,
    pub text: String,
    pub is_code: bool,
    pub code_lang: Option<String>,
}

/// `POST /documents` : ajout direct d'un texte.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AddDocument {
    pub text: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    /// `markdown`, `plain`, ou un langage de programmation (`rust`, `python`…).
    #[serde(default)]
    pub format: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AddOutcome {
    Added,
    Updated,
    Unchanged,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Added {
    pub id: u64,
    pub outcome: AddOutcome,
}

/// `POST /sources`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AddSource {
    /// Dossier, `rss:<url>`, `wiki:<édition>:<Titre>|<Titre>`, `wiki-search:…`, `wiki-cat:…`.
    pub spec: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceView {
    /// Identifiant opaque (empreinte de `spec`), utilisable dans une URL.
    pub id: String,
    pub spec: String,
    pub kind: SourceKind,
    pub state: SourceState,
    /// Dernière collecte réussie (secondes Unix).
    pub last_run: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SourceState {
    Idle,
    Collecting { done: usize },
    Failed { message: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Status {
    pub version: String,
    pub docs: u64,
    pub passages: u64,
    pub words: u64,
    /// Langues analysées avec lexique.
    pub langs: Vec<String>,
    /// Analyseur neuronal chargé.
    pub parser: bool,
    /// Où il calcule : `gpu` (Vulkan) ou `cpu` ; absent sans analyseur.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parser_device: Option<ComputeDevice>,
    /// Une collecte est en cours.
    pub collecting: bool,
    /// Modèles (lexiques, analyseur) : prêts, ou en cours de téléchargement.
    pub models: ModelsState,
}

/// Catalogue de sources proposées à l'import.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    /// (clé, libellé), dans l'ordre d'affichage.
    pub themes: Vec<(String, String)>,
    pub entries: Vec<CatalogEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogEntry {
    pub name: String,
    pub spec: String,
    pub kind: SourceKind,
    pub lang: String,
    pub theme: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Déjà parmi les sources configurées.
    pub added: bool,
}

/// Réglages d'analyse du moteur (`GET`/`PUT /settings`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineSettings {
    /// Analyseur syntaxique (adjectifs mieux reliés ; coûteux à l'indexation).
    pub parser: bool,
    pub precision: ParserPrecision,
    pub device: DeviceChoice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParserPrecision {
    /// Table des mots en 4 bits, couches en 8 bits : précision intacte.
    #[default]
    Standard,
    /// Tout en 4 bits : plus léger, un point d'exactitude en moins (sur CPU).
    Light,
}

/// Où calculer l'analyse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceChoice {
    /// GPU (Vulkan) s'il répond, sinon CPU.
    #[default]
    Auto,
    Gpu,
    Cpu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComputeDevice {
    Gpu,
    Cpu,
}

/// État des modèles : téléchargés au premier lancement, rechargés quand ils sont prêts.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ModelsState {
    Ready,
    #[default]
    Missing,
    /// Octets reçus sur le total à télécharger.
    Downloading {
        done: u64,
        total: u64,
    },
    /// Échec (hors ligne…) : nouvel essai plus tard ; le moteur fonctionne sans lemmes.
    Failed {
        message: String,
    },
}

/// `POST /session` (interface web) : le jeton s'échange contre un cookie `HttpOnly`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionRequest {
    pub token: String,
}

/// Corps de toute réponse d'erreur.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiError {
    pub error: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_tagged() {
        let e = SearchEvent::Phase {
            phase: Phase::Search,
        };
        assert_eq!(
            serde_json::to_string(&e).unwrap(),
            r#"{"type":"phase","phase":"search"}"#
        );
        let done = SearchEvent::Done(Box::default());
        let back: SearchEvent =
            serde_json::from_str(&serde_json::to_string(&done).unwrap()).unwrap();
        assert_eq!(back, done);
    }

    #[test]
    fn limit_has_a_default() {
        let r: SearchRequest = serde_json::from_str(r#"{"query":"actu paris"}"#).unwrap();
        assert_eq!(r.limit, 20);
    }
}
