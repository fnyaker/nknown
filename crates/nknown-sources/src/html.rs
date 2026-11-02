//! HTML → texte, pour les résumés de flux (souvent en HTML).
//!
//! Volontairement simple : retire les balises, garde un saut de paragraphe pour les blocs,
//! ignore scripts et styles, décode les entités courantes et numériques.

const BLOCKS: &[&str] = &[
    "p",
    "br",
    "div",
    "li",
    "ul",
    "ol",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "tr",
    "blockquote",
    "section",
    "article",
    "figure",
    "figcaption",
    "hr",
];

pub fn to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(lt) = rest.find('<') {
        out.push_str(&decode_entities(&rest[..lt]));
        let Some(gt) = rest[lt..].find('>') else {
            rest = "";
            break;
        };
        let tag = &rest[lt + 1..lt + gt];
        rest = &rest[lt + gt + 1..];
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        if (name == "script" || name == "style") && !tag.starts_with('/') {
            let close = format!("</{name}");
            match rest.to_ascii_lowercase().find(&close) {
                Some(i) => rest = rest[i..].split_once('>').map_or("", |(_, r)| r),
                None => rest = "",
            }
            continue;
        }
        if BLOCKS.contains(&name.as_str()) {
            out.push_str("\n\n");
        }
    }
    out.push_str(&decode_entities(rest));
    normalize_spaces(&out)
}

/// Espaces fusionnés dans les lignes, paragraphes séparés par une ligne vide.
fn normalize_spaces(s: &str) -> String {
    s.split("\n\n")
        .map(|p| p.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let decoded = rest.find(';').filter(|&semi| semi <= 10).and_then(|semi| {
            let entity = &rest[1..semi];
            let c = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                "laquo" => Some('«'),
                "raquo" => Some('»'),
                "rsquo" => Some('\u{2019}'),
                "lsquo" => Some('\u{2018}'),
                "hellip" => Some('…'),
                "mdash" => Some('—'),
                "ndash" => Some('–'),
                "eacute" => Some('é'),
                "egrave" => Some('è'),
                "agrave" => Some('à'),
                "ccedil" => Some('ç'),
                _ => entity
                    .strip_prefix("#x")
                    .or_else(|| entity.strip_prefix("#X"))
                    .and_then(|h| u32::from_str_radix(h, 16).ok())
                    .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                    .and_then(char::from_u32),
            }?;
            Some((c, semi + 1))
        });
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
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
    fn strips_tags_and_keeps_paragraphs() {
        let html = "<p>Grève à <b>Paris</b>&nbsp;:</p><p>les rues  sont <i>calmes</i>.<script>x=1</script></p>";
        assert_eq!(to_text(html), "Grève à Paris :\n\nles rues sont calmes.");
    }

    #[test]
    fn decodes_entities() {
        assert_eq!(
            decode_entities("l&rsquo;actu &amp; &#233;t&#xE9; &unknown; R&D"),
            "l\u{2019}actu & été &unknown; R&D"
        );
    }
}
