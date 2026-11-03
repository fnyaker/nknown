//! Source : un site web, exploré à partir d'une adresse.
//!
//! Spécification : `web:<url>`, suivie au besoin de `pages=<n>` et `depth=<n>`
//! (`web:https://doc.exemple.org/guide/ pages=500 depth=4`).
//!
//! - Périmètre : même hôte, sous le dossier de l'adresse de départ ; HTML seulement.
//! - Politesse : `robots.txt` respecté (groupes `*` et `NKnown`, `Crawl-delay`), au moins une
//!   seconde entre deux pages, une seule requête à la fois.
//! - Protection anti-robot (Cloudflare, reCAPTCHA, hCaptcha, Turnstile, DDoS-Guard, réponse
//!   429…) : l'exploration s'arrête aussitôt ([`SourceError::Blocked`]) ; on ne la contourne pas.
//! - Contenu : `<main>` ou `<article>` s'il y en a un (sans menus ni pied de page), sinon la page.

use std::collections::{HashSet, VecDeque};
use std::time::Duration;

use nknown_core::item::RawItem;
use nknown_core::source::{Capabilities, ItemStream, Source, SourceError, SourceId, SourceKind};
use nknown_core::time::{DateConfidence, Dated, Timestamp};
use url::Url;

use crate::html;
use crate::http::{Http, Page};

pub const DEFAULT_PAGES: usize = 200;
pub const DEFAULT_DEPTH: usize = 3;
/// Délai minimal entre deux pages (relevé si `robots.txt` demande plus).
const MIN_DELAY: Duration = if cfg!(test) {
    Duration::ZERO
} else {
    Duration::from_secs(1)
};
const MAX_DELAY: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub struct WebSource {
    id: SourceId,
    start: Url,
    max_pages: usize,
    max_depth: usize,
}

impl WebSource {
    /// `rest` : l'adresse, suivie au besoin de `pages=<n>` et `depth=<n>`.
    pub fn parse(rest: &str) -> Result<Self, String> {
        let mut parts = rest.split_whitespace();
        let url = parts.next().ok_or("adresse manquante")?;
        let start = Url::parse(url).map_err(|e| format!("{url} : {e}"))?;
        if !matches!(start.scheme(), "http" | "https") || start.host_str().is_none() {
            return Err(format!("{url} : adresse web attendue"));
        }
        let (mut max_pages, mut max_depth) = (DEFAULT_PAGES, DEFAULT_DEPTH);
        for p in parts {
            match p.split_once('=') {
                Some(("pages", n)) => max_pages = n.parse().map_err(|_| format!("pages : {n}"))?,
                Some(("depth", n)) => max_depth = n.parse().map_err(|_| format!("depth : {n}"))?,
                _ => return Err(format!("option inconnue : {p}")),
            }
        }
        Ok(Self {
            id: SourceId(format!("web:{}", start)),
            start,
            max_pages: max_pages.max(1),
            max_depth,
        })
    }
}

impl Source for WebSource {
    fn id(&self) -> &SourceId {
        &self.id
    }

    fn kind(&self) -> SourceKind {
        SourceKind::Web
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            published_at: false,
            lang: false,
            incremental: false,
        }
    }

    fn fetch(&mut self, _since: Option<Timestamp>) -> Result<ItemStream<'_>, SourceError> {
        let http = Http::with_interval(MIN_DELAY);
        let robots = Robots::fetch(&http, &self.start);
        let http = Http::with_interval(robots.delay.clamp(MIN_DELAY, MAX_DELAY));
        let mut queue = VecDeque::new();
        queue.push_back((self.start.clone(), 0));
        Ok(Box::new(Crawl {
            id: self.id.clone(),
            scope: scope_of(&self.start),
            http,
            robots,
            queue,
            seen: HashSet::from([key(&self.start)]),
            pages: 0,
            max_pages: self.max_pages,
            max_depth: self.max_depth,
            stopped: false,
        }))
    }
}

/// Préfixe d'adresse explorable : hôte et dossier de l'adresse de départ.
fn scope_of(start: &Url) -> String {
    let mut s = start.clone();
    s.set_query(None);
    s.set_fragment(None);
    let path = s.path();
    let dir = &path[..path.rfind('/').map_or(0, |i| i + 1)];
    format!("{}{dir}", s.origin().ascii_serialization())
}

/// Clé de déduplication : sans fragment.
fn key(u: &Url) -> String {
    let mut u = u.clone();
    u.set_fragment(None);
    u.to_string()
}

struct Crawl {
    id: SourceId,
    scope: String,
    http: Http,
    robots: Robots,
    queue: VecDeque<(Url, usize)>,
    seen: HashSet<String>,
    pages: usize,
    max_pages: usize,
    max_depth: usize,
    stopped: bool,
}

