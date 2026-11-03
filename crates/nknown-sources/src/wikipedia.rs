//! Source : articles de Wikipédia, via l'API MediaWiki (texte brut des articles).
//!
//! Sélection par titres, par recherche ou par catégorie. Chaque article est daté par sa
//! dernière révision. Les titres de section deviennent des coupures de paragraphe, et les
//! sections sans contenu propre (références, liens externes...) sont retirées.

use nknown_core::item::RawItem;
use nknown_core::lang::Lang;
use nknown_core::source::{Capabilities, ItemStream, Source, SourceError, SourceId, SourceKind};
use nknown_core::time::{DateConfidence, Dated, Timestamp};
use serde_json::Value;

use crate::http::{Http, encode_component};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    Titles(Vec<String>),
    /// Recherche plein texte de Wikipédia ; les `limit` premiers articles.
    Search {
        query: String,
        limit: usize,
    },
    /// Articles d'une catégorie (sans le préfixe « Catégorie: »).
    Category {
        name: String,
        limit: usize,
    },
    /// Les `limit` articles les plus lus (pages vues des derniers mois, API Wikimedia).
    TopRead {
        limit: usize,
    },
}

/// Espaces de noms (préfixes) des pages qui ne sont pas des articles.
const NOT_ARTICLES: &[&str] = &[
    "special:",
    "spécial:",
    "wikipedia:",
    "wikipédia:",
    "file:",
    "fichier:",
    "portal:",
    "portail:",
    "category:",
    "catégorie:",
    "help:",
    "aide:",
    "template:",
    "modèle:",
    "user:",
    "utilisateur:",
    "talk:",
    "discussion:",
    "служебная:",
    "википедия:",
    "портал:",
    "файл:",
    "категория:",
    "special:",
    "特殊:",
    "wikipedia:",
    "portal:",
    "file:",
    "project:",
];

/// Pages vues : liste mensuelle des 1000 plus lues → total par article sur les mois parcourus.
pub fn add_top_month(json: &Value, totals: &mut std::collections::HashMap<String, u64>) {
    let articles = json["items"][0]["articles"]
        .as_array()
        .into_iter()
        .flatten();
    for a in articles {
        let (Some(title), Some(views)) = (a["article"].as_str(), a["views"].as_u64()) else {
            continue;
        };
        let lower = title.to_lowercase();
        if title == "-"
            || lower.contains("main_page")
            || lower.contains("accueil_principal")
            || NOT_ARTICLES.iter().any(|p| lower.starts_with(p))
        {
            continue;
        }
        *totals.entry(title.replace('_', " ")).or_default() += views;
    }
}

#[derive(Debug)]
pub struct WikipediaSource {
    id: SourceId,
    /// Code de l'édition : `fr`, `en`...
    edition: String,
    selection: Selection,
    http: Http,
}

impl WikipediaSource {
    pub fn new(edition: impl Into<String>, selection: Selection) -> Self {
        let edition = edition.into();
        Self {
            id: SourceId(format!("wikipedia:{edition}")),
            edition,
            // Politesse envers l'API Wikimedia : une requête à la fois, une par seconde au plus
            // (cinq pour un palier, qui compte des milliers d'articles).
            http: Http::with_interval(match selection {
                Selection::TopRead { .. } => std::time::Duration::from_millis(200),
                _ => std::time::Duration::from_secs(1),
            }),
            selection,
        }
    }

    fn api(&self, params: &str) -> String {
        format!(
            "https://{}.wikipedia.org/w/api.php?format=json&formatversion=2&{params}",
            self.edition
        )
    }

    /// Les plus lus : listes mensuelles (1000 articles chacune) en remontant jusqu'à trois ans,
    /// cumulées, jusqu'à en avoir assez.
    fn top_read(&self, limit: usize) -> Result<Vec<String>, SourceError> {
        let mut totals = std::collections::HashMap::new();
        let (mut year, mut month, _) = Timestamp::now().ymd();
        for _ in 0..36 {
            // Mois précédent (le mois courant n'est pas encore publié).
            if month == 1 {
                year -= 1;
                month = 12;
            } else {
                month -= 1;
            }
            let url = format!(
                "https://wikimedia.org/api/rest_v1/metrics/pageviews/top/{}.wikipedia/all-access/{year}/{month:02}/all-days",
                self.edition
            );
            match self.http.get_json(&url) {
                Ok(json) => add_top_month(&json, &mut totals),
                Err(e) if totals.is_empty() => return Err(e),
                Err(_) => break,
            }
            if totals.len() >= limit + limit / 10 {
                break;
            }
        }
        let mut ranked: Vec<(String, u64)> = totals.into_iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        Ok(ranked.into_iter().take(limit).map(|(t, _)| t).collect())
    }

