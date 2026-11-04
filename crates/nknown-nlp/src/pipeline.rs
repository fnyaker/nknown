//! Assemblage des couches en un [`Analyzer`].
//!
//! 1. découpage en tokens, séparation des élisions inconnues du lexique (« l'actu ») ;
//! 2. découpage en phrases ;
//! 3. par phrase : catégories (tagueur s'il y en a un, sinon lecture la plus fréquente),
//!    lemme et traits tirés de la lecture du lexique qui correspond à la catégorie ;
//! 4. rattachement des adjectifs.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use nknown_core::analyze::{Analysis, Analyzer, Domain, Feats, Pos, Relation, Token};
use nknown_core::lang::Lang;

use crate::attach::{self, AttachRules, Word};
use crate::lexicon::{LexEntry, Lexicon, normalize};
use crate::parser::{ParsedSentence, SentenceParser};
use crate::relations::Relations;
use crate::tagger::Tagger;
use crate::tokenize;

/// Pipeline multilingue. Une langue sans lexique ni tagueur reste analysable,
/// en mode dégradé (mots en minuscules, catégorie inconnue).
#[derive(Default)]
pub struct Pipeline {
    lexicons: HashMap<Lang, Lexicon>,
    taggers: HashMap<Lang, Tagger>,
    parsers: HashMap<Lang, Arc<dyn SentenceParser>>,
    relations: HashMap<Lang, Relations>,
}

