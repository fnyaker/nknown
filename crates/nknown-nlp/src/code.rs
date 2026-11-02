//! Analyse d'un bloc de code : termes de recherche, sans lemmatisation.
//!
//! Pour `std::fs::read_to_string(path)` :
//! - le chemin complet `std::fs::read_to_string` ;
//! - chaque identifiant : `std`, `fs`, `read_to_string`, `path` ;
//! - les morceaux des identifiants composés : `read`, `string` (snake_case), `hash`, `map`
//!   (`HashMap`, camelCase) — `to` est trop court pour être utile.
//!
//! Les mots-clés de syntaxe (`fn`, `let`, `def`, `return`...) ne sont pas indexés.
//! Tous les termes sont en minuscules.

use std::ops::Range;

use nknown_core::analyze::{Domain, Feats, Pos, Token};

const KEYWORDS: &[&str] = &[
    // Rust
    "fn",
    "let",
    "mut",
    "pub",
    "use",
    "mod",
    "impl",
    "struct",
    "enum",
    "trait",
    "where",
    "self",
    "return",
    "if",
    "else",
    "for",
    "while",
    "loop",
    "match",
    "as",
    "in",
    "ref",
    "const",
    "static",
    "type",
    "unsafe",
    "async",
    "await",
    "move",
    "crate",
    "super",
    "dyn",
    "true",
    "false",
    // Python, JS, C, Java, Go, shell, SQL
    "def",
    "class",
    "import",
    "from",
    "pass",
    "lambda",
    "yield",
    "with",
    "try",
    "except",
    "finally",
    "raise",
    "not",
    "and",
    "or",
    "is",
    "none",
    "null",
    "nil",
    "function",
    "var",
    "this",
    "extends",
    "implements",
    "interface",
    "package",
    "private",
    "protected",
    "public",
    "void",
    "func",
    "echo",
    "then",
    "fi",
    "do",
    "done",
    "esac",
    "case",
    "break",
    "continue",
    "default",
    "switch",
    "throw",
    "throws",
    "catch",
    "elif",
    "end",
    "select",
    "where",
    "and",
];

/// Termes de code de `text[span]`, positions absolues, triés par position.
pub fn tokens(text: &str, span: Range<usize>) -> Vec<Token> {
    let s = &text[span.clone()];
    let mut out = Vec::new();
    let ident_start = |c: char| c.is_alphabetic() || c == '_';
    let ident_char = |c: char| c.is_alphanumeric() || c == '_';

    let mut i = 0;
    while i < s.len() {
        let c = s[i..].chars().next().expect("non vide");
        if !ident_start(c) {
            i += c.len_utf8();
            continue;
        }
        // Un chemin : identifiants reliés par `::` ou `.`.
        let mut parts: Vec<Range<usize>> = Vec::new();
        let mut j = i;
        loop {
            let len = s[j..]
                .char_indices()
                .find(|&(_, ch)| !ident_char(ch))
                .map_or(s.len() - j, |(k, _)| k);
            parts.push(j..j + len);
            j += len;
            let sep = if s[j..].starts_with("::") {
                2
            } else if s[j..].starts_with('.') {
                1
            } else {
                0
            };
            if sep > 0 && s[j + sep..].chars().next().is_some_and(ident_start) {
                j += sep;
            } else {
                break;
            }
        }
        let abs = |r: &Range<usize>| span.start + r.start..span.start + r.end;
        if parts.len() > 1 {
            let whole = parts[0].start..parts[parts.len() - 1].end;
            out.push(term(abs(&whole), &s[whole.clone()]));
        }
        for p in &parts {
            let ident = &s[p.clone()];
            let lower = ident.to_lowercase();
            if lower.chars().count() < 2 || KEYWORDS.contains(&lower.as_str()) {
                continue;
            }
            out.push(term(abs(p), ident));
            let pieces = split_identifier(ident);
            if pieces.len() > 1 {
                for piece in pieces.into_iter().filter(|w| w.chars().count() >= 3) {
                    out.push(Token {
                        lemma: piece,
                        ..term(abs(p), ident)
                    });
                }
            }
        }
        i = j;
    }
    out
}

fn term(span: Range<usize>, text: &str) -> Token {
    Token {
        span,
        lemma: text.to_lowercase(),
        pos: Pos::X,
        feats: Feats::default(),
        domain: Domain::Code,
    }
}

/// `read_to_string` → read, to, string ; `HashMap` → hash, map ; `parseHTTPRequest` →
/// parse, http, request ; `utf8Decode` → utf8, decode.
pub fn split_identifier(ident: &str) -> Vec<String> {
    let mut out = Vec::new();
    for chunk in ident.split('_').filter(|c| !c.is_empty()) {
        let chars: Vec<char> = chunk.chars().collect();
        let mut start = 0;
        for k in 1..chars.len() {
            let (prev, cur) = (chars[k - 1], chars[k]);
            let next_lower = chars.get(k + 1).is_some_and(|c| c.is_lowercase());
            // Coupure avant une majuscule qui suit une minuscule, ou avant la dernière
            // majuscule d'un sigle suivi d'un mot (HTTPRequest → HTTP | Request).
            if cur.is_uppercase()
                && (prev.is_lowercase() || prev.is_numeric() || (prev.is_uppercase() && next_lower))
            {
                out.push(chars[start..k].iter().collect::<String>().to_lowercase());
                start = k;
            }
        }
        out.push(chars[start..].iter().collect::<String>().to_lowercase());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_identifiers() {
        assert_eq!(split_identifier("read_to_string"), ["read", "to", "string"]);
        assert_eq!(split_identifier("HashMap"), ["hash", "map"]);
        assert_eq!(
            split_identifier("parseHTTPRequest"),
            ["parse", "http", "request"]
        );
        assert_eq!(split_identifier("utf8Decode"), ["utf8", "decode"]);
        assert_eq!(split_identifier("x"), ["x"]);
    }

    #[test]
    fn indexes_paths_identifiers_and_pieces() {
        let src =
            "let map: HashMap<String, u32> = HashMap::new();\nstd::fs::read_to_string(path)?;";
        let lemmas: Vec<String> = tokens(src, 0..src.len())
            .into_iter()
            .map(|t| t.lemma)
            .collect();
        for expected in [
            "map",
            "hashmap",
            "hash",
            "string",
            "u32",
            "hashmap::new",
            "new",
            "std::fs::read_to_string",
            "std",
            "fs",
            "read_to_string",
            "read",
            "path",
        ] {
            assert!(
                lemmas.contains(&expected.to_string()),
                "{expected} absent de {lemmas:?}"
            );
        }
        assert!(!lemmas.contains(&"let".to_string()));
        assert!(!lemmas.contains(&"to".to_string()));
        let toks = tokens(src, 0..src.len());
        assert!(toks.windows(2).all(|w| w[0].span.start <= w[1].span.start));
        assert!(toks.iter().all(|t| t.domain == Domain::Code));
    }
}