    fn titles(&self) -> Result<Vec<String>, SourceError> {
        let (url, list) = match &self.selection {
            Selection::Titles(t) => return Ok(t.clone()),
            Selection::TopRead { limit } => return self.top_read(*limit),
            Selection::Search { query, limit } => (
                self.api(&format!(
                    "action=query&list=search&srnamespace=0&srlimit={}&srsearch={}",
                    limit.clamp(&1, &50),
                    encode_component(query)
                )),
                "search",
            ),
            Selection::Category { name, limit } => (
                self.api(&format!(
                    "action=query&list=categorymembers&cmnamespace=0&cmlimit={}&cmtitle={}",
                    limit.clamp(&1, &500),
                    encode_component(&format!("Category:{name}"))
                )),
                "categorymembers",
            ),
        };
        let json = self.http.get_json(&url)?;
        Ok(json["query"][list]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|p| p["title"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default())
    }

    fn article(
        &self,
        title: &str,
        since: Option<Timestamp>,
        fetched_at: Timestamp,
    ) -> Result<Option<RawItem>, SourceError> {
        let url = self.api(&format!(
            "action=query&redirects=1&prop=extracts|revisions|info&explaintext=1\
             &exsectionformat=wiki&rvprop=timestamp&inprop=url&titles={}",
            encode_component(title)
        ));
        let json = self.http.get_json(&url)?;
        article_from_json(&self.id, &self.edition, &json, since, fetched_at)
    }
}

impl Source for WikipediaSource {
    fn id(&self) -> &SourceId {
        &self.id
    }

    fn kind(&self) -> SourceKind {
        SourceKind::Encyclopedia
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            published_at: true,
            lang: true,
            incremental: true,
        }
    }

    fn checkpoint_key(&self) -> String {
        let sel = match &self.selection {
            Selection::Titles(t) => format!("titres:{}", t.join("|")),
            Selection::Search { query, limit } => format!("recherche:{query}:{limit}"),
            Selection::Category { name, limit } => format!("catégorie:{name}:{limit}"),
            Selection::TopRead { limit } => format!("plus-lus:{limit}"),
        };
        format!("{}:{sel}", self.id.0)
    }

    fn fetch_new(
        &mut self,
        since: Option<Timestamp>,
        known: &dyn Fn(&str) -> Option<Timestamp>,
    ) -> Result<ItemStream<'_>, SourceError> {
        // Un palier se met à jour chaque semaine (des milliers d'articles à revérifier).
        if let (Selection::TopRead { .. }, Some(since)) = (&self.selection, since)
            && Timestamp::now().0 - since.0 < 5 * 86_400
        {
            return Ok(Box::new(std::iter::empty()));
        }
        // Dates de révision par lots de 50 : on ne télécharge que les articles
        // inconnus ou révisés depuis la version indexée.
        let mut wanted = Vec::new();
        for chunk in self.titles()?.chunks(50) {
            let url = self.api(&format!(
                "action=query&redirects=1&prop=revisions&rvprop=timestamp&titles={}",
                encode_component(&chunk.join("|"))
            ));
            let json = self.http.get_json(&url)?;
            for page in json["query"]["pages"].as_array().into_iter().flatten() {
                let (Some(title), None) = (page["title"].as_str(), page.get("missing")) else {
                    continue;
                };
                let revised = page["revisions"][0]["timestamp"]
                    .as_str()
                    .and_then(Timestamp::parse_rfc3339);
                let indexed = known(&format!("{}:{title}", self.edition));
                let changed = match (indexed, revised) {
                    (Some(i), Some(r)) => r > i,
                    _ => true,
                };
                if changed {
                    wanted.push(title.to_owned());
                }
            }
        }
        let fetched_at = Timestamp::now();
        let light = matches!(self.selection, Selection::TopRead { .. });
        Ok(Box::new(wanted.into_iter().filter_map(move |t| {
            self.article(&t, None, fetched_at).transpose().map(|r| {
                r.map(|mut item| {
                    if light {
                        // Gros volume : analyse légère (sans l'analyseur neuronal).
                        item.meta.push(("analysis".into(), "light".into()));
                    }
                    item
                })
            })
        })))
    }

    fn fetch(&mut self, since: Option<Timestamp>) -> Result<ItemStream<'_>, SourceError> {
        let titles = self.titles()?;
        let fetched_at = Timestamp::now();
        // Une requête par article (l'API ne renvoie qu'un texte complet à la fois).
        Ok(Box::new(titles.into_iter().filter_map(move |t| {
            self.article(&t, since, fetched_at).transpose()
        })))
    }
}

