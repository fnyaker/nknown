//! Segmentation d'un document en blocs de prose et de code, imbriqués.
//!
//! - Markdown : blocs délimités (```lang / ~~~), le reste passe au classifieur de lignes ;
//! - texte brut : classifieur de lignes (code collé sans balisage) ;
//! - fichier de code : tout est du code, sauf les commentaires et docstrings (prose) ;
//! - dans un bloc de code, les commentaires deviennent des blocs de prose ;
//! - dans un bloc de prose, [`inline_code`] repère le code en ligne (`` `x` ``, `read_to_string`).

use std::ops::Range;

use nknown_core::analyze::SegmentKind;

/// Format déclaré d'un document (déduit de l'extension, du type de source...).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Format {
    Plain,
    Markdown,
    /// Fichier de code (langage éventuel).
    Code {
        lang: Option<String>,
    },
}

impl Format {
    /// Format d'après une indication textuelle : `plain`, `markdown`, `code` ou `code:rust`.
    pub fn from_hint(hint: &str) -> Format {
        match hint.split_once(':') {
            Some(("code", lang)) => Format::Code {
                lang: Some(lang.to_owned()),
            },
            _ => match hint {
                "markdown" | "md" => Format::Markdown,
                "code" => Format::Code { lang: None },
                _ => Format::Plain,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    /// Contenu du bloc (sans les lignes de délimitation).
    pub span: Range<usize>,
    pub kind: SegmentKind,
}

/// Découpe `text` en blocs ordonnés, sans chevauchement.
pub fn blocks(text: &str, format: &Format) -> Vec<Block> {
    match format {
        Format::Code { lang } => {
            let lang = lang.clone().or_else(|| guess_lang(text));
            code_with_comments(text, 0..text.len(), lang)
        }
        Format::Markdown => markdown(text),
        Format::Plain => classify(text, 0..text.len()),
    }
}

/// Lignes de `text[span]` avec leur position absolue (sans le saut de ligne).
fn lines(text: &str, span: Range<usize>) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = span.start;
    for (i, c) in text[span.clone()].char_indices() {
        if c == '\n' {
            out.push(start..span.start + i);
            start = span.start + i + 1;
        }
    }
    if start < span.end {
        out.push(start..span.end);
    }
    out
}

fn markdown(text: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut prose_start: Option<usize> = None;
    let mut fence: Option<(char, usize, Option<String>, usize)> = None; // (car, longueur, langage, début)
    for line in lines(text, 0..text.len()) {
        let l = &text[line.clone()];
        let t = l.trim_start();
        let marker = t.chars().next().filter(|c| *c == '`' || *c == '~');
        let run = marker.map_or(0, |m| t.chars().take_while(|&c| c == m).count());
        match &fence {
            None if run >= 3 => {
                if let Some(s) = prose_start.take() {
                    out.extend(classify(text, s..line.start));
                }
                let info = t[run..].trim();
                let lang = info
                    .split(|c: char| c.is_whitespace() || c == ',' || c == '{')
                    .next()
                    .filter(|s| !s.is_empty())
                    .map(str::to_lowercase);
                fence = Some((
                    marker.unwrap_or('`'),
                    run,
                    lang,
                    (line.end + 1).min(text.len()),
                ));
            }
            Some((m, n, lang, start))
                if marker == Some(*m) && run >= *n && t[run..].trim().is_empty() =>
            {
                let span = *start..line.start.max(*start);
                let lang = lang.clone().or_else(|| guess_lang(&text[span.clone()]));
                out.extend(code_with_comments(text, span, lang));
                fence = None;
            }
            Some(_) => {}
            None => {
                prose_start.get_or_insert(line.start);
            }
        }
    }
    match fence {
        // Bloc jamais fermé : jusqu'à la fin.
        Some((_, _, lang, start)) if start < text.len() => {
            let lang = lang.or_else(|| guess_lang(&text[start..]));
            out.extend(code_with_comments(text, start..text.len(), lang));
        }
        _ => {
            if let Some(s) = prose_start {
                out.extend(classify(text, s..text.len()));
            }
        }
    }
    out.retain(|b| !text[b.span.clone()].trim().is_empty());
    out
}

// ---------------------------------------------------------------- classifieur de lignes

const PROSE_WORDS: &[&str] = &[
    "le", "la", "les", "de", "des", "du", "un", "une", "et", "est", "sont", "pour", "dans", "que",
    "qui", "pas", "sur", "avec", "ce", "il", "elle", "nous", "vous", "on", "the", "of", "and",
    "to", "is", "are", "in", "that", "for", "with", "this", "it", "you", "be", "can", "not", "an",
];

const CODE_STARTS: &[&str] = &[
    "fn ",
    "pub ",
    "let ",
    "const ",
    "use ",
    "impl ",
    "struct ",
    "enum ",
    "mod ",
    "def ",
    "class ",
    "import ",
    "from ",
    "return ",
    "function ",
    "var ",
    "#include",
    "#!",
    "export ",
    "package ",
    "func ",
    "if (",
    "for (",
    "while (",
    "} else",
    "elif ",
    "async ",
    "await ",
    "SELECT ",
    "INSERT ",
    "CREATE ",
    "$ ",
    "sudo ",
    "@",
    "//",
    "/*",
    "*/",
];

/// Score « code » moins score « prose » d'une ligne.
fn code_score(line: &str) -> i32 {
    let t = line.trim();
    if t.is_empty() {
        return 0;
    }
    let mut code = 0;
    if t.ends_with([';', '{', '}']) || t.starts_with('}') || t == ")" || t.ends_with("):") {
        code += 2;
    }
    if CODE_STARTS.iter().any(|k| t.starts_with(k)) {
        code += 2;
    }
    // Appel ou définition : un identifiant collé à sa parenthèse (`print(x)`, `range(10)`).
    let call = t
        .as_bytes()
        .windows(2)
        .any(|w| (w[0].is_ascii_alphanumeric() || w[0] == b'_') && w[1] == b'(');
    if call {
        code += 2;
    }
    for pat in [
        "=>", "->", "::", "==", "!=", "&&", "||", "();", "+=", " = ", "</", "/>",
    ] {
        if t.contains(pat) {
            code += 1;
        }
    }
    let symbols = t
        .chars()
        .filter(|c| {
            !c.is_alphanumeric() && !c.is_whitespace() && !matches!(c, ',' | '.' | '\'' | '’')
        })
        .count();
    if symbols * 100 / t.chars().count().max(1) >= 15 {
        code += 1;
    }
    if line.starts_with("    ") || line.starts_with('\t') {
        code += 1;
    }

    let words: Vec<String> = t
        .split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect();
    let mut prose = words
        .iter()
        .filter(|w| PROSE_WORDS.contains(&w.as_str()))
        .count() as i32;
    if words.len() >= 6 && t.ends_with(['.', '!', '?', ':']) {
        prose += 2;
    }
    code - prose
}

/// Classe les lignes d'un passage sans balisage en blocs de prose et de code.
pub fn classify(text: &str, span: Range<usize>) -> Vec<Block> {
    let ls = lines(text, span.clone());
    let scores: Vec<i32> = ls.iter().map(|l| code_score(&text[l.clone()])).collect();
    // Une ligne vide prend le type de ses voisines (un bloc de code peut contenir des lignes vides).
    let is_code: Vec<bool> = (0..ls.len())
        .map(|i| {
            if text[ls[i].clone()].trim().is_empty() {
                let prev = (0..i)
                    .rev()
                    .find(|&j| !text[ls[j].clone()].trim().is_empty());
                let next = (i + 1..ls.len()).find(|&j| !text[ls[j].clone()].trim().is_empty());
                matches!((prev, next), (Some(p), Some(n)) if scores[p] >= 2 && scores[n] >= 2)
            } else {
                scores[i] >= 2
            }
        })
        .collect();

    let mut out: Vec<Block> = Vec::new();
    let mut i = 0;
    while i < ls.len() {
        let code = is_code[i];
        let mut j = i;
        while j + 1 < ls.len() && is_code[j + 1] == code {
            j += 1;
        }
        // Une ligne de code isolée et peu marquée reste dans la prose.
        let strong = (i..=j).map(|k| scores[k]).max().unwrap_or(0) >= 3 || j > i;
        let block_span = ls[i].start..ls[j].end;
        if code && strong {
            let lang = guess_lang(&text[block_span.clone()]);
            out.extend(code_with_comments(text, block_span, lang));
        } else {
            match out.last_mut() {
                Some(Block {
                    span,
                    kind: SegmentKind::Prose,
                }) => span.end = block_span.end,
                _ => out.push(Block {
                    span: block_span,
                    kind: SegmentKind::Prose,
                }),
            }
        }
        i = j + 1;
    }
    out.retain(|b| !text[b.span.clone()].trim().is_empty());
    out
}

// ---------------------------------------------------------------- langages et commentaires

/// Langage probable d'un extrait de code, par indices caractéristiques.
pub fn guess_lang(code: &str) -> Option<String> {
    const HINTS: &[(&str, &[&str])] = &[
        (
            "rust",
            &[
                "fn ",
                "let mut ",
                "impl ",
                "pub fn",
                "&self",
                "use std",
                "-> Result",
                "#[derive",
            ],
        ),
        (
            "python",
            &[
                "def ", "import ", "self.", "elif ", "print(", "__init__", "None:",
            ],
        ),
        (
            "javascript",
            &[
                "function ",
                "const ",
                "=> {",
                "console.",
                "export ",
                "require(",
            ],
        ),
        ("c", &["#include", "int main", "printf(", "malloc(", "->"]),
        (
            "java",
            &[
                "public class",
                "System.out",
                "private ",
                "public static void",
            ],
        ),
        ("go", &["func ", "package ", ":= ", "fmt."]),
        (
            "shell",
            &["#!/bin", "echo ", "$(", "sudo ", "export ", "fi\n"],
        ),
        (
            "sql",
            &["SELECT ", "FROM ", "WHERE ", "INSERT INTO", "CREATE TABLE"],
        ),
        ("toml", &["[package]", "[dependencies]", "version = \""]),
        ("html", &["<div", "</div>", "<html", "<span"]),
    ];
    let mut best = (0, None);
    for (lang, pats) in HINTS {
        let n = pats.iter().filter(|p| code.contains(*p)).count();
        if n > best.0 {
            best = (n, Some((*lang).to_owned()));
        }
    }
    best.1
}

#[derive(Clone, Copy)]
enum CommentStyle {
    /// `//` et `/* */`
    CLike,
    /// `#`, et docstrings `"""` pour Python
    Hash {
        docstrings: bool,
    },
    /// `--`
    Dash,
    None,
}

fn comment_style(lang: Option<&str>) -> CommentStyle {
    match lang.unwrap_or("") {
        "python" | "py" => CommentStyle::Hash { docstrings: true },
        "shell" | "sh" | "bash" | "zsh" | "fish" | "ruby" | "rb" | "toml" | "yaml" | "yml"
        | "perl" | "r" | "make" | "makefile" | "dockerfile" | "conf" | "ini" | "nix" => {
            CommentStyle::Hash { docstrings: false }
        }
        "sql" | "lua" | "haskell" | "hs" | "elm" => CommentStyle::Dash,
        "json" => CommentStyle::None,
        _ => CommentStyle::CLike,
    }
}

/// Bloc de code découpé autour de ses commentaires (blocs de prose imbriqués).
fn code_with_comments(text: &str, span: Range<usize>, lang: Option<String>) -> Vec<Block> {
    let comments = find_comments(text, span.clone(), comment_style(lang.as_deref()));
    let mut out = Vec::new();
    let mut pos = span.start;
    for c in comments {
        // Un commentaire d'un seul mot (`// TODO`) n'est pas de la prose utile.
        let words = text[c.clone()]
            .split_whitespace()
            .filter(|w| w.chars().any(char::is_alphabetic))
            .count();
        if words < 2 {
            continue;
        }
        if c.start > pos {
            out.push(Block {
                span: pos..c.start,
                kind: SegmentKind::Code { lang: lang.clone() },
            });
        }
        out.push(Block {
            span: c.clone(),
            kind: SegmentKind::Prose,
        });
        pos = c.end;
    }
    if pos < span.end {
        out.push(Block {
            span: pos..span.end,
            kind: SegmentKind::Code { lang },
        });
    }
    // Un reste de code sans identifiant ni valeur (`///`, `*/`, `}`) n'est pas un bloc utile.
    out.retain(|b| match b.kind {
        SegmentKind::Code { .. } => text[b.span.clone()].chars().any(char::is_alphanumeric),
        SegmentKind::Prose => !text[b.span.clone()].trim().is_empty(),
    });
    out
}

/// Plages de texte des commentaires (sans leurs marqueurs d'ouverture), lignes de
/// commentaires consécutives fusionnées. Les chaînes `"..."` sont ignorées.
fn find_comments(text: &str, span: Range<usize>, style: CommentStyle) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let mut out: Vec<Range<usize>> = Vec::new();
    let mut i = span.start;
    let line_end = |from: usize| {
        text[from..span.end]
            .find('\n')
            .map_or(span.end, |k| from + k)
    };
    let push = |r: Range<usize>, out: &mut Vec<Range<usize>>| {
        // Fusion avec le commentaire précédent s'il n'y a que des blancs entre les deux.
        if let Some(last) = out.last_mut()
            && text[last.end..r.start]
                .trim_matches(|c: char| c.is_whitespace() || matches!(c, '/' | '#' | '-' | '!'))
                .is_empty()
        {
            last.end = r.end;
            return;
        }
        out.push(r);
    };
    while i < span.end {
        let rest = &text[i..span.end];
        let b = bytes[i];
        if b == b'"' {
            // Docstring Python ?
            if let CommentStyle::Hash { docstrings: true } = style
                && rest.starts_with("\"\"\"")
            {
                let end = rest[3..].find("\"\"\"").map_or(span.end, |k| i + 3 + k);
                push(i + 3..end, &mut out);
                i = (end + 3).min(span.end);
                continue;
            }
            // Chaîne : jusqu'au guillemet fermant non échappé, sur la même ligne.
            let mut j = i + 1;
            while j < span.end && bytes[j] != b'"' && bytes[j] != b'\n' {
                j += if bytes[j] == b'\\' { 2 } else { 1 };
            }
            i = (j + 1).min(span.end);
            continue;
        }
        let line_comment = match style {
            CommentStyle::CLike => rest.starts_with("//"),
            CommentStyle::Hash { .. } => b == b'#' && !rest.starts_with("#!"),
            CommentStyle::Dash => rest.starts_with("--"),
            CommentStyle::None => false,
        };
        if line_comment {
            let skip = rest
                .bytes()
                .take_while(|c| matches!(c, b'/' | b'#' | b'-' | b'!'))
                .count();
            let end = line_end(i);
            push(i + skip..end, &mut out);
            i = end;
            continue;
        }
        if matches!(style, CommentStyle::CLike) && rest.starts_with("/*") {
            let end = rest[2..].find("*/").map_or(span.end, |k| i + 2 + k);
            let skip = rest[2..]
                .bytes()
                .take_while(|c| *c == b'*' || *c == b'!')
                .count();
            push(i + 2 + skip..end, &mut out);
            i = (end + 2).min(span.end);
            continue;
        }
        i += text[i..].chars().next().map_or(1, char::len_utf8);
    }
    out.retain(|r| r.start < r.end);
    out
}

// ---------------------------------------------------------------- code en ligne

/// Code en ligne dans un bloc de prose : `` `extrait` `` et identifiants manifestes
/// (`read_to_string`, `std::fs`, `HashMap`, `appel()`).
pub fn inline_code(text: &str, span: Range<usize>) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let s = &text[span.clone()];
    let mut i = 0;
    while i < s.len() {
        let rest = &s[i..];
        if let Some(inner) = rest.strip_prefix('`') {
            if let Some(k) = inner.find(['`', '\n'])
                && inner.as_bytes()[k] == b'`'
                && k > 0
            {
                out.push(span.start + i + 1..span.start + i + 1 + k);
                i += k + 2;
                continue;
            }
            i += 1;
            continue;
        }
        let c = rest.chars().next().expect("non vide");
        if c.is_alphanumeric() || c == '_' {
            let len = rest
                .char_indices()
                .find(|&(k, ch)| {
                    !(ch.is_alphanumeric()
                        || ch == '_'
                        || (ch == ':' && rest[k..].starts_with("::"))
                        || (ch == ':' && rest[..k].ends_with(':')))
                })
                .map_or(rest.len(), |(k, _)| k);
            let mut word = &rest[..len];
            let mut end = len;
            if rest[len..].starts_with("()") {
                end += 2;
            }
            word = word.trim_end_matches(':');
            if looks_like_code(word) || (end > len && word.len() > 1) {
                out.push(span.start + i..span.start + i + end.max(word.len()));
            }
            i += len.max(c.len_utf8());
            continue;
        }
        i += c.len_utf8();
    }
    out
}

