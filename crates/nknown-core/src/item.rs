//! Contenu brut produit par une source.

use crate::lang::Lang;
use crate::source::SourceId;
use crate::time::{DateConfidence, Dated, Timestamp};

/// Un contenu tel que livré par une [`Source`](crate::source::Source), avant analyse.
#[derive(Debug, Clone, PartialEq)]
pub struct RawItem {
    pub source: SourceId,
    /// Identifiant stable dans la source (chemin, GUID RSS, titre de page...).
    pub external_id: Option<String>,
    pub url: Option<String>,
    pub title: Option<String>,
    pub text: String,
    pub published_at: Option<Dated>,
    pub fetched_at: Timestamp,
    pub lang_hint: Option<Lang>,
    pub meta: Vec<(String, String)>,
}

impl RawItem {
    pub fn new(source: SourceId, text: String, fetched_at: Timestamp) -> Self {
        Self {
            source,
            external_id: None,
            url: None,
            title: None,
            text,
            published_at: None,
            fetched_at,
            lang_hint: None,
            meta: Vec::new(),
        }
    }

    /// Date à retenir : la date de publication si connue, sinon la date de collecte.
    pub fn date(&self) -> Dated {
        self.published_at.unwrap_or(Dated {
            at: self.fetched_at,
            confidence: DateConfidence::FetchTimeOnly,
        })
    }
}
