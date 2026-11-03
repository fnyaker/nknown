//! Source : une édition de Wikipédia entière (ou ses N premiers articles), lue dans le dump
//! « multistream » de Wikimedia.
//!
//! Le dump n'est jamais enregistré : son index donne la position de chacun de ses blocs bzip2
//! (100 pages), lus un à un par requêtes `Range`. L'avancement (blocs lus) est enregistré dans
//! le dossier d'état : une collecte interrompue reprend au bloc suivant ; un dump terminé n'est
//! relu que lorsque Wikimedia en publie un nouveau. Les articles reçoivent l'identifiant de
//! ceux de l'API (`<édition>:<titre>`) : pas de doublon entre les deux.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use nknown_core::item::RawItem;
use nknown_core::lang::Lang;
use nknown_core::source::{Capabilities, ItemStream, Source, SourceError, SourceId, SourceKind};
use nknown_core::time::{DateConfidence, Dated, Timestamp};

use crate::http::Http;
use crate::wikitext;

/// Serveur des dumps.
pub const DUMPS: &str = "https://dumps.wikimedia.org";

pub fn dump_url(base: &str, edition: &str) -> String {
    format!("{base}/{edition}wiki/latest/{edition}wiki-latest-pages-articles-multistream.xml.bz2")
}

fn index_url(base: &str, edition: &str) -> String {
    format!(
        "{base}/{edition}wiki/latest/{edition}wiki-latest-pages-articles-multistream-index.txt.bz2"
    )
}

/// Avancement enregistré entre deux collectes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct State {
    /// Version du dump (`Last-Modified` et taille).
    dump: String,
    /// Blocs entièrement lus.
    streams_done: usize,
    articles: u64,
    done: bool,
}

impl State {
    fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        let mut s = Self::default();
        for line in text.lines() {
            match line.split_once('=') {
                Some(("dump", v)) => s.dump = v.to_owned(),
                Some(("streams_done", v)) => s.streams_done = v.parse().unwrap_or(0),
                Some(("articles", v)) => s.articles = v.parse().unwrap_or(0),
                Some(("done", v)) => s.done = v == "true",
                _ => {}
            }
        }
        s
    }

    fn save(&self, path: &Path) -> Result<(), SourceError> {
        let text = format!(
            "dump={}\nstreams_done={}\narticles={}\ndone={}\n",
            self.dump, self.streams_done, self.articles, self.done
        );
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }
}

#[derive(Debug)]
pub struct WikiDump {
    id: SourceId,
    edition: String,
    /// Nombre maximal d'articles ; `None` : l'édition entière.
    limit: Option<u64>,
    state_dir: PathBuf,
    http: Http,
    /// Serveur des dumps ([`DUMPS`] ; un serveur local pour les essais).
    base: String,
}

impl WikiDump {
    pub fn new(
        edition: impl Into<String>,
        limit: Option<u64>,
        state_dir: impl Into<PathBuf>,
    ) -> Self {
        let edition = edition.into();
        Self {
            id: SourceId(format!("wikipedia:{edition}")),
            edition,
            limit,
            state_dir: state_dir.into(),
            // Serveurs de dumps : quelques requêtes par seconde au plus.
            http: Http::with_interval(Duration::from_millis(250)),
            base: DUMPS.into(),
        }
    }

    pub fn with_base(mut self, base: impl Into<String>) -> Self {
        self.base = base.into();
        self
    }

    fn state_path(&self) -> PathBuf {
        let n = self.limit.map_or("tout".into(), |n| n.to_string());
        self.state_dir
            .join(format!("{}-dump-{n}.state", self.edition))
    }

    fn offsets_path(&self, dump: &str) -> PathBuf {
        let tag: String = dump.chars().filter(char::is_ascii_alphanumeric).collect();
        self.state_dir
            .join(format!("{}-{tag}.offsets", self.edition))
    }

