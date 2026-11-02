//! Pipeline NLP en trois couches :
//! 1. lexique (formes → lemme, catégorie, traits), compilé depuis Wiktextract ;
//! 2. tagueur perceptron + rattachement des adjectifs par accord ;
//! 3. analyseur neuronal optionnel (candle).
//!
//! Seule la couche 1 existe pour l'instant.

pub mod import;
pub mod lexicon;
pub mod pipeline;
pub mod tokenize;

pub use lexicon::{LexEntry, Lexicon, LexiconBuilder, LexiconError};
pub use pipeline::Pipeline;