impl Iterator for Crawl {
    type Item = Result<RawItem, SourceError>;

    fn next(&mut self) -> Option<Self::Item> {
        while !self.stopped && self.pages < self.max_pages {
            let (url, depth) = self.queue.pop_front()?;
            if !self.robots.allows(url.path()) {
                continue;
            }
            let page = match self.http.get_page(url.as_str()) {
                Ok(p) => p,
                Err(e) => return Some(Err(e)),
            };
            if let Some(reason) = bot_wall(&page) {
                self.stopped = true;
                return Some(Err(SourceError::Blocked(format!(
                    "{url} : {reason} ; exploration arrêtée"
                ))));
            }
            if !(200..300).contains(&page.status) || !page.content_type.contains("html") {
                continue;
            }
            self.pages += 1;
            let body = String::from_utf8_lossy(&page.body);
            if depth < self.max_depth {
                for link in links(&body, &url) {
                    let k = key(&link);
                    if link.as_str().starts_with(&self.scope)
                        && !skipped_extension(link.path())
                        && self.seen.insert(k)
                    {
                        self.queue.push_back((link, depth + 1));
                    }
                }
            }
            let text = html::to_text(main_content(&body));
            if text.trim().is_empty() {
                continue;
            }
            let mut item = RawItem::new(self.id.clone(), text, Timestamp::now());
            item.title = title(&body);
            item.url = Some(url.to_string());
            item.external_id = Some(key(&url));
            item.meta.push(("format".into(), "markdown".into()));
            if let Some(at) = page.last_modified {
                item.published_at = Some(Dated {
                    at,
                    confidence: DateConfidence::Inferred,
                });
            }
            return Some(Ok(item));
        }
        None
    }
}

/// Protection anti-robot reconnue : la raison, pour l'arrêt.
pub fn bot_wall(page: &Page) -> Option<&'static str> {
    if page.challenge_header {
        return Some("défi anti-robot (Cloudflare)");
    }
    if page.status == 429 {
        return Some("trop de requêtes (429)");
    }
    let head: String =
        String::from_utf8_lossy(&page.body[..page.body.len().min(64 * 1024)]).to_lowercase();
    const MARKERS: &[(&str, &str)] = &[
        ("challenges.cloudflare.com", "défi anti-robot (Cloudflare)"),
        ("cf-chl-", "défi anti-robot (Cloudflare)"),
        ("cf-turnstile", "captcha (Turnstile)"),
        ("g-recaptcha", "captcha (reCAPTCHA)"),
        ("www.google.com/recaptcha", "captcha (reCAPTCHA)"),
        ("h-captcha", "captcha (hCaptcha)"),
        ("hcaptcha.com", "captcha (hCaptcha)"),
        ("ddos-guard", "protection DDoS-Guard"),
        (
            "<title>just a moment...</title>",
            "défi anti-robot (Cloudflare)",
        ),
        ("attention required! | cloudflare", "blocage Cloudflare"),
        (
            "please verify you are a human",
            "vérification humaine demandée",
        ),
        ("are you a robot", "vérification humaine demandée"),
    ];
    MARKERS
        .iter()
        .find(|(m, _)| head.contains(m))
        .map(|(_, reason)| *reason)
        .or_else(|| {
            // Refus pur et simple d'une page HTML qui parle de robots.
            (matches!(page.status, 401 | 403 | 503)
                && (head.contains("captcha") || head.contains("bot")))
            .then_some("accès refusé aux robots")
        })
}

fn skipped_extension(path: &str) -> bool {
    let ext = path
        .rsplit_once('.')
        .map_or("", |(_, e)| e)
        .to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "pdf"
            | "jpg"
            | "jpeg"
            | "png"
            | "gif"
            | "webp"
            | "svg"
            | "ico"
            | "zip"
            | "gz"
            | "tar"
            | "xz"
            | "bz2"
            | "7z"
            | "mp3"
            | "mp4"
            | "webm"
            | "avi"
            | "mkv"
            | "ogg"
            | "wav"
            | "css"
            | "js"
            | "json"
            | "xml"
            | "exe"
            | "dmg"
            | "iso"
            | "deb"
            | "rpm"
            | "woff"
            | "woff2"
            | "ttf"
    )
}