    /// Positions des blocs (début de chacun, puis la fin du fichier), depuis l'index du dump ;
    /// gardées dans le dossier d'état pour les reprises.
    fn offsets(&self, dump: &str, size: u64) -> Result<Vec<u64>, SourceError> {
        let path = self.offsets_path(dump);
        if let Ok(bytes) = std::fs::read(&path)
            && bytes.len() % 8 == 0
            && !bytes.is_empty()
        {
            return Ok(bytes
                .chunks_exact(8)
                .map(|c| u64::from_le_bytes(c.try_into().expect("8 octets")))
                .collect());
        }
        let reader = bzip2::read::MultiBzDecoder::new(BufReader::new(
            self.http.open(&index_url(&self.base, &self.edition))?,
        ));
        let mut offsets = parse_index(BufReader::new(reader))?;
        offsets.push(size);
        // Anciennes positions (dump précédent) : inutiles.
        if let Ok(entries) = std::fs::read_dir(&self.state_dir) {
            let prefix = format!("{}-", self.edition);
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with(&prefix) && name.ends_with(".offsets") {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        let bytes: Vec<u8> = offsets.iter().flat_map(|o| o.to_le_bytes()).collect();
        std::fs::write(&path, bytes)?;
        Ok(offsets)
    }
}

/// Lignes `position:identifiant:titre` → positions distinctes, croissantes.
fn parse_index(reader: impl BufRead) -> Result<Vec<u64>, SourceError> {
    let mut offsets: Vec<u64> = Vec::new();
    for line in reader.lines() {
        let line = line?;
        let Some(off) = line.split(':').next().and_then(|o| o.parse::<u64>().ok()) else {
            continue;
        };
        if offsets.last() != Some(&off) {
            offsets.push(off);
        }
    }
    offsets.sort_unstable();
    offsets.dedup();
    if offsets.is_empty() {
        return Err(SourceError::Parse("index du dump vide".into()));
    }
    Ok(offsets)
}

impl Source for WikiDump {
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
        format!("{}:dump:{}", self.id.0, self.limit.unwrap_or(0))
    }

    fn fetch(&mut self, _since: Option<Timestamp>) -> Result<ItemStream<'_>, SourceError> {
        std::fs::create_dir_all(&self.state_dir)?;
        let url = dump_url(&self.base, &self.edition);
        let (size, modified) = self.http.head(&url)?;
        let dump = format!("{modified} ({size})");
        let state_path = self.state_path();
        let mut state = State::load(&state_path);
        if state.dump != dump {
            state = State {
                dump: dump.clone(),
                ..State::default()
            };
        }
        if state.done {
            return Ok(Box::new(std::iter::empty()));
        }
        let offsets = self.offsets(&dump, size)?;
        state.save(&state_path)?;
        Ok(Box::new(DumpReader {
            id: self.id.clone(),
            edition: self.edition.clone(),
            url,
            http: self.http.clone(),
            offsets,
            state,
            state_path,
            limit: self.limit,
            pending: VecDeque::new(),
            current: None,
            fetched_at: Timestamp::now(),
            failed: false,
        }))
    }
}

struct DumpReader {
    id: SourceId,
    edition: String,
    url: String,
    http: Http,
    offsets: Vec<u64>,
    state: State,
    state_path: PathBuf,
    limit: Option<u64>,
    pending: VecDeque<RawItem>,
    /// Bloc dont les articles sont en cours de remise.
    current: Option<usize>,
    fetched_at: Timestamp,
    failed: bool,
}

impl DumpReader {
    /// Lit le bloc suivant ; `Ok(false)` à la fin du dump (ou du nombre d'articles voulu).
    fn next_stream(&mut self) -> Result<bool, SourceError> {
        let k = self.state.streams_done;
        let limit_reached = self.limit.is_some_and(|n| self.state.articles >= n);
        if k + 1 >= self.offsets.len() || limit_reached {
            self.state.done = true;
            self.state.save(&self.state_path)?;
            return Ok(false);
        }
        let (start, end) = (self.offsets[k], self.offsets[k + 1]);
        let compressed = self.http.get_range(&self.url, start, end)?;
        self.current = Some(k);
        let mut xml = String::new();
        bzip2::read::BzDecoder::new(&compressed[..])
            .read_to_string(&mut xml)
            .map_err(|e| SourceError::Parse(format!("bloc {k} : {e}")))?;
        for page in pages(&xml) {
            if self
                .limit
                .is_some_and(|n| self.state.articles + self.pending.len() as u64 >= n)
            {
                break;
            }
            if let Some(item) = self.item(&page) {
                self.pending.push_back(item);
            }
        }
        Ok(true)
    }

