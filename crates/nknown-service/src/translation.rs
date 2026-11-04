//! Traduction en français des sources dans une autre langue : proposée, jamais imposée.
//!
//! Une source dont la plupart des documents ne sont pas en français est proposée à la
//! traduction ([`suggest`]). Acceptée, chacun de ses documents dans une langue traduisible est
//! indexé une seconde fois, traduit : même source, même lien, identifiant suffixé
//! ([`copy_id`], « `#fr<-eng` »). La copie suit l'original (même cycle de mises à jour) ; la
//! base ne change pas de format.
//!
//! Modèles : Marian (OPUS-MT), un par paire, téléchargés à la première acceptation d'une
//! langue. Le chinois passe par l'anglais (pas de modèle chinois → français).

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use nknown_api::DeviceChoice;
use nknown_core::lang::Lang;
use nknown_nlp::tokenize;

use crate::Error;

/// Langue d'arrivée.
pub const TARGET: Lang = Lang::FRA;

/// Part de documents dans une autre langue au-delà de laquelle une source est proposée.
pub const SUGGEST_SHARE: f64 = 0.6;
/// Documents minimaux pour juger de la langue d'une source.
pub const SUGGEST_MIN_DOCS: usize = 3;
/// Caractères traduits par document, au plus (coupé à une fin de paragraphe) : un article
/// encyclopédique entier coûterait des minutes.
pub const MAX_CHARS: usize = 20_000;

/// Modèles à enchaîner pour traduire `lang` en français (dossiers sous `translate/`).
pub fn chain(lang: Lang) -> Option<&'static [&'static str]> {
    Some(match lang.as_str() {
        "eng" => &["en-fr"],
        "deu" => &["de-fr"],
        "spa" => &["es-fr"],
        "ita" => &["it-fr"],
        "nld" => &["nl-fr"],
        "rus" => &["ru-fr"],
        "zho" => &["zh-en", "en-fr"],
        _ => return None,
    })
}

/// Langues traduisibles.
pub fn languages() -> Vec<Lang> {
    ["eng", "deu", "spa", "ita", "nld", "rus", "zho"]
        .into_iter()
        .filter_map(Lang::from_code)
        .collect()
}

/// Identifiant de la copie traduite d'un contenu de la source.
pub fn copy_id(external_id: &str, from: Lang) -> String {
    format!("{external_id}#fr<-{}", from.as_str())
}

/// Identifiant de l'original (le même s'il ne s'agit pas d'une copie traduite).
pub fn original_id(external_id: &str) -> &str {
    match external_id.rsplit_once("#fr<-") {
        Some((orig, from)) if Lang::from_code(from).is_some() => orig,
        _ => external_id,
    }
}

/// Langue d'origine d'une copie traduite, d'après son identifiant.
pub fn translated_from(external_id: &str) -> Option<Lang> {
    let (_, from) = external_id.rsplit_once("#fr<-")?;
    Lang::from_code(from)
}

/// Phrases d'un paragraphe, avec ce qui les sépare (rendu tel quel).
fn sentences(paragraph: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    for span in tokenize::tokens(paragraph) {
        if tokenize::ends_sentence(&paragraph[span.clone()]) {
            out.push(paragraph[start..span.end].trim());
            start = span.end;
        }
    }
    let rest = paragraph[start..].trim();
    if !rest.is_empty() {
        out.push(rest);
    }
    out.retain(|s| !s.is_empty());
    out
}

/// Texte coupé à [`MAX_CHARS`], à une fin de ligne si possible.
fn bounded(text: &str) -> &str {
    if text.len() <= MAX_CHARS {
        return text;
    }
    let mut end = MAX_CHARS;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    match text[..end].rfind('\n') {
        Some(nl) if nl > MAX_CHARS / 2 => &text[..nl],
        _ => &text[..end],
    }
}

/// Erreur des moteurs de traduction (sans eux : aucune).
mod nknown_neural_error {
    #[cfg(feature = "neural")]
    pub type E = nknown_neural::Error;
    #[cfg(not(feature = "neural"))]
    pub type E = std::convert::Infallible;
}

/// Un modèle chargé, sur CPU ou GPU.
enum Engine {
    #[cfg(feature = "neural")]
    Cpu(nknown_neural::Translator),
    #[cfg(feature = "vulkan")]
    Gpu(nknown_neural::translate_vulkan::VulkanTranslator),
}

impl Engine {
    #[cfg_attr(not(feature = "vulkan"), allow(unused_variables))]
    fn load(dir: &Path, gpu: bool) -> Result<Self, Error> {
        #[cfg(feature = "vulkan")]
        if gpu {
            return nknown_neural::translate_vulkan::VulkanTranslator::load(dir)
                .map(Engine::Gpu)
                .map_err(|e| Error::Invalid(format!("traduction (GPU) : {e}")));
        }
        #[cfg(feature = "neural")]
        {
            nknown_neural::Translator::load(dir)
                .map(Engine::Cpu)
                .map_err(|e| Error::Invalid(format!("traduction : {e}")))
        }
        #[cfg(not(feature = "neural"))]
        Err(Error::Invalid(
            "traduction indisponible : compilé sans la fonctionnalité « neural »".into(),
        ))
    }

