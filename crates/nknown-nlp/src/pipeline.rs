//! Assemblage des couches en un [`Analyzer`].
//!
//! 1. découpage en tokens, séparation des élisions inconnues du lexique (« l'actu ») ;
//! 2. découpage en phrases ;
//! 3. par phrase : catégories (tagueur s'il y en a un, sinon lecture la plus fréquente),
//!    lemme et traits tirés de la lecture du lexique qui correspond à la catégorie ;
//! 4. rattachement des adjectifs.

use std::collections::HashMap;
use std::ops::Range;

use nknown_core::analyze::{Analysis, Analyzer, Feats, Pos, Relation, Token};
use nknown_core::lang::Lang;

use crate::attach::{self, AttachRules, Word};
use crate::lexicon::{LexEntry, Lexicon, normalize};
use crate::tagger::Tagger;
use crate::tokenize;

/// Pipeline multilingue. Une langue sans lexique ni tagueur reste analysable,
/// en mode dégradé (mots en minuscules, catégorie inconnue).
#[derive(Debug, Default)]
pub struct Pipeline {
    lexicons: HashMap<Lang, Lexicon>,
    taggers: HashMap<Lang, Tagger>,
}

impl Pipeline {
    pub fn with_lexicon(mut self, lang: Lang, lexicon: Lexicon) -> Self {
        self.lexicons.insert(lang, lexicon);
        self
    }

    pub fn with_tagger(mut self, lang: Lang, tagger: Tagger) -> Self {
        self.taggers.insert(lang, tagger);
        self
    }

    pub fn lexicon(&self, lang: Lang) -> Option<&Lexicon> {
        self.lexicons.get(&lang)
    }

    /// Analyse une phrase déjà découpée en tokens (indices des relations relatifs à `words`).
    pub fn annotate(&self, lang: Lang, words: &[&str]) -> (Vec<Word>, Vec<Relation>) {
        let lexicon = self.lexicons.get(&lang);
        let tags = self.taggers.get(&lang).map(|t| t.tag(words, lexicon));
        let annotated: Vec<Word> = words
            .iter()
            .enumerate()
            .map(|(i, &form)| {
                let entries: Vec<LexEntry<'_>> = lexicon
                    .map(|l| l.lookup(form).collect())
                    .unwrap_or_default();
                match &tags {
                    Some(tags) => with_tag(form, &entries, tags[i]),
                    None => without_tag(words, i, &entries),
                }
            })
            .collect();
        let relations = AttachRules::for_lang(lang)
            .map(|rules| attach::attach(&annotated, &rules))
            .unwrap_or_default();
        (annotated, relations)
    }
}

impl Analyzer for Pipeline {
    fn supports(&self, _lang: Lang) -> bool {
        true
    }

    fn analyze(&self, lang: Lang, text: &str) -> Analysis {
        let lexicon = self.lexicons.get(&lang);
        let mut spans = Vec::new();
        for span in tokenize::tokens(text) {
            if tokenize::is_word(&text[span.clone()]) {
                split_word(lexicon, text, span, &mut spans);
            } else {
                spans.push(span);
            }
        }

        let mut analysis = Analysis::default();
        let mut start = 0;
        while start < spans.len() {
            let end = spans[start..]
                .iter()
                .position(|r| tokenize::ends_sentence(&text[r.clone()]))
                .map_or(spans.len(), |k| start + k + 1);
            let words: Vec<&str> = spans[start..end].iter().map(|r| &text[r.clone()]).collect();
            let (annotated, relations) = self.annotate(lang, &words);
            let offset = analysis.tokens.len();
            analysis
                .tokens
                .extend(
                    spans[start..end]
                        .iter()
                        .zip(annotated)
                        .map(|(span, w)| Token {
                            span: span.clone(),
                            lemma: w.lemma,
                            pos: w.pos,
                            feats: w.feats,
                        }),
                );
            analysis.relations.extend(relations.into_iter().map(
                |Relation::Qualifies { adj, noun }| Relation::Qualifies {
                    adj: adj + offset,
                    noun: noun + offset,
                },
            ));
            start = end;
        }
        analysis
    }
}

/// Ajoute le mot `span` à `out`. Si la forme entière est inconnue, sépare les élisions
/// (« l'actu » → « l' » + « actu ») et détache les joints finaux (apostrophe fermante, tiret).
fn split_word(
    lexicon: Option<&Lexicon>,
    text: &str,
    mut span: Range<usize>,
    out: &mut Vec<Range<usize>>,
) {
    let known =
        |r: &Range<usize>| lexicon.is_some_and(|l| l.lookup(&text[r.clone()]).next().is_some());
    loop {
        if known(&span) {
            out.push(span);
            return;
        }
        let form = &text[span.clone()];
        let cut = form
            .char_indices()
            .find(|&(_, c)| matches!(c, '\'' | '\u{2019}'))
            .map(|(i, c)| i + c.len_utf8());
        match cut {
            Some(cut) if cut < form.len() => {
                out.push(span.start..span.start + cut);
                span.start += cut;
            }
            _ => {
                let kept = form.trim_end_matches(tokenize::is_joiner).len();
                if kept > 0 && kept < form.len() {
                    out.push(span.start..span.start + kept);
                    out.push(span.start + kept..span.end);
                } else {
                    out.push(span);
                }
                return;
            }
        }
    }
}