    fn item(&self, page: &Page<'_>) -> Option<RawItem> {
        if page.ns != "0" || page.redirect || wikitext::is_redirect(page.text) {
            return None;
        }
        let text = wikitext::to_text(&unescape_xml(page.text));
        if text.chars().count() < 200 {
            return None; // ébauche vide, homonymie réduite à une liste
        }
        let title = unescape_xml(page.title);
        let mut item = RawItem::new(self.id.clone(), text, self.fetched_at);
        item.external_id = Some(format!("{}:{title}", self.edition));
        item.url = Some(format!(
            "https://{}.wikipedia.org/wiki/{}",
            self.edition,
            crate::http::encode_component(&title.replace(' ', "_")).replace("%2F", "/")
        ));
        item.title = Some(title);
        item.published_at = Timestamp::parse_rfc3339(page.timestamp).map(|at| Dated {
            at,
            confidence: DateConfidence::Declared,
        });
        item.lang_hint = Lang::from_tag(&self.edition);
        item.meta.push(("date".into(), "dernière révision".into()));
        // Gros volume : analyse légère (sans l'analyseur neuronal).
        item.meta.push(("analysis".into(), "light".into()));
        Some(item)
    }
}

impl Iterator for DumpReader {
    type Item = Result<RawItem, SourceError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(item) = self.pending.pop_front() {
                self.state.articles += 1;
                return Some(Ok(item));
            }
            if self.failed {
                return None;
            }
            // Tous les articles du bloc courant ont été remis : il compte comme lu.
            if self.current.take().is_some() {
                self.state.streams_done += 1;
                if let Err(e) = self.state.save(&self.state_path) {
                    self.failed = true;
                    return Some(Err(e));
                }
            }
            match self.next_stream() {
                Ok(true) => {}
                Ok(false) => return None,
                Err(e) => {
                    // Réseau : on s'arrête ; la prochaine collecte reprend à ce bloc.
                    self.failed = true;
                    return Some(Err(e));
                }
            }
        }
    }
}

/// Une page du dump (texte encore échappé en XML).
#[derive(Debug, PartialEq)]
struct Page<'a> {
    title: &'a str,
    ns: &'a str,
    redirect: bool,
    timestamp: &'a str,
    text: &'a str,
}

fn between<'a>(s: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let i = s.find(open)? + open.len();
    let j = s[i..].find(close)? + i;
    Some(&s[i..j])
}

/// Pages d'un bloc XML (`<page>…</page>`).
fn pages(xml: &str) -> Vec<Page<'_>> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find("<page>") {
        let Some(j) = rest[i..].find("</page>") else {
            break;
        };
        let page = &rest[i..i + j];
        rest = &rest[i + j + 7..];
        // `<text bytes="…" xml:space="preserve">…</text>` (ou `<text … />` si vide).
        let text = page
            .find("<text")
            .and_then(|t| {
                let open_end = t + page[t..].find('>')?;
                if page[..open_end].ends_with('/') {
                    return Some("");
                }
                let close = page[open_end..].find("</text>")? + open_end;
                Some(&page[open_end + 1..close])
            })
            .unwrap_or("");
        out.push(Page {
            title: between(page, "<title>", "</title>").unwrap_or(""),
            ns: between(page, "<ns>", "</ns>").unwrap_or(""),
            redirect: page.contains("<redirect"),
            timestamp: between(page, "<timestamp>", "</timestamp>").unwrap_or(""),
            text,
        });
    }
    out
}

