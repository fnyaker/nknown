//! Assemblage des couches en un [`Analyzer`].

use std::collections::HashMap;
use std::ops::Range;

use nknown_core::analyze::{Analysis, Analyzer, Feats, Pos, Token};
use nknown_core::lang::Lang;

use crate::lexicon::Lexicon;
use crate::tokenize;

/// Pipeline multilingue. Une langue sans lexique reste analysable,
/// en mode dégradé (mots en minuscules, catégorie inconnue).
#[derive(Debug, Default)]
pub struct Pipeline {
    lexicons: HashMap<Lang, Lexicon>,
}

impl Pipeline {
    pub fn with_lexicon(mut self, lang: Lang, lexicon: Lexicon) -> Self {
        self.lexicons.insert(lang, lexicon);
        self
    }
}

impl Analyzer for Pipeline {
    fn supports(&self, _lang: Lang) -> bool {
        true
    }

    fn analyze(&self, lang: Lang, text: &str) -> Analysis {
        let lexicon = self.lexicons.get(&lang);
        let mut tokens = Vec::new();
        for span in tokenize::words(text) {
            push_word(lexicon, text, span, &mut tokens);
        }
        Analysis {
            tokens,
            relations: Vec::new(),
        }
    }
}

/// Ajoute le mot `span` à `out`. Si la forme entière est inconnue, sépare les élisions
/// (« l'actu » → « l' » + « actu ») et retire les joints finaux (apostrophe fermante, tiret).
fn push_word(lexicon: Option<&Lexicon>, text: &str, mut span: Range<usize>, out: &mut Vec<Token>) {
    loop {
        if let Some(t) = lookup(lexicon, text, span.clone()) {
            out.push(t);
            return;
        }
        let form = &text[span.clone()];
        let cut = form
            .char_indices()
            .find(|&(_, c)| matches!(c, '\'' | '\u{2019}'))
            .map(|(i, c)| i + c.len_utf8());
        match cut {
            Some(cut) if cut < form.len() => {
                let head = span.start..span.start + cut;
                out.push(
                    lookup(lexicon, text, head.clone()).unwrap_or_else(|| unknown(text, head)),
                );
                span.start += cut;
            }
            _ => {
                let trimmed = form.trim_end_matches(tokenize::is_joiner);
                if !trimmed.is_empty() && trimmed.len() < form.len() {
                    span.end = span.start + trimmed.len();
                    continue;
                }
                out.push(unknown(text, span));
                return;
            }
        }
    }
}

/// Sans couche 2, on retient la première analyse du lexique.
fn lookup(lexicon: Option<&Lexicon>, text: &str, span: Range<usize>) -> Option<Token> {
    let e = lexicon?.lookup(&text[span.clone()]).next()?;
    Some(Token {
        span,
        lemma: e.lemma.to_owned(),
        pos: e.pos,
        feats: e.feats,
    })
}

fn unknown(text: &str, span: Range<usize>) -> Token {
    Token {
        lemma: text[span.clone()].to_lowercase(),
        span,
        pos: Pos::X,
        feats: Feats::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexicon::LexiconBuilder;

    #[test]
    fn lemmatizes_known_forms() {
        let mut b = LexiconBuilder::new();
        b.insert("courent", "courir", Pos::Verb, Feats::default());
        let pipeline = Pipeline::default().with_lexicon(Lang::FRA, b.build().unwrap());

        let a = pipeline.analyze(Lang::FRA, "Ils courent");
        assert_eq!(a.tokens[0].lemma, "ils");
        assert_eq!(a.tokens[0].pos, Pos::X);
        assert_eq!(a.tokens[1].lemma, "courir");
        assert_eq!(a.tokens[1].pos, Pos::Verb);
    }

    #[test]
    fn splits_elisions_only_when_needed() {
        let mut b = LexiconBuilder::new();
        b.insert("l'", "le", Pos::Det, Feats::default());
        b.insert("actu", "actu", Pos::Noun, Feats::default());
        b.insert("aujourd'hui", "aujourd'hui", Pos::Adv, Feats::default());
        let pipeline = Pipeline::default().with_lexicon(Lang::FRA, b.build().unwrap());

        let text = "L\u{2019}actu d'aujourd'hui 'actu'";
        let a = pipeline.analyze(Lang::FRA, text);
        let got: Vec<_> = a
            .tokens
            .iter()
            .map(|t| (&text[t.span.clone()], t.lemma.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("L\u{2019}", "le"),
                ("actu", "actu"),
                ("d'", "d'"),
                ("aujourd'hui", "aujourd'hui"),
                ("actu", "actu"),
            ]
        );
    }
}
