//! Couche 3 : analyseur syntaxique neuronal.
//!
//! Encodeur XLM-RoBERTa (multilingue) + têtes de prédiction par mot :
//! catégorie (UPOS), genre, nombre, et dépendances par attention biaffine
//! (Dozat & Manning, 2017), décodées en arbre par Chu-Liu-Edmonds.
//!
//! Le modèle est sauvegardé en safetensors avec les noms de poids de Hugging Face
//! pour l'encodeur (`encoder.layer.N...`) et `heads.*` pour les têtes : le même
//! fichier sert aux deux moteurs d'exécution (candle sur CPU, burn sur Vulkan).
//!
//! Répertoire d'un modèle : `parser.json` (configuration, étiquettes), `model.safetensors`,
//! `tokenizer.json`.

pub mod config;
pub mod data;
pub mod encoder;
pub mod error;
pub mod model;
pub mod mst;
pub mod parser;
pub mod train;
#[cfg(feature = "vulkan")]
pub mod vulkan;

pub use config::ParserConfig;
pub use error::Error;
pub use parser::NeuralParser;
pub use train::{TrainOptions, train};