/// Réponse `prop=extracts|revisions|info` → contenu ; `None` si absent ou pas plus récent.
pub fn article_from_json(
    id: &SourceId,
    edition: &str,
    json: &Value,
    since: Option<Timestamp>,
    fetched_at: Timestamp,
) -> Result<Option<RawItem>, SourceError> {
    let Some(page) = json["query"]["pages"].get(0) else {
        return Err(SourceError::Parse("réponse sans page".into()));
    };
    if page.get("missing").is_some() {
        return Ok(None);
    }
    let revised = page["revisions"][0]["timestamp"]
        .as_str()
        .and_then(Timestamp::parse_rfc3339);
    if let (Some(since), Some(r)) = (since, revised)
        && r <= since
    {
        return Ok(None);
    }
    let text = clean_extract(page["extract"].as_str().unwrap_or_default());
    if text.is_empty() {
        return Ok(None);
    }
    let title = page["title"].as_str().unwrap_or_default().to_owned();
    let mut item = RawItem::new(id.clone(), text, fetched_at);
    item.external_id = Some(format!("{edition}:{title}"));
    item.url = page["fullurl"].as_str().map(str::to_owned);
    item.title = Some(title);
    item.published_at = revised.map(|at| Dated {
        at,
        confidence: DateConfidence::Declared,
    });
    item.lang_hint = Lang::from_tag(edition);
    item.meta.push(("date".into(), "dernière révision".into()));
    Ok(Some(item))
}

/// Sections retirées : listes de renvois, sans texte propre.
const SKIPPED_SECTIONS: &[&str] = &[
    "notes et références",
    "notes",
    "références",
    "bibliographie",
    "liens externes",
    "voir aussi",
    "articles connexes",
    "annexes",
    "sources",
    "references",
    "external links",
    "see also",
    "further reading",
];

/// Texte brut de l'API (`exsectionformat=wiki`) → paragraphes séparés par une ligne vide,
/// sans titres de section ni sections de renvois.
pub fn clean_extract(extract: &str) -> String {
    let mut paragraphs: Vec<&str> = Vec::new();
    // Niveau de la section ignorée en cours (ses sous-sections le sont aussi).
    let mut skipping: Option<usize> = None;
    for line in extract.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with("==") && line.ends_with("==") {
            let level = line.chars().take_while(|&c| c == '=').count();
            let name = line.trim_matches('=').trim().to_lowercase();
            if skipping.is_some_and(|l| level <= l) {
                skipping = None;
            }
            if skipping.is_none() && SKIPPED_SECTIONS.contains(&name.as_str()) {
                skipping = Some(level);
            }
            continue;
        }
        if skipping.is_none() {
            paragraphs.push(line);
        }
    }
    paragraphs.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXTRACT: &str = "Paris est la capitale de la France.\nElle compte deux millions d'habitants.\n\n\n== Géographie ==\nLa Seine traverse Paris.\n\n=== Climat ===\nTempéré.\n\n\n== Voir aussi ==\n\n=== Articles connexes ===\nLyon\n\n\n== Notes et références ==\nRéf. 1";

    #[test]
    fn top_read_months_add_up() {
        let month = |items: &str| -> Value {
            serde_json::from_str(&format!(r#"{{"items":[{{"articles":[{items}]}}]}}"#)).unwrap()
        };
        let mut totals = std::collections::HashMap::new();
        add_top_month(
            &month(
                r#"{"article":"Paris","views":10},{"article":"Spécial:Recherche","views":99},{"article":"Wikipédia:Accueil_principal","views":99},{"article":"Tour_Eiffel","views":5}"#,
            ),
            &mut totals,
        );
        add_top_month(
            &month(r#"{"article":"Tour_Eiffel","views":7},{"article":"-","views":50}"#),
            &mut totals,
        );
        assert_eq!(totals.len(), 2);
        assert_eq!(totals["Tour Eiffel"], 12);
        assert_eq!(totals["Paris"], 10);
    }

    #[test]
    fn cleans_extracts() {
        assert_eq!(
            clean_extract(EXTRACT),
            "Paris est la capitale de la France.\n\nElle compte deux millions d'habitants.\n\nLa Seine traverse Paris.\n\nTempéré."
        );
    }

    #[test]
    fn parses_api_responses() {
        let json: Value = serde_json::json!({"query": {"pages": [{
            "title": "Paris",
            "extract": EXTRACT,
            "fullurl": "https://fr.wikipedia.org/wiki/Paris",
            "revisions": [{"timestamp": "2026-10-30T12:00:00Z"}]
        }]}});
        let id = SourceId("wikipedia:fr".into());
        let item = article_from_json(&id, "fr", &json, None, Timestamp(0))
            .unwrap()
            .unwrap();
        assert_eq!(item.title.as_deref(), Some("Paris"));
        assert_eq!(item.lang_hint, Some(Lang::FRA));
        assert_eq!(
            item.date().at,
            Timestamp::parse_rfc3339("2026-10-30T12:00:00Z").unwrap()
        );
        // Pas de révision plus récente que `since` : rien à reprendre.
        let since = Timestamp::parse_rfc3339("2026-10-31T00:00:00Z");
        assert!(
            article_from_json(&id, "fr", &json, since, Timestamp(0))
                .unwrap()
                .is_none()
        );
        let missing = serde_json::json!({"query": {"pages": [{"title": "X", "missing": true}]}});
        assert!(
            article_from_json(&id, "fr", &missing, None, Timestamp(0))
                .unwrap()
                .is_none()
        );
    }
}