/// Catégories assez proches pour que la lecture du lexique fournisse lemme et traits
/// quand le tagueur et le lexique ne suivent pas la même convention.
fn similar(a: Pos, b: Pos) -> bool {
    use Pos::*;
    matches!(
        (a, b),
        (Aux, Verb)
            | (Verb, Aux)
            | (Det, Pron)
            | (Pron, Det)
            | (Verb, Adj)
            | (Adj, Verb)
            | (Det, Num)
            | (Num, Det)
            | (Cconj, Sconj)
            | (Sconj, Cconj)
            | (Noun, Propn)
            | (Propn, Noun)
    )
}

fn with_tag(form: &str, entries: &[LexEntry<'_>], tag: Pos) -> Word {
    let entry = entries
        .iter()
        .find(|e| e.pos == tag)
        .or_else(|| entries.iter().find(|e| similar(e.pos, tag)));
    match entry {
        Some(e) => Word {
            lemma: e.lemma.to_owned(),
            pos: tag,
            feats: e.feats,
        },
        None => Word {
            lemma: if tag == Pos::Propn {
                form.to_owned()
            } else {
                normalize(form)
            },
            pos: tag,
            feats: Feats::default(),
        },
    }
}

/// Sans tagueur : lecture la plus fréquente, sauf pour un mot à majuscule en milieu
/// de phrase, où l'on préfère un nom propre (« Paris » plutôt que « pari »).
fn without_tag(words: &[&str], i: usize, entries: &[LexEntry<'_>]) -> Word {
    let form = words[i];
    if !tokenize::is_word(form) {
        return Word {
            lemma: form.to_owned(),
            pos: Pos::Punct,
            feats: Feats::default(),
        };
    }
    let mid_sentence = words[..i].iter().any(|w| tokenize::is_word(w));
    let capitalized = form.starts_with(char::is_uppercase);
    let entry = if capitalized && mid_sentence {
        entries
            .iter()
            .find(|e| e.pos == Pos::Propn)
            .or(entries.first())
    } else {
        entries.first()
    };
    match entry {
        Some(e) => Word {
            lemma: e.lemma.to_owned(),
            pos: e.pos,
            feats: e.feats,
        },
        None => Word {
            lemma: normalize(form),
            pos: Pos::X,
            feats: Feats::default(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexicon::LexiconBuilder;

    fn lemmas(pipeline: &Pipeline, text: &str) -> Vec<String> {
        pipeline
            .analyze(Lang::FRA, text)
            .tokens
            .into_iter()
            .map(|t| t.lemma)
            .collect()
    }

    #[test]
    fn lemmatizes_known_forms() {
        let mut b = LexiconBuilder::new();
        b.insert("courent", "courir", Pos::Verb, Feats::default());
        let pipeline = Pipeline::default().with_lexicon(Lang::FRA, b.build().unwrap());

        let a = pipeline.analyze(Lang::FRA, "Ils courent.");
        let got: Vec<_> = a.tokens.iter().map(|t| (t.lemma.as_str(), t.pos)).collect();
        assert_eq!(
            got,
            [("ils", Pos::X), ("courir", Pos::Verb), (".", Pos::Punct)]
        );
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
                ("'", "'"),
                ("actu", "actu"),
                ("'", "'"),
            ]
        );
    }

    #[test]
    fn prefers_proper_nouns_mid_sentence() {
        let mut b = LexiconBuilder::new();
        b.insert_with_frequency("paris", "pari", Pos::Noun, Feats::default(), 10.0);
        b.insert("Paris", "Paris", Pos::Propn, Feats::default());
        let pipeline = Pipeline::default().with_lexicon(Lang::FRA, b.build().unwrap());
        assert_eq!(
            lemmas(&pipeline, "à Paris. Paris"),
            ["à", "Paris", ".", "pari"]
        );
        assert_eq!(lemmas(&pipeline, "des paris"), ["des", "pari"]);
    }

    #[test]
    fn relations_are_offset_per_sentence() {
        use nknown_core::analyze::{Gender, Number};
        let mut b = LexiconBuilder::new();
        let fem_pl = Feats {
            gender: Some(Gender::Fem),
            number: Some(Number::Plur),
        };
        b.insert("rues", "rue", Pos::Noun, fem_pl);
        b.insert("calmes", "calme", Pos::Adj, fem_pl);
        let pipeline = Pipeline::default().with_lexicon(Lang::FRA, b.build().unwrap());
        let a = pipeline.analyze(Lang::FRA, "Ah. rues calmes.");
        assert_eq!(a.relations, [Relation::Qualifies { adj: 3, noun: 2 }]);
    }
}
