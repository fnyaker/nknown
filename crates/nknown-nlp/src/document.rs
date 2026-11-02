//! Analyse d'un document complet : segmentation en blocs prose / code, analyse de chaque
//! bloc avec l'outil adapté, puis assemblage en une seule [`Analysis`].
//!
//! Le code en ligne dans la prose (`` `HashMap` ``, `read_to_string()`) est analysé comme du
//! code, mais la phrase garde sa structure : l'analyse linguistique voit la phrase entière,
//! puis les tokens de l'extrait de code sont remplacés par des termes de code.

use std::collections::HashMap;
use std::ops::Range;

use nknown_core::analyze::{Analysis, Analyzer, Relation, Segment, SegmentKind, Token};
use nknown_core::lang::Lang;

use crate::code;
use crate::pipeline::Pipeline;
use crate::segment::{self, Format};

pub fn analyze_document(pipeline: &Pipeline, lang: Lang, text: &str, format: &Format) -> Analysis {
    let mut out = Analysis::default();
    for block in segment::blocks(text, format) {
        let first = out.tokens.len();
        match &block.kind {
            SegmentKind::Prose => {
                let mut a = pipeline.analyze(lang, &text[block.span.clone()]);
                for t in &mut a.tokens {
                    t.span = t.span.start + block.span.start..t.span.end + block.span.start;
                }
                let inline = segment::inline_code(text, block.span.clone());
                if !inline.is_empty() {
                    a = splice_inline(a, &inline, text);
                }
                out.tokens.extend(a.tokens);
                out.relations
                    .extend(a.relations.into_iter().map(|r| r.shifted(first)));
            }
            SegmentKind::Code { .. } => out.tokens.extend(code::tokens(text, block.span.clone())),
        }
        if out.tokens.len() > first {
            out.segments.push(Segment {
                tokens: first..out.tokens.len(),
                kind: block.kind,
            });
        }
    }
    out
}

/// Remplace les tokens de prose qui recouvrent un extrait de code en ligne par les termes
/// de code de cet extrait ; les relations vers des tokens retirés disparaissent.
fn splice_inline(a: Analysis, inline: &[Range<usize>], text: &str) -> Analysis {
    let overlaps = |t: &Token, r: &Range<usize>| t.span.start < r.end && r.start < t.span.end;
    let mut tokens = Vec::with_capacity(a.tokens.len());
    let mut new_index: HashMap<usize, usize> = HashMap::new();
    let mut next_inline = 0;
    for (i, t) in a.tokens.into_iter().enumerate() {
        // Extraits qui commencent avant ce token : leurs termes de code d'abord.
        while next_inline < inline.len() && inline[next_inline].start < t.span.end {
            if inline[next_inline].end <= t.span.start || overlaps(&t, &inline[next_inline]) {
                tokens.extend(code::tokens(text, inline[next_inline].clone()));
                next_inline += 1;
            } else {
                break;
            }
        }
        if inline.iter().any(|r| overlaps(&t, r)) {
            continue;
        }
        new_index.insert(i, tokens.len());
        tokens.push(t);
    }
    for r in &inline[next_inline..] {
        tokens.extend(code::tokens(text, r.clone()));
    }
    let relations = a
        .relations
        .into_iter()
        .filter_map(|r| match r {
            Relation::Qualifies { adj, noun } => Some(Relation::Qualifies {
                adj: *new_index.get(&adj)?,
                noun: *new_index.get(&noun)?,
            }),
            Relation::Dependency { dep, head, label } => Some(Relation::Dependency {
                dep: *new_index.get(&dep)?,
                head: match head {
                    Some(h) => Some(*new_index.get(&h)?),
                    None => None,
                },
                label,
            }),
        })
        .collect();
    Analysis {
        tokens,
        relations,
        segments: a.segments,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nknown_core::analyze::Domain;

    #[test]
    fn mixed_markdown_page() {
        let md = "# Lire un fichier\n\nLa fonction `read_to_string` renvoie le contenu.\n\n```rust\n// lit la configuration depuis le disque\nlet s = std::fs::read_to_string(\"a.toml\")?;\n```\n\nC'est simple.";
        let a = analyze_document(&Pipeline::default(), Lang::FRA, md, &Format::Markdown);
        let kinds: Vec<_> = a
            .segments
            .iter()
            .map(|s| matches!(s.kind, SegmentKind::Code { .. }))
            .collect();
        // titre + phrase | commentaire du bloc | code | conclusion
        assert_eq!(kinds, [false, false, true, false], "{:?}", a.segments);
        // Le code en ligne de la prose est un terme de code.
        let first_prose = &a.tokens[a.segments[0].tokens.clone()];
        assert!(
            first_prose
                .iter()
                .any(|t| t.domain == Domain::Code && t.lemma == "read_to_string")
        );
        assert!(
            first_prose
                .iter()
                .any(|t| t.domain == Domain::Prose && t.lemma == "fonction")
        );
        // Le commentaire du bloc de code est de la prose.
        let comment = &a.tokens[a.segments[1].tokens.clone()];
        assert!(comment.iter().all(|t| t.domain == Domain::Prose));
        assert!(comment.iter().any(|t| t.lemma == "configuration"));
        // Tokens dans l'ordre du texte, segments contigus.
        assert!(
            a.tokens
                .windows(2)
                .all(|w| w[0].span.start <= w[1].span.start)
        );
        assert!(
            a.segments
                .windows(2)
                .all(|w| w[0].tokens.end == w[1].tokens.start)
        );
    }
}