fn unescape_xml(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#039;", "'")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Estimation pour l'écran d'import : taille du dump (octets) et articles de l'édition.
pub fn estimate(http: &Http, edition: &str) -> Result<(u64, u64), SourceError> {
    let (size, _) = http.head(&dump_url(DUMPS, edition))?;
    let json = http.get_json(&format!(
        "https://{edition}.wikipedia.org/w/api.php?action=query&meta=siteinfo&siprop=statistics&format=json&formatversion=2"
    ))?;
    let articles = json["query"]["statistics"]["articles"]
        .as_u64()
        .unwrap_or(0);
    Ok((size, articles))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const PAGE: &str = "<page>\n<title>Tour &amp; Eiffel</title>\n<ns>0</ns>\n<id>1</id>\n<revision><timestamp>2026-10-01T12:00:00Z</timestamp>\n<text bytes=\"9\" xml:space=\"preserve\">'''La tour''' est un [[monument]] de [[Paris]]. </text></revision></page>";

    fn bz(s: &str) -> Vec<u8> {
        let mut e = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
        e.write_all(s.as_bytes()).unwrap();
        e.finish().unwrap()
    }

    #[test]
    fn parses_pages_and_index() {
        let xml = format!(
            "{PAGE}<page><title>Redir</title><ns>0</ns><redirect title=\"X\" /><revision><text bytes=\"1\" xml:space=\"preserve\">#REDIRECT [[X]]</text></revision></page>\
             <page><title>Discussion:Y</title><ns>1</ns><revision><text bytes=\"0\" /></revision></page>"
        );
        let p = pages(&xml);
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].title, "Tour &amp; Eiffel");
        assert!(p[0].text.starts_with("'''La tour'''"));
        assert!(p[1].redirect && p[2].ns == "1" && p[2].text.is_empty());
        let idx = parse_index("600:1:A\n600:2:B\n1200:3:C\n".as_bytes()).unwrap();
        assert_eq!(idx, [600, 1200]);
    }

    /// Serveur local : `/frwiki/latest/…xml.bz2` (avec `Range`) et l'index.
    fn serve(dump: Vec<u8>, index: Vec<u8>) -> String {
        use std::io::{BufRead, BufReader};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let mut range = None;
                let mut h = String::new();
                while reader.read_line(&mut h).is_ok_and(|n| n > 2) {
                    if let Some(r) = h.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        let (a, b) = r.trim().split_once('-').unwrap();
                        range = Some((a.parse::<usize>().unwrap(), b.parse::<usize>().unwrap()));
                    }
                    h.clear();
                }
                let head = line.starts_with("HEAD");
                let body: &[u8] = if line.contains("index") {
                    &index
                } else {
                    &dump
                };
                let (status, part) = match range {
                    Some((a, b)) => ("206 Partial", &body[a..=b]),
                    None => ("200 OK", body),
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nLast-Modified: Sat, 01 Nov 2026 00:00:00 GMT\r\nConnection: close\r\n\r\n",
                    part.len()
                );
                if !head {
                    let _ = stream.write_all(part);
                }
            }
        });
        format!("http://{addr}")
    }

    #[test]
    fn reads_and_resumes_a_dump() {
        let long = "Texte assez long pour compter comme un article. ".repeat(6);
        let page = |t: &str| {
            format!(
                "<page><title>{t}</title><ns>0</ns><revision><timestamp>2026-10-01T12:00:00Z</timestamp><text xml:space=\"preserve\">{long}</text></revision></page>"
            )
        };
        // Trois blocs : en-tête, deux pages, une page.
        let blocks = [
            bz("<mediawiki><siteinfo/>"),
            bz(&(page("A") + &page("B"))),
            bz(&page("C")),
        ];
        let mut dump = Vec::new();
        let mut offs = Vec::new();
        for b in &blocks {
            offs.push(dump.len());
            dump.extend_from_slice(b);
        }
        let index = bz(&format!(
            "{}:1:A\n{}:2:B\n{}:3:C\n",
            offs[1], offs[1], offs[2]
        ));
        let base = serve(dump, index);
        let dir = tempfile::tempdir().unwrap();
        let titles =
            |it: ItemStream<'_>| -> Vec<String> { it.map(|i| i.unwrap().title.unwrap()).collect() };

        // Un article au plus.
        let mut first = WikiDump::new("fr", Some(1), dir.path()).with_base(&base);
        assert_eq!(titles(first.fetch(None).unwrap()), ["A"]);

        // Interruption après le premier bloc (A, B remis, puis C commencé) : la collecte
        // suivante reprend au bloc de C.
        let mut all = WikiDump::new("fr", None, dir.path()).with_base(&base);
        {
            let mut it = all.fetch(None).unwrap();
            let got: Vec<String> = it
                .by_ref()
                .take(3)
                .map(|i| i.unwrap().title.unwrap())
                .collect();
            assert_eq!(got, ["A", "B", "C"]);
        }
        assert_eq!(titles(all.fetch(None).unwrap()), ["C"]);
        // Terminé : rien de plus tant que le dump ne change pas.
        assert_eq!(all.fetch(None).unwrap().count(), 0);
        let state = State::load(&all.state_path());
        assert!(state.done && state.articles == 3 && state.streams_done == 2);
    }
}
