//! Détection de la langue d'un bloc de prose (lingua), et racinisation (Snowball) pour les
//! langues sans lexique.

use lingua::{Language, LanguageDetector, LanguageDetectorBuilder};
use nknown_core::lang::Lang;
use rust_stemmers::{Algorithm, Stemmer};

/// Langues reconnues. Garder la liste courte : chaque langue ajoute un modèle au binaire,
/// et moins de candidats rend la détection plus sûre sur les textes courts.
const LANGUAGES: [Language; 7] = [
    Language::French,
    Language::English,
    Language::German,
    Language::Spanish,
    Language::Italian,
    Language::Portuguese,
    Language::Dutch,
];

/// En dessous, un bloc est trop court pour qu'on se fie à la détection.
const MIN_WORDS: usize = 4;
/// Confiance minimale (0..1) de la langue retenue.
const MIN_CONFIDENCE: f64 = 0.5;

pub struct LangDetector {
    detector: LanguageDetector,
}

impl std::fmt::Debug for LangDetector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LangDetector").finish_non_exhaustive()
    }
}

impl Default for LangDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl LangDetector {
    pub fn new() -> Self {
        Self {
            detector: LanguageDetectorBuilder::from_languages(&LANGUAGES).build(),
        }
    }

    /// Langues que ce détecteur peut renvoyer.
    pub fn languages() -> Vec<Lang> {
        LANGUAGES.iter().filter_map(|l| to_lang(*l)).collect()
    }

    /// Langue de `text`, si le texte est assez long et la détection assez sûre.
    pub fn detect(&self, text: &str) -> Option<Lang> {
        let words = text
            .split_whitespace()
            .filter(|w| w.chars().any(char::is_alphabetic))
            .count();
        if words < MIN_WORDS {
            return None;
        }
        let (language, confidence) = self
            .detector
            .compute_language_confidence_values(text)
            .into_iter()
            .next()?;
        (confidence >= MIN_CONFIDENCE)
            .then(|| to_lang(language))
            .flatten()
    }
}

fn to_lang(l: Language) -> Option<Lang> {
    Lang::from_code(&l.iso_code_639_3().to_string())
}

/// Racine d'un mot (Snowball) pour les langues prises en charge ; `None` sinon.
pub fn stem(lang: Lang, word: &str) -> Option<String> {
    let algorithm = match lang.as_str() {
        "fra" => Algorithm::French,
        "eng" => Algorithm::English,
        "deu" => Algorithm::German,
        "spa" => Algorithm::Spanish,
        "ita" => Algorithm::Italian,
        "por" => Algorithm::Portuguese,
        "nld" => Algorithm::Dutch,
        "dan" => Algorithm::Danish,
        "swe" => Algorithm::Swedish,
        "nob" => Algorithm::Norwegian,
        "fin" => Algorithm::Finnish,
        "ron" => Algorithm::Romanian,
        "rus" => Algorithm::Russian,
        "tur" => Algorithm::Turkish,
        "ell" => Algorithm::Greek,
        "hun" => Algorithm::Hungarian,
        "ara" => Algorithm::Arabic,
        _ => return None,
    };
    Some(
        Stemmer::create(algorithm)
            .stem(&word.to_lowercase())
            .into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_blocks() {
        let d = LangDetector::new();
        assert_eq!(
            d.detect("Lit le fichier de configuration et renvoie son contenu."),
            Some(Lang::FRA)
        );
        assert_eq!(
            d.detect("Reads the configuration file and returns its content."),
            Some(Lang::ENG)
        );
        assert_eq!(
            d.detect("Liest die Konfigurationsdatei und gibt ihren Inhalt zurück."),
            Lang::from_code("deu")
        );
        assert_eq!(d.detect("TODO fix"), None, "trop court");
        assert!(LangDetector::languages().contains(&Lang::ENG));
    }

    #[test]
    fn stems() {
        for w in ["returns", "returned", "returning"] {
            assert_eq!(stem(Lang::ENG, w).as_deref(), Some("return"));
        }
        assert_eq!(stem(Lang::from_code("xxx").unwrap(), "mot"), None);
    }
}
