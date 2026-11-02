//! Source : un flux RSS, Atom ou JSON Feed.
//!
//! Chaque entrée devient un contenu : titre, puis résumé ou contenu (HTML converti en texte),
//! daté par sa date de publication (ou de mise à jour) déclarée par le flux.
//! Seul le flux est lu : les articles complets ne sont pas téléchargés.

use nknown_core::item::RawItem;
use nknown_core::lang::Lang;
use nknown_core::source::{Capabilities, ItemStream, Source, SourceError, SourceId, SourceKind};
use nknown_core::time::{DateConfidence, Dated, Timestamp};

use crate::html;
use crate::http::Http;

#[derive(Debug)]
pub struct RssSource {
    id: SourceId,
    url: String,
    http: Http,
}

impl RssSource {
    pub fn new(url: impl Into<String>) -> Self {
        let url = url.into();
        Self {
            id: SourceId(format!("rss:{url}")),
            url,
            http: Http::new(),
        }
    }
}

impl Source for RssSource {
    fn id(&self) -> &SourceId {
        &self.id
    }

    fn kind(&self) -> SourceKind {
        SourceKind::Feed
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            published_at: true,
            lang: true,
            incremental: true,
        }
    }

    fn fetch(&mut self, since: Option<Timestamp>) -> Result<ItemStream<'_>, SourceError> {
        let bytes = self.http.get_bytes(&self.url)?;
        let items = parse_feed(&self.id, &bytes, since, Timestamp::now())?;
        Ok(Box::new(items.into_iter().map(Ok)))
    }
}

/// Analyse un flux déjà téléchargé.
pub fn parse_feed(
    id: &SourceId,
    bytes: &[u8],
    since: Option<Timestamp>,
    fetched_at: Timestamp,
) -> Result<Vec<RawItem>, SourceError> {
    let feed = feed_rs::parser::parse(bytes).map_err(|e| SourceError::Parse(e.to_string()))?;
    let feed_lang = feed.language.as_deref().and_then(Lang::from_tag);
    let feed_title = feed.title.map(|t| html::to_text(&t.content));
    let mut items = Vec::new();
    for entry in feed.entries {
        let date = entry
            .published
            .or(entry.updated)
            .map(|d| Timestamp(d.timestamp()));
        if let (Some(since), Some(d)) = (since, date)
            && d <= since
        {
            continue;
        }
        let title = entry.title.map(|t| html::to_text(&t.content));
        let body = entry
            .content
            .and_then(|c| c.body)
            .or(entry.summary.map(|s| s.content))
            .map(|b| html::to_text(&b))
            .unwrap_or_default();
        let text = match (&title, body.is_empty()) {
            (Some(t), false) => format!("{t}\n\n{body}"),
            (Some(t), true) => t.clone(),
            (None, _) => body,
        };
        if text.trim().is_empty() {
            continue;
        }
        let mut item = RawItem::new(id.clone(), text, fetched_at);
        item.external_id = Some(entry.id);
        item.url = entry.links.into_iter().next().map(|l| l.href);
        item.title = title;
        item.published_at = date.map(|at| Dated {
            at,
            confidence: DateConfidence::Declared,
        });
        item.lang_hint = entry
            .language
            .as_deref()
            .and_then(Lang::from_tag)
            .or(feed_lang);
        if let Some(t) = &feed_title {
            item.meta.push(("flux".into(), t.clone()));
        }
        // Le HTML converti garde les blocs de code sous forme Markdown.
        item.meta.push(("format".into(), "markdown".into()));
        items.push(item);
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RSS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0"><channel>
  <title>Le Journal</title><language>fr-FR</language>
  <item>
    <title>Manifestation &#224; Paris</title>
    <link>https://exemple.fr/a</link>
    <guid>a1</guid>
    <pubDate>Mon, 02 Nov 2026 08:30:00 +0100</pubDate>
    <description><![CDATA[<p>Les rues <b>calmes</b> de Paris&nbsp;: un cortège.</p>]]></description>
  </item>
  <item>
    <title>Vieille nouvelle</title>
    <guid>a0</guid>
    <pubDate>Sun, 01 Nov 2026 08:00:00 +0000</pubDate>
  </item>
</channel></rss>"#;

    const ATOM: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xml:lang="en">
  <title>Blog</title><id>urn:b</id><updated>2026-11-02T10:00:00Z</updated>
  <entry><title>Hello</title><id>urn:b:1</id><updated>2026-11-02T10:00:00Z</updated>
    <link href="https://exemple.org/1"/><summary>World</summary></entry>
</feed>"#;

    #[test]
    fn parses_rss() {
        let id = SourceId("rss:test".into());
        let items = parse_feed(&id, RSS.as_bytes(), None, Timestamp(0)).unwrap();
        assert_eq!(items.len(), 2);
        let a = &items[0];
        assert_eq!(
            a.text,
            "Manifestation à Paris\n\nLes rues calmes de Paris : un cortège."
        );
        assert_eq!(a.url.as_deref(), Some("https://exemple.fr/a"));
        assert_eq!(a.lang_hint, Some(Lang::FRA));
        assert_eq!(a.date().confidence, DateConfidence::Declared);
        assert_eq!(
            a.date().at,
            Timestamp::parse_rfc3339("2026-11-02T07:30:00Z").unwrap()
        );
        assert_eq!(a.meta[0], ("flux".to_string(), "Le Journal".to_string()));
        assert_eq!(items[1].text, "Vieille nouvelle");
    }

    #[test]
    fn incremental_fetch_skips_old_entries() {
        let id = SourceId("rss:test".into());
        let since = Timestamp::parse_rfc3339("2026-11-01T12:00:00Z");
        let items = parse_feed(&id, RSS.as_bytes(), since, Timestamp(0)).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].external_id.as_deref(), Some("a1"));
    }

    #[test]
    fn parses_atom() {
        let id = SourceId("rss:atom".into());
        let items = parse_feed(&id, ATOM.as_bytes(), None, Timestamp(0)).unwrap();
        assert_eq!(items[0].text, "Hello\n\nWorld");
        assert_eq!(items[0].lang_hint, Some(Lang::ENG));
    }
}
