//! Wikitexte (dumps de Wikipédia) → texte brut, comme celui de l'API (`explaintext`).
//!
//! Retirés : modèles `{{…}}` (boîtes, références, bandeaux), tableaux `{|…|}`, références
//! `<ref>`, commentaires, fichiers et catégories, sections de renvois (références, liens
//! externes…). Gardés : le texte des liens (`[[cible|texte]]` → `texte`), les listes, les
//! titres de section (une ligne à part, qui sépare les paragraphes).

/// Sections de renvois, retirées avec leurs sous-sections (comparaison en minuscules).
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
    "notes and references",
    "примечания",
    "ссылки",
    "литература",
    "参考文献",
    "外部链接",
    "参见",
];

/// Préfixes de liens qui ne sont pas du texte : fichiers, images, catégories, interlangues.
const MEDIA_PREFIXES: &[&str] = &[
    "fichier:",
    "file:",
    "image:",
    "media:",
    "catégorie:",
    "category:",
    "категория:",
    "файл:",
    "изображение:",
    "分类:",
    "文件:",
    "图像:",
];

/// L'article est une redirection (à ignorer).
pub fn is_redirect(src: &str) -> bool {
    let head = src.trim_start().get(..16).unwrap_or(src.trim_start());
    head.to_lowercase().starts_with("#redirect")
        || head.to_lowercase().starts_with("#перенаправление")
}

pub fn to_text(src: &str) -> String {
    let s = strip_comments(src);
    let s = strip_refs(&s);
    let s = templates(&s);
    let s = strip_nested(&s, "{|", "|}");
    let s = links(&s);
    let mut out = String::with_capacity(s.len());
    // Niveau de la section ignorée en cours (ses sous-sections le sont aussi).
    let mut skipping: Option<usize> = None;
    for line in s.lines() {
        let t = line.trim();
        if let Some((level, title)) = heading(t) {
            if skipping.is_some_and(|l| level > l) {
                continue;
            }
            skipping = None;
            if SKIPPED_SECTIONS.contains(&title.to_lowercase().as_str()) {
                skipping = Some(level);
                continue;
            }
            push_paragraph(&mut out, &inline(title));
            continue;
        }
        if skipping.is_some() || t.starts_with("__") && t.ends_with("__") {
            continue;
        }
        // Listes et définitions : le texte, sans les marques.
        let t = t.trim_start_matches(['*', '#', ':', ';']).trim();
        if t.is_empty() {
            if !out.ends_with("\n\n") && !out.is_empty() {
                out.push_str("\n\n");
            }
            continue;
        }
        let text = inline(t);
        if text.trim().is_empty() {
            continue;
        }
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(text.trim());
    }
    // Lignes vides en trop.
    let mut clean = String::with_capacity(out.len());
    for para in out.split("\n\n").map(str::trim).filter(|p| !p.is_empty()) {
        if !clean.is_empty() {
            clean.push_str("\n\n");
        }
        clean.push_str(para);
    }
    clean
}

fn push_paragraph(out: &mut String, text: &str) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(text);
    out.push_str("\n\n");
}

/// `== Titre ==` → (niveau, titre).
fn heading(line: &str) -> Option<(usize, &str)> {
    let level = line.bytes().take_while(|&b| b == b'=').count();
    if level < 2 || !line.ends_with("==") {
        return None;
    }
    let end = line.len() - line.bytes().rev().take_while(|&b| b == b'=').count();
    (end > level).then(|| (level, line[level..end].trim()))
}