/// Liens `<a href>` d'une page, résolus par rapport à elle.
fn links(body: &str, base: &Url) -> Vec<Url> {
    let lower = body.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = lower[from..].find("<a ") {
        let start = from + i;
        let end = lower[start..].find('>').map_or(lower.len(), |e| start + e);
        let tag = &body[start..end];
        if let Some(href) = attr(tag, "href")
            && !href.starts_with("javascript:")
            && !href.starts_with("mailto:")
            && let Ok(u) = base.join(href.trim())
            && matches!(u.scheme(), "http" | "https")
        {
            out.push(u);
        }
        from = end;
    }
    out
}

/// Valeur d'un attribut dans une balise (guillemets doubles ou simples).
fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let lower = tag.to_ascii_lowercase();
    let i = lower.find(&format!("{name}="))? + name.len() + 1;
    let rest = &tag[i..];
    let quote = rest.chars().next()?;
    if quote == '"' || quote == '\'' {
        let rest = &rest[1..];
        Some(&rest[..rest.find(quote)?])
    } else {
        Some(rest.split(|c: char| c.is_whitespace() || c == '>').next()?)
    }
}

fn title(body: &str) -> Option<String> {
    let lower = body.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let open = start + lower[start..].find('>')? + 1;
    let close = open + lower[open..].find("</title>")?;
    let t = html::to_text(&body[open..close]);
    let t = t.trim();
    (!t.is_empty()).then(|| t.to_owned())
}

/// Le contenu principal : `<main>`, sinon `<article>`, sinon la page entière.
fn main_content(body: &str) -> &str {
    let lower = body.to_ascii_lowercase();
    for tag in ["main", "article"] {
        if let Some(s) = lower.find(&format!("<{tag}"))
            && let Some(e) = lower.rfind(&format!("</{tag}>"))
            && e > s
        {
            return &body[s..e];
        }
    }
    body
}

/// Règles de `robots.txt` qui nous concernent (`*` et `NKnown`).
#[derive(Debug, Default)]
struct Robots {
    /// (préfixe, autorisé) ; la règle la plus longue l'emporte.
    rules: Vec<(String, bool)>,
    delay: Duration,
}

impl Robots {
    fn fetch(http: &Http, start: &Url) -> Self {
        let Ok(url) = start.join("/robots.txt") else {
            return Self::default();
        };
        match http.get_page(url.as_str()) {
            Ok(p) if (200..300).contains(&p.status) => {
                Self::parse(&String::from_utf8_lossy(&p.body))
            }
            _ => Self::default(),
        }
    }