    #[cfg_attr(not(feature = "neural"), allow(unused_variables))]
    fn translate(&self, sentences: &[&str]) -> Result<Vec<String>, Error> {
        let err = |e: nknown_neural_error::E| Error::Invalid(format!("traduction : {e}"));
        match *self {
            #[cfg(feature = "neural")]
            Engine::Cpu(ref t) => t.translate(sentences).map_err(err),
            #[cfg(feature = "vulkan")]
            Engine::Gpu(ref t) => t.translate(sentences).map_err(err),
        }
    }
}

/// Modèle chargé, partagé entre les fils (un calcul à la fois).
type Shared = Arc<Mutex<Engine>>;

/// Modèles de traduction, chargés à la demande et gardés en mémoire.
#[derive(Default)]
pub struct Translators {
    loaded: Mutex<HashMap<(String, bool), Shared>>,
}

impl std::fmt::Debug for Translators {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Translators").finish_non_exhaustive()
    }
}

/// Fichiers présents pour toute la chaîne de `lang` dans `models_dir`.
pub fn models_present(models_dir: &Path, lang: Lang) -> bool {
    chain(lang).is_some_and(|c| {
        c.iter().all(|pair| {
            nknown_files()
                .iter()
                .all(|f| models_dir.join("translate").join(pair).join(f).exists())
        })
    })
}

fn nknown_files() -> [&'static str; 4] {
    [
        "config.json",
        "model.safetensors",
        "source.spm",
        "vocab.json",
    ]
}

impl Translators {
    /// Libère les modèles (changement de périphérique, de dossier).
    pub fn clear(&self) {
        crate::lock(&self.loaded).clear();
    }

    fn engine(&self, models_dir: &Path, pair: &str, gpu: bool) -> Result<Shared, Error> {
        let key = (pair.to_owned(), gpu);
        if let Some(e) = crate::lock(&self.loaded).get(&key) {
            return Ok(e.clone());
        }
        let engine = Arc::new(Mutex::new(Engine::load(
            &models_dir.join("translate").join(pair),
            gpu,
        )?));
        crate::lock(&self.loaded).insert(key, engine.clone());
        Ok(engine)
    }

    /// Traduit `text` (de langue `from`) en français, paragraphe par paragraphe, phrase par
    /// phrase ; les sauts de ligne sont gardés.
    pub fn translate(
        &self,
        models_dir: &Path,
        device: DeviceChoice,
        from: Lang,
        text: &str,
    ) -> Result<String, Error> {
        let chain = chain(from)
            .ok_or_else(|| Error::Invalid(format!("pas de traduction depuis {}", from.as_str())))?;
        let gpu = match device {
            DeviceChoice::Cpu => false,
            DeviceChoice::Gpu => true,
            DeviceChoice::Auto => crate::models::gpu_available(),
        };
        let mut current = bounded(text).to_owned();
        for pair in chain {
            let engine = self.engine(models_dir, pair, gpu)?;
            let engine = crate::lock(&engine);
            let mut out = String::with_capacity(current.len());
            for (i, line) in current.split('\n').enumerate() {
                if i > 0 {
                    out.push('\n');
                }
                let parts = sentences(line);
                if parts.is_empty() {
                    continue;
                }
                out.push_str(&engine.translate(&parts)?.join(" "));
            }
            current = out;
        }
        Ok(current)
    }
}

/// Source proposée à la traduction : sa langue dominante, et la part de ses documents dans
/// cette langue.
#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    pub lang: Lang,
    pub docs: usize,
    pub share: f64,
}

/// Langue dominante parmi `langs` (langues des documents d'une source), si elle n'est pas le
/// français, est traduisible et dépasse [`SUGGEST_SHARE`].
pub fn suggest(langs: &[Lang]) -> Option<Suggestion> {
    if langs.len() < SUGGEST_MIN_DOCS {
        return None;
    }
    let mut count: HashMap<Lang, usize> = HashMap::new();
    for l in langs {
        *count.entry(*l).or_default() += 1;
    }
    let (lang, n) = count
        .into_iter()
        .filter(|(l, _)| *l != TARGET && chain(*l).is_some())
        .max_by_key(|(l, n)| (*n, std::cmp::Reverse(l.as_str().to_owned())))?;
    let share = n as f64 / langs.len() as f64;
    (share >= SUGGEST_SHARE).then_some(Suggestion {
        lang,
        docs: langs.len(),
        share,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_are_recognised() {
        let id = copy_id("https://example.org/a", Lang::ENG);
        assert_eq!(translated_from(&id), Some(Lang::ENG));
        assert_eq!(translated_from("https://example.org/a"), None);
    }

    #[test]
    fn sentences_are_split() {
        assert_eq!(
            sentences("First one. Second one! Third"),
            ["First one.", "Second one!", "Third"]
        );
        assert!(sentences("   ").is_empty());
    }

    #[test]
    fn long_texts_are_bounded_at_a_line() {
        let text = format!("{}\n{}", "a".repeat(MAX_CHARS - 100), "b".repeat(500));
        assert_eq!(bounded(&text).len(), MAX_CHARS - 100);
    }

    #[test]
    fn suggestions_need_a_dominant_foreign_language() {
        let en = Lang::ENG;
        let fr = Lang::FRA;
        assert_eq!(suggest(&[en, en, en, fr]).map(|s| s.lang), Some(en));
        assert!(suggest(&[en, fr, fr, fr]).is_none());
        assert!(suggest(&[en, en]).is_none());
        // Chinois : par l'anglais.
        assert_eq!(chain(Lang::from_code("zho").unwrap()).unwrap().len(), 2);
    }
}