fn strip_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("<!--") {
        out.push_str(&rest[..i]);
        match rest[i..].find("-->") {
            Some(j) => rest = &rest[i + j + 3..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// `<ref …>…</ref>`, `<ref …/>`, et les blocs sans texte (`<gallery>`, `<math>`, `<syntaxhighlight>`
/// gardé) ; les autres balises sont retirées, leur contenu gardé.
fn strip_refs(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let lower = s.to_ascii_lowercase();
    let mut i = 0;
    while let Some(off) = lower[i..].find('<') {
        let start = i + off;
        out.push_str(&s[i..start]);
        let Some(close) = lower[start..].find('>').map(|c| start + c) else {
            out.push_str(&s[start..]);
            return out;
        };
        let tag = &lower[start + 1..close];
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        let self_closing = tag.ends_with('/');
        let dropped = matches!(
            name.as_str(),
            "ref" | "gallery" | "math" | "timeline" | "imagemap" | "score" | "graph" | "references"
        );
        i = close + 1;
        if dropped && !self_closing && !tag.starts_with('/') {
            // Saute jusqu'à la balise fermante.
            let end = format!("</{name}");
            match lower[i..].find(&end) {
                Some(e) => {
                    let after = i + e;
                    i = lower[after..]
                        .find('>')
                        .map_or(lower.len(), |g| after + g + 1);
                }
                None => i = lower.len(),
            }
        } else if name == "br" {
            out.push('\n');
        }
    }
    out.push_str(&s[i..]);
    out
}

/// Modèles `{{…}}`, des plus intérieurs aux plus extérieurs : ceux qui portent du texte en ligne
/// (dates, nombres, siècles, `lang`…) sont rendus, les autres retirés.
fn templates(s: &str) -> String {
    let mut s = s.to_owned();
    // Chaque passe remplace les modèles sans modèle imbriqué.
    for _ in 0..16 {
        if !s.contains("{{") {
            break;
        }
        let mut out = String::with_capacity(s.len());
        let mut rest = s.as_str();
        let mut changed = false;
        while let Some(i) = rest.find("{{") {
            let inner = &rest[i + 2..];
            let close = inner.find("}}");
            let nested = inner.find("{{");
            match (close, nested) {
                (Some(c), n) if n.is_none_or(|n| n > c) => {
                    out.push_str(&rest[..i]);
                    out.push_str(&render_template(&inner[..c]));
                    rest = &inner[c + 2..];
                    changed = true;
                }
                _ => {
                    out.push_str(&rest[..i + 2]);
                    rest = inner;
                }
            }
        }
        out.push_str(rest);
        s = out;
        if !changed {
            break;
        }
    }
    // Restes déséquilibrés.
    strip_nested(&s, "{{", "}}")
}

/// Texte d'un modèle en ligne ; vide pour les autres (boîtes, bandeaux, références).
fn render_template(body: &str) -> String {
    let mut parts = body.split('|');
    let name = parts.next().unwrap_or("").trim().to_lowercase();
    let args: Vec<&str> = parts.map(str::trim).collect();
    let positional: Vec<&str> = args.iter().copied().filter(|a| !a.contains('=')).collect();
    let named = |key: &str| {
        args.iter().find_map(|a| {
            a.split_once('=')
                .filter(|(k, _)| k.trim() == key)
                .map(|(_, v)| v.trim())
        })
    };
    let joined = || positional.join(" ");
    match name.as_str() {
        "lang" | "langue" | "lang-en" | "nobr" | "nowrap" | "abbr" | "sigle" | "citation"
        | "small" | "petit" | "smallcaps" | "nom" | "math" => {
            positional.last().copied().unwrap_or_default().to_owned()
        }
        n if n.starts_with("lang-") => positional.last().copied().unwrap_or_default().to_owned(),
        "date" | "date-" | "date de naissance" | "date de décès" | "nombre" | "nb"
        | "formatnum" | "unité" | "num" | "nobr date" | "heure" => joined(),
        "s" | "s-" | "-s-" | "sp" | "sp-" | "s2" => {
            format!(
                "{}e siècle",
                positional.first().copied().unwrap_or_default()
            )
        }
        "convert" | "cvt" => positional
            .iter()
            .take(2)
            .copied()
            .collect::<Vec<_>>()
            .join(" "),
        "lien" => named("texte")
            .or_else(|| named("fr"))
            .or_else(|| positional.first().copied())
            .unwrap_or_default()
            .to_owned(),
        "ier" | "er" => "er".into(),
        "e" | "ème" => "e".into(),
        _ => String::new(),
    }
}

/// Retire les blocs imbriqués délimités par `open` et `close` (modèles, tableaux).
fn strip_nested(s: &str, open: &str, close: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0usize;
    let mut i = 0;
    let b = s.as_bytes();
    let mut last = 0;
    while i < b.len() {
        if s[i..].starts_with(open) {
            if depth == 0 {
                out.push_str(&s[last..i]);
            }
            depth += 1;
            i += open.len();
        } else if depth > 0 && s[i..].starts_with(close) {
            depth -= 1;
            i += close.len();
            if depth == 0 {
                last = i;
            }
        } else {
            i += s[i..].chars().next().map_or(1, char::len_utf8);
        }
    }
    if depth == 0 {
        out.push_str(&s[last..]);
    }
    out
}

/// Liens internes : `[[cible|texte]]` → `texte`, `[[cible]]` → `cible` ; fichiers et
/// catégories retirés (avec leurs légendes, qui peuvent contenir des liens).
fn links(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("[[") {
        out.push_str(&rest[..i]);
        // Fin du lien, en tenant compte des liens imbriqués (légendes d'images).
        let body_start = i + 2;
        let mut depth = 1;
        let mut j = body_start;
        let bytes = rest.as_bytes();
        while j + 1 < bytes.len() && depth > 0 {
            if &bytes[j..j + 2] == b"[[" {
                depth += 1;
                j += 2;
            } else if &bytes[j..j + 2] == b"]]" {
                depth -= 1;
                j += 2;
            } else {
                j += rest[j..].chars().next().map_or(1, char::len_utf8);
            }
        }
        if depth > 0 {
            out.push_str(&rest[i..]);
            return out;
        }
        let body = &rest[body_start..j - 2];
        let lower = body.trim_start().to_lowercase();
        if !MEDIA_PREFIXES.iter().any(|p| lower.starts_with(p)) && !is_interlanguage(&lower) {
            let shown = body.rsplit_once('|').map_or(body, |(_, t)| t);
            let shown = shown.trim_start_matches(':');
            out.push_str(&links(shown));
        }
        rest = &rest[j..];
    }
    out.push_str(rest);
    out
}

/// `[[en:Paris]]` : lien vers une autre édition.
fn is_interlanguage(lower: &str) -> bool {
    lower.split_once(':').is_some_and(|(p, _)| {
        (2..=3).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_lowercase())
    })
}

/// Mise en forme en ligne : liens externes, gras et italique, entités.
fn inline(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    // `[http://x texte]` → `texte` ; `[http://x]` → rien.
    while let Some(i) = rest.find('[') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        if (after.starts_with("http://")
            || after.starts_with("https://")
            || after.starts_with("//"))
            && let Some(end) = after.find(']')
        {
            if let Some((_, text)) = after[..end].split_once(' ') {
                out.push_str(text.trim());
            }
            rest = &after[end + 1..];
        } else {
            out.push('[');
            rest = after;
        }
    }
    out.push_str(rest);
    let out = out.replace("'''", "").replace("''", "");
    decode_entities(&out)
}

/// Entités HTML courantes du wikitexte (`&nbsp;`, `&amp;`, `&#160;`…).
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let end = after.find(';').filter(|&e| e <= 10);
        let decoded = end.and_then(|e| {
            let name = &after[..e];
            Some(match name {
                "nbsp" | "#160" | "thinsp" | "ensp" | "emsp" => ' ',
                "amp" => '&',
                "lt" => '<',
                "gt" => '>',
                "quot" => '"',
                "apos" => '\'',
                "ndash" => '–',
                "mdash" => '—',
                _ => {
                    let n = name.strip_prefix('#')?;
                    let code = match n.strip_prefix(['x', 'X']) {
                        Some(h) => u32::from_str_radix(h, 16).ok()?,
                        None => n.parse().ok()?,
                    };
                    char::from_u32(code)?
                }
            })
        });
        match (decoded, end) {
            (Some(c), Some(e)) => {
                out.push(c);
                rest = &after[e + 1..];
            }
            _ => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_an_article() {
        let src = "{{Infobox Ville|nom=Paris|pays={{France}}}}\n\
            '''Paris''' est la [[capitale de la France|capitale]] de la [[France]].<ref>{{Lien web|url=x}}</ref>\n\
            [[Fichier:Paris.jpg|vignette|La [[tour Eiffel]] vue du [[Trocadéro]].]]\n\
            <!-- commentaire -->\n\
            {| class=\"wikitable\"\n| a || b\n|}\n\
            == Histoire ==\n\
            * Fondée par les [[Parisii]] ;\n\
            * voir [https://example.org le site] et [https://x.org].\n\
            == Notes et références ==\n\
            {{Références}}\n\
            === Sous-section ===\n\
            texte de renvoi\n\
            == Géographie ==\n\
            La Seine&nbsp;traverse Paris.\n\
            [[Catégorie:Capitale]]\n[[en:Paris]]";
        let t = to_text(src);
        assert!(t.starts_with("Paris est la capitale de la France."), "{t}");
        assert!(
            t.contains("Histoire\n\nFondée par les Parisii ;\nvoir le site et ."),
            "{t}"
        );
        assert!(t.contains("La Seine traverse Paris."), "{t}");
        for absent in [
            "Infobox",
            "tour Eiffel",
            "wikitable",
            "commentaire",
            "renvoi",
            "Catégorie",
            "en:Paris",
            "Lien web",
        ] {
            assert!(!t.contains(absent), "{absent} : {t}");
        }
    }

    #[test]
    fn inline_templates_keep_their_text() {
        let t = to_text(
            "Fondée au {{s-|XIX}}, peuplée de {{nombre|500000|habitants}}, dite {{lang|en|the city}} {{Infobox|x={{s|I}}}}.",
        );
        assert_eq!(
            t,
            "Fondée au XIXe siècle, peuplée de 500000 habitants, dite the city ."
        );
    }

    #[test]
    fn redirects_and_entities() {
        assert!(is_redirect("#REDIRECTION [[Paris]]"));
        assert!(is_redirect("  #redirect [[Paris]]"));
        assert!(!is_redirect("Paris est…"));
        assert_eq!(
            decode_entities("a&#233;b &amp; c &unknown; d"),
            "aéb & c &unknown; d"
        );
    }
}

/// Essai manuel : `NKNOWN_WIKITEXT=<fichier> cargo test -p nknown-sources wikitext_file -- --ignored --nocapture`.
#[cfg(test)]
#[test]
#[ignore = "fichier fourni à la main"]
fn wikitext_file() {
    let path = std::env::var("NKNOWN_WIKITEXT").expect("NKNOWN_WIKITEXT");
    let src = std::fs::read_to_string(path).unwrap();
    println!("{}", to_text(&src));
}
