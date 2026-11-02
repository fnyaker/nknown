//! Assemblage des couches en un [`Analyzer`].

use std::collections::HashMap;

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
        let tokens = tokenize::words(text)
            .into_iter()
            .map(|span| {
                let form = &text[span.clone()];
                // Sans couche 2, on retient la première analyse du lexique.
                match lexicon.and_then(|l| l.lookup(form).first()) {
                    Some(e) => Token {
                        span,
                        lemma: e.lemma.clone(),
                        pos: e.pos,
                        feats: e.feats,
                    },
                    None => Token {
                        span,
                        lemma: form.to_lowercase(),
                        pos: Pos::X,
                        feats: Feats::default(),
                    },
                }
            })
            .collect();
        Analysis {
            tokens,
            relations: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexicon::LexEntry;

    #[test]
    fn lemmatizes_known_forms() {
        let mut lex = Lexicon::default();
        lex.insert(
            "courent",
            LexEntry {
                lemma: "courir".into(),
                pos: Pos::Verb,
                feats: Feats::default(),
            },
        );
        let pipeline = Pipeline::default().with_lexicon(Lang::FRA, lex);

        let a = pipeline.analyze(Lang::FRA, "Ils courent");
        assert_eq!(a.tokens[0].lemma, "ils");
        assert_eq!(a.tokens[0].pos, Pos::X);
        assert_eq!(a.tokens[1].lemma, "courir");
        assert_eq!(a.tokens[1].pos, Pos::Verb);
    }
}
