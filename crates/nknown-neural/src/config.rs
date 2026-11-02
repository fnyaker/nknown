//! Configuration d'un modèle, sauvegardée en `parser.json`.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Result;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParserConfig {
    /// Configuration Hugging Face de l'encodeur (`config.json` de XLM-RoBERTa), telle quelle.
    pub encoder: serde_json::Value,
    /// Étiquettes de dépendance, dans l'ordre des sorties du modèle.
    pub deprels: Vec<String>,
    /// Dimension des projections biaffines des arcs.
    pub arc_dim: usize,
    /// Dimension des projections biaffines des étiquettes.
    pub label_dim: usize,
    /// Nombre maximal de sous-mots par phrase (au-delà, la phrase n'est pas analysée).
    pub max_subwords: usize,
    /// Langues d'entraînement (codes ISO 639-3) ; vide = toutes.
    pub langs: Vec<String>,
}

impl ParserConfig {
    pub fn encoder_config(&self) -> Result<crate::encoder::EncoderConfig> {
        Ok(serde_json::from_value(self.encoder.clone())?)
    }

    pub fn hidden_size(&self) -> usize {
        self.encoder["hidden_size"].as_u64().unwrap_or(768) as usize
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        std::fs::write(path, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        Ok(serde_json::from_slice(&std::fs::read(path)?)?)
    }
}