fn looks_like_code(w: &str) -> bool {
    if w.contains("::") {
        return true;
    }
    let inner_underscore = w.trim_matches('_').contains('_');
    let chars: Vec<char> = w.chars().collect();
    // camelCase / PascalCase avec au moins une majuscule interne après une minuscule.
    let camel = chars.len() >= 4
        && chars
            .windows(2)
            .any(|p| p[0].is_lowercase() && p[1].is_uppercase());
    (inner_underscore && w.chars().any(char::is_alphabetic)) || camel
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str, format: &Format) -> Vec<(&'static str, String)> {
        blocks(text, format)
            .into_iter()
            .map(|b| {
                let k = match b.kind {
                    SegmentKind::Prose => "prose",
                    SegmentKind::Code { .. } => "code",
                };
                (k, text[b.span].trim().to_owned())
            })
            .collect()
    }

    #[test]
    fn markdown_fences() {
        let md = "Pour lire un fichier :\n\n```rust\nlet s = std::fs::read_to_string(\"a.txt\")?;\n```\n\nEnsuite on affiche.";
        let b = blocks(md, &Format::Markdown);
        assert_eq!(b.len(), 3);
        assert_eq!(
            b[1].kind,
            SegmentKind::Code {
                lang: Some("rust".into())
            }
        );
        assert!(md[b[1].span.clone()].starts_with("let s"));
        assert_eq!(
            kinds(md, &Format::Markdown)[2],
            ("prose", "Ensuite on affiche.".into())
        );
    }

    #[test]
    fn unmarked_code_in_plain_text() {
        let text = "Voici comment on fait une boucle en Python, c'est très simple.\n\nfor i in range(10):\n    total += i\n    print(total)\n\nEt voilà, le total est affiché à chaque tour de la boucle.";
        let k = kinds(text, &Format::Plain);
        assert_eq!(
            k.iter().map(|(k, _)| *k).collect::<Vec<_>>(),
            ["prose", "code", "prose"]
        );
        assert!(k[1].1.starts_with("for i in range"));
    }

    #[test]
    fn english_prose_with_code_words_stays_prose() {
        // Mots qu'on trouve aussi dans du code (map, read, return, string, for, in, if),
        // et même une notation d'appel : la syntaxe et les mots outils l'emportent.
        let text = "We read the map and return home before night.\nThe function f(x) returns a string for each value in the list.\nIf you want, you can use it in your own work.";
        let b = blocks(text, &Format::Plain);
        assert_eq!(b.len(), 1, "{:?}", kinds(text, &Format::Plain));
        assert_eq!(b[0].kind, SegmentKind::Prose);
        assert!(inline_code(text, 0..text.len()).is_empty());
        let fr = "Le chemin le plus court passe par la carte : on lit, on compare, on décide.";
        assert_eq!(blocks(fr, &Format::Plain)[0].kind, SegmentKind::Prose);
    }

    #[test]
    fn comments_are_prose_inside_code() {
        let src = "/// Lit le fichier de configuration et renvoie son contenu.\npub fn load(path: &str) -> String {\n    let s = \"// pas un commentaire\";\n    std::fs::read_to_string(path).unwrap() // TODO\n}\n";
        let k = kinds(
            src,
            &Format::Code {
                lang: Some("rust".into()),
            },
        );
        assert_eq!(
            k[0],
            (
                "prose",
                "Lit le fichier de configuration et renvoie son contenu.".into()
            )
        );
        assert_eq!(k.len(), 2, "{k:?}"); // « // TODO » et la chaîne ne sont pas de la prose
        assert_eq!(k[1].0, "code");
    }

    #[test]
    fn python_docstrings_and_hash_comments() {
        let src = "def f(x):\n    \"\"\"Calcule le carré du nombre donné.\"\"\"\n    # on multiplie le nombre par lui-même\n    return x * x\n";
        let k = kinds(
            src,
            &Format::Code {
                lang: Some("python".into()),
            },
        );
        let prose: Vec<_> = k
            .iter()
            .filter(|(k, _)| *k == "prose")
            .map(|(_, t)| t.as_str())
            .collect();
        assert_eq!(
            prose,
            [
                "Calcule le carré du nombre donné.",
                "on multiplie le nombre par lui-même"
            ]
        );
    }

    #[test]
    fn inline_code_in_prose() {
        let t =
            "Utilisez `HashMap` ou appelez read_to_string() puis std::fs::write, comme en Rust.";
        let found: Vec<&str> = inline_code(t, 0..t.len())
            .into_iter()
            .map(|r| &t[r])
            .collect();
        assert_eq!(found, ["HashMap", "read_to_string()", "std::fs::write"]);
        assert!(inline_code("Le Président de la République.", 0..30).is_empty());
    }

    #[test]
    fn guesses_languages() {
        assert_eq!(
            guess_lang("fn main() {\n    let mut x = 1;\n}").as_deref(),
            Some("rust")
        );
        assert_eq!(
            guess_lang("def f(self):\n    return None").as_deref(),
            Some("python")
        );
        assert_eq!(guess_lang("bonjour"), None);
    }
}
