//! Pipeline NLP en trois couches :
//! 1. lexique (formes → lemme, catégorie, traits, fréquence), compilé depuis Wiktextract
//!    et Lexique ;
//! 2. tagueur perceptron (catégories en contexte) + rattachement des adjectifs par accord ;
//! 3. analyseur neuronal optionnel (candle) — pas encore implémenté.

pub mod attach;
pub mod conllu;
pub mod eval;
pub mod import;
pub mod lexicon;
pub mod pipeline;
pub mod tagger;
pub mod tokenize;

pub use lexicon::{LexEntry, Lexicon, LexiconBuilder, LexiconError};
pub use pipeline::Pipeline;
pub use tagger::{Tagger, TrainingSentence};