    fn parse(text: &str) -> Self {
        let mut robots = Self::default();
        let (mut ours, mut in_agents) = (false, false);
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or_default().trim();
            let Some((field, value)) = line.split_once(':') else {
                continue;
            };
            let (field, value) = (field.trim().to_ascii_lowercase(), value.trim());
            match field.as_str() {
                "user-agent" => {
                    // Plusieurs User-agent d'affilée forment un même groupe.
                    if !in_agents {
                        ours = false;
                    }
                    in_agents = true;
                    let v = value.to_ascii_lowercase();
                    ours |= v == "*" || v.contains("nknown");
                }
                "disallow" | "allow" if ours => {
                    in_agents = false;
                    if !value.is_empty() {
                        robots.rules.push((value.to_owned(), field == "allow"));
                    }
                }
                "crawl-delay" if ours => {
                    in_agents = false;
                    if let Ok(s) = value.parse::<f64>() {
                        robots.delay = Duration::from_secs_f64(s.max(0.0));
                    }
                }
                _ => in_agents = false,
            }
        }
        robots
    }

    fn allows(&self, path: &str) -> bool {
        self.rules
            .iter()
            .filter(|(prefix, _)| path.starts_with(prefix.trim_end_matches('*')))
            .max_by_key(|(prefix, allow)| (prefix.len(), *allow))
            .is_none_or(|(_, allow)| *allow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(status: u16, body: &str) -> Page {
        Page {
            status,
            content_type: "text/html".into(),
            body: body.as_bytes().to_vec(),
            challenge_header: false,
            last_modified: None,
        }
    }

    #[test]
    fn spec_and_scope() {
        let w =
            WebSource::parse("https://doc.exemple.org/guide/intro.html pages=10 depth=2").unwrap();
        assert_eq!((w.max_pages, w.max_depth), (10, 2));
        assert_eq!(scope_of(&w.start), "https://doc.exemple.org/guide/");
        assert!(WebSource::parse("ftp://x/").is_err());
        assert!(WebSource::parse("https://x/ vitesse=9").is_err());
    }

    #[test]
    fn detects_bot_walls() {
        assert!(bot_wall(&page(200, "<html><title>Just a moment...</title>")).is_some());
        assert!(bot_wall(&page(200, "<div class=\"g-recaptcha\"></div>")).is_some());
        assert!(bot_wall(&page(403, "<p>Bot detected</p>")).is_some());
        assert!(bot_wall(&page(429, "slow down")).is_some());
        let mut p = page(403, "nope");
        p.challenge_header = true;
        assert!(bot_wall(&p).is_some());
        // Une page ordinaire qui parle de robots n'est pas un mur.
        assert!(bot_wall(&page(200, "<p>Les robots de cuisine</p>")).is_none());
        assert!(bot_wall(&page(404, "introuvable")).is_none());
    }

    #[test]
    fn links_titles_and_main_content() {
        let base = Url::parse("https://x.org/doc/a.html").unwrap();
        let body = r#"<html><head><title>Guide &amp; plus</title></head><body>
            <nav><a href="/menu">menu</a></nav>
            <main><a href='b.html#s'>B</a> <a href="https://y.org/">Y</a>
            <a href="mailto:a@b">m</a><p>Contenu utile</p></main></body></html>"#;
        let l: Vec<String> = links(body, &base).iter().map(Url::to_string).collect();
        assert!(l.contains(&"https://x.org/doc/b.html#s".to_owned()));
        assert!(l.contains(&"https://x.org/menu".to_owned()));
        assert!(!l.iter().any(|u| u.starts_with("mailto")));
        assert_eq!(title(body).as_deref(), Some("Guide & plus"));
        let main = html::to_text(main_content(body));
        assert!(main.contains("Contenu utile") && !main.contains("menu"));
    }

    /// Petit serveur local : `pages` (chemin → (statut, corps)), une connexion par requête.
    fn serve(pages: Vec<(&'static str, u16, &'static str)>) -> String {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut line = String::new();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                reader.read_line(&mut line).unwrap();
                // En-têtes jusqu'à la ligne vide.
                let mut h = String::new();
                while reader.read_line(&mut h).is_ok_and(|n| n > 2) {
                    h.clear();
                }
                let path = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
                let (status, body) = pages
                    .iter()
                    .find(|(p, _, _)| *p == path)
                    .map_or((404, "absent"), |(_, s, b)| (*s, *b));
                let kind = if path.ends_with(".txt") {
                    "text/plain"
                } else {
                    "text/html"
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n\
                     Connection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        format!("http://{addr}")
    }

    fn crawl(spec: &str) -> Vec<Result<RawItem, SourceError>> {
        let mut w = WebSource::parse(spec.strip_prefix("web:").unwrap()).unwrap();
        w.fetch(None).unwrap().collect()
    }

    #[test]
    fn crawls_within_scope_and_robots() {
        let base = serve(vec![
            ("/robots.txt", 200, "User-agent: *\nDisallow: /doc/prive\n"),
            (
                "/doc/",
                200,
                "<title>Accueil</title><p>Bienvenue</p><a href=\"a.html\">a</a>\
                 <a href=\"/ailleurs.html\">hors</a><a href=\"prive.html\">p</a>",
            ),
            (
                "/doc/a.html",
                200,
                "<p>Page A</p><a href=\"/doc/\">retour</a>",
            ),
            ("/doc/prive.html", 200, "<p>secret</p>"),
            ("/ailleurs.html", 200, "<p>hors périmètre</p>"),
        ]);
        let items: Vec<RawItem> = crawl(&format!("web:{base}/doc/"))
            .into_iter()
            .map(Result::unwrap)
            .collect();
        let texts: Vec<&str> = items.iter().map(|i| i.text.as_str()).collect();
        assert_eq!(items.len(), 2, "{texts:?}");
        assert_eq!(items[0].title.as_deref(), Some("Accueil"));
        assert!(texts[1].contains("Page A"));
    }

    #[test]
    fn stops_on_captcha() {
        let base = serve(vec![
            (
                "/",
                200,
                "<p>Début</p><a href=\"/b\">b</a><a href=\"/c\">c</a>",
            ),
            ("/b", 200, "<div class=\"h-captcha\"></div>"),
            ("/c", 200, "<p>jamais lue</p>"),
        ]);
        let out = crawl(&format!("web:{base}/"));
        assert_eq!(out.len(), 2);
        assert!(out[0].is_ok());
        assert!(matches!(out[1], Err(SourceError::Blocked(_))));
    }

    #[test]
    fn robots_rules() {
        let r = Robots::parse(
            "User-agent: Googlebot\nDisallow: /\n\nUser-agent: *\nDisallow: /prive/\n\
             Allow: /prive/public/\nCrawl-delay: 2\n",
        );
        assert!(r.allows("/doc/a"));
        assert!(!r.allows("/prive/x"));
        assert!(r.allows("/prive/public/y"));
        assert_eq!(r.delay, Duration::from_secs(2));
    }
}
