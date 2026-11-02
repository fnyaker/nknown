//! Pipeline NLP en trois couches :
//! 1. lexique (formes → lemme, catégorie, traits, fréquence), compilé depuis Wiktextract
//!    et Lexique ;
//! 2. tagueur perceptron (catégories en contexte) + rattachement des adjectifs par accord ;
//! 3. analyseur syntaxique complet, branché via [`parser::SentenceParser`] (crate `nknown-neural`).

pub mod attach;
pub mod conllu;
pub mod eval;
pub mod import;
pub mod lexicon;
pub mod parser;
pub mod pipeline;
pub mod tagger;
pub mod tokenize;

pub use lexicon::{LexEntry, Lexicon, LexiconBuilder, LexiconError};
pub use parser::{ParsedSentence, SentenceParser};
pub use pipeline::Pipeline;
pub use tagger::{Tagger, TrainingSentence};
