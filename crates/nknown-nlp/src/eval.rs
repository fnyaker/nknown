//! Évaluation du pipeline sur un corpus annoté UD : catégories et rattachement des adjectifs.

use std::collections::HashMap;

use nknown_core::analyze::Relation;
use nknown_core::lang::Lang;

use crate::conllu::GoldSentence;
use crate::pipeline::Pipeline;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    pub tokens: usize,
    pub correct: usize,
    /// Tokens absents du lexique.
    pub unknown: usize,
    pub unknown_correct: usize,
    /// Tokens dont le lexique propose plusieurs catégories.
    pub ambiguous: usize,
    pub ambiguous_correct: usize,
    /// Adjectifs `amod` de référence.
    pub amod: usize,
    pub amod_correct: usize,
    pub amod_wrong_head: usize,
    pub amod_missed: usize,
    /// Rattachements sur des adjectifs non `amod` dans la référence (attributs, erreurs de catégorie).
    pub other_attachments: usize,
}

fn pct(a: usize, b: usize) -> f64 {
    100.0 * a as f64 / b.max(1) as f64
}

impl Report {
    pub fn accuracy(&self) -> f64 {
        pct(self.correct, self.tokens)
    }
}

impl std::fmt::Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "catégories : {:.2} % sur {} tokens",
            self.accuracy(),
            self.tokens
        )?;
        writeln!(
            f,
            "  inconnus du lexique : {:.2} % ({} tokens)",
            pct(self.unknown_correct, self.unknown),
            self.unknown
        )?;
        writeln!(
            f,
            "  ambigus dans le lexique : {:.2} % ({} tokens)",
            pct(self.ambiguous_correct, self.ambiguous),
            self.ambiguous
        )?;
        let predicted = self.amod_correct + self.amod_wrong_head + self.other_attachments;
        writeln!(
            f,
            "adjectifs (amod) : rappel {:.2} %, précision {:.2} % ({} de référence ; {} bons, {} mauvaise tête, {} manqués, {} rattachements hors amod)",
            pct(self.amod_correct, self.amod),
            pct(self.amod_correct, predicted),
            self.amod,
            self.amod_correct,
            self.amod_wrong_head,
            self.amod_missed,
            self.other_attachments
        )
    }
}

pub fn evaluate(pipeline: &Pipeline, lang: Lang, sentences: &[GoldSentence]) -> Report {
    let lexicon = pipeline.lexicon(lang);
    let mut r = Report::default();
    for s in sentences {
        let words = s.words();
        let (annotated, relations) = pipeline.annotate(lang, &words);
        for ((form, gold), got) in words.iter().zip(&s.tokens).zip(&annotated) {
            let ok = gold.upos == got.pos;
            r.tokens += 1;
            r.correct += usize::from(ok);
            let mut cats: Vec<_> = lexicon
                .map(|l| l.lookup(form).map(|e| e.pos).collect())
                .unwrap_or_default();
            cats.dedup();
            cats.sort_by_key(|p| *p as u8);
            cats.dedup();
            if cats.is_empty() {
                r.unknown += 1;
                r.unknown_correct += usize::from(ok);
            } else if cats.len() > 1 {
                r.ambiguous += 1;
                r.ambiguous_correct += usize::from(ok);
            }
        }

        let predicted: HashMap<usize, usize> = relations
            .iter()
            .map(|&Relation::Qualifies { adj, noun }| (adj, noun))
            .collect();
        let gold: HashMap<usize, usize> = s
            .tokens
            .iter()
            .enumerate()
            .filter(|(_, t)| t.deprel == "amod" || t.deprel.starts_with("amod:"))
            .filter_map(|(i, t)| Some((i, t.head?)))
            .collect();
        r.amod += gold.len();
        for (adj, head) in &gold {
            match predicted.get(adj) {
                Some(n) if n == head => r.amod_correct += 1,
                Some(_) => r.amod_wrong_head += 1,
                None => r.amod_missed += 1,
            }
        }
        r.other_attachments += predicted.keys().filter(|a| !gold.contains_key(a)).count();
    }
    r
}