impl std::fmt::Debug for Pipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pipeline")
            .field("lexicons", &self.lexicons.keys().collect::<Vec<_>>())
            .field("taggers", &self.taggers.keys().collect::<Vec<_>>())
            .field("parsers", &self.parsers.keys().collect::<Vec<_>>())
            .field("relations", &self.relations.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Analyse d'une phrase : mots annotés et relations (indices relatifs à la phrase).
pub type Annotated = (Vec<Word>, Vec<Relation>);

impl Pipeline {
    pub fn with_lexicon(mut self, lang: Lang, lexicon: Lexicon) -> Self {
        self.lexicons.insert(lang, lexicon);
        self
    }

    /// Liens de sens entre lemmes (`<langue>.rel`), pour élargir les requêtes.
    pub fn with_relations(mut self, lang: Lang, relations: Relations) -> Self {
        self.relations.insert(lang, relations);
        self
    }

    pub fn relations(&self, lang: Lang) -> Option<&Relations> {
        self.relations.get(&lang)
    }

    pub fn with_tagger(mut self, lang: Lang, tagger: Tagger) -> Self {
        self.taggers.insert(lang, tagger);
        self
    }

    /// Couche 3 : analyseur syntaxique complet. Prioritaire sur le tagueur et les règles
    /// de rattachement quand il sait analyser la phrase.
    pub fn with_parser(mut self, lang: Lang, parser: Box<dyn SentenceParser>) -> Self {
        self.parsers.insert(lang, Arc::from(parser));
        self
    }

    /// Un même analyseur multilingue pour plusieurs langues.
    pub fn with_shared_parser(mut self, lang: Lang, parser: Arc<dyn SentenceParser>) -> Self {
        self.parsers.insert(lang, parser);
        self
    }

    /// Langues dotées d'un lexique.
    pub fn lexicon_langs(&self) -> Vec<Lang> {
        let mut langs: Vec<Lang> = self.lexicons.keys().copied().collect();
        langs.sort();
        langs
    }

    pub fn lexicon(&self, lang: Lang) -> Option<&Lexicon> {
        self.lexicons.get(&lang)
    }

    /// Analyse un lot de phrases déjà découpées en tokens.
    pub fn annotate(&self, lang: Lang, sentences: &[Vec<&str>]) -> Vec<Annotated> {
        self.annotate_with(lang, sentences, true)
    }

    fn annotate_with(&self, lang: Lang, sentences: &[Vec<&str>], parse: bool) -> Vec<Annotated> {
        let lexicon = self.lexicons.get(&lang);
        let parsed = match self.parsers.get(&lang) {
            Some(parser) if parse && parser.supports(lang) => parser.parse(lang, sentences),
            _ => vec![None; sentences.len()],
        };
        sentences
            .iter()
            .zip(parsed)
            .map(|(words, parsed)| match parsed {
                Some(p) if p.upos.len() == words.len() => from_parse(lexicon, words, &p),
                _ => self.annotate_without_parser(lang, lexicon, words),
            })
            .collect()
    }

    fn annotate_without_parser(
        &self,
        lang: Lang,
        lexicon: Option<&Lexicon>,
        words: &[&str],
    ) -> Annotated {
        let tags = self.taggers.get(&lang).map(|t| t.tag(words, lexicon));
        // Langue sans ressources : racines (Snowball) plutôt que formes brutes, pour que
        // « returns », « returned » et « returning » se rejoignent.
        if lexicon.is_none() && tags.is_none() {
            let annotated = words
                .iter()
                .map(|&form| Word {
                    lemma: if tokenize::is_word(form) {
                        crate::langid::stem(lang, form).unwrap_or_else(|| normalize(form))
                    } else {
                        form.to_owned()
                    },
                    pos: if tokenize::is_word(form) {
                        Pos::X
                    } else {
                        Pos::Punct
                    },
                    feats: Feats::default(),
                })
                .collect();
            return (annotated, Vec::new());
        }
        let annotated: Vec<Word> = words
            .iter()
            .enumerate()
            .map(|(i, &form)| {
                let entries = lookup_all(lexicon, form);
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

fn lookup_all<'a>(lexicon: Option<&'a Lexicon>, form: &str) -> Vec<LexEntry<'a>> {
    lexicon
        .map(|l| l.lookup(form).collect())
        .unwrap_or_default()
}

/// Mots et relations à partir de l'analyse de la couche 3 : catégorie et dépendances
/// de l'analyseur, lemme du lexique, traits de l'analyseur à défaut de ceux du lexique.
fn from_parse(lexicon: Option<&Lexicon>, words: &[&str], p: &ParsedSentence) -> Annotated {
    let annotated: Vec<Word> = words
        .iter()
        .enumerate()
        .map(|(i, &form)| {
            let mut w = with_tag(form, &lookup_all(lexicon, form), p.upos[i]);
            let predicted = p.feats[i];
            w.feats.gender = predicted.gender.or(w.feats.gender);
            w.feats.number = predicted.number.or(w.feats.number);
            w
        })
        .collect();
    let mut relations = Vec::new();
    for (i, (&head, label)) in p.heads.iter().zip(&p.deprels).enumerate() {
        if let Some(noun) = head
            && (label == "amod" || label.starts_with("amod:"))
        {
            relations.push(Relation::Qualifies { adj: i, noun });
        }
        relations.push(Relation::Dependency {
            dep: i,
            head,
            label: label.clone(),
        });
    }
    (annotated, relations)
}

impl Pipeline {
    /// Analyse sans l'analyseur neuronal : lexique et tagueur seulement. Pour un texte court
    /// et interactif (une requête), où un aller-retour vers le GPU coûterait plus que tout le
    /// reste.
    pub fn analyze_light(&self, lang: Lang, text: &str) -> Analysis {
        self.analyze_with(lang, text, false)
    }

    fn analyze_with(&self, lang: Lang, text: &str, parse: bool) -> Analysis {
        let lexicon = self.lexicons.get(&lang);
        let mut spans = Vec::new();
        for span in tokenize::tokens(text) {
            if tokenize::is_word(&text[span.clone()]) {
                split_word(lexicon, text, span, &mut spans);
            } else {
                spans.push(span);
            }
        }

        let mut bounds = Vec::new();
        let mut start = 0;
        while start < spans.len() {
            let end = spans[start..]
                .iter()
                .position(|r| tokenize::ends_sentence(&text[r.clone()]))
                .map_or(spans.len(), |k| start + k + 1);
            bounds.push(start..end);
            start = end;
        }
        let sentences: Vec<Vec<&str>> = bounds
            .iter()
            .map(|b| spans[b.clone()].iter().map(|r| &text[r.clone()]).collect())
            .collect();

        let mut analysis = Analysis::default();
        for (bound, (annotated, relations)) in bounds
            .into_iter()
            .zip(self.annotate_with(lang, &sentences, parse))
        {
            let offset = analysis.tokens.len();
            analysis
                .tokens
                .extend(spans[bound].iter().zip(annotated).map(|(span, w)| Token {
                    span: span.clone(),
                    lemma: w.lemma,
                    pos: w.pos,
                    feats: w.feats,
                    domain: Domain::Prose,
                }));
            analysis
                .relations
                .extend(relations.into_iter().map(|r| r.shifted(offset)));
        }
        analysis
    }
}

impl Analyzer for Pipeline {
    fn supports(&self, _lang: Lang) -> bool {
        true
    }

    fn analyze(&self, lang: Lang, text: &str) -> Analysis {
        self.analyze_with(lang, text, true)
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
