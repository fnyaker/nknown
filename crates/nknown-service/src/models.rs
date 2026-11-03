//! Chargement des modèles : lexiques, tagueurs, analyseur neuronal.

use std::path::Path;

use nknown_core::lang::Lang;
use nknown_nlp::{Lexicon, Pipeline, Tagger};

use crate::Error;

/// Langue d'un fichier de modèle d'après son nom (`fr.lex`, `en_ewt-ud-test.conllu`).
pub fn lang_of(path: &Path) -> Option<Lang> {
    let stem = path.file_stem()?.to_str()?;
    Lang::from_tag(stem.split(['_', '.', '-']).next()?)
}

/// Pipeline depuis `lexicon` : un fichier `.lex`, ou un dossier dont chaque `<langue>.lex` et
/// `<langue>.tag` est chargé. `models` ajoute des tagueurs (`.tag`) et des analyseurs
/// neuronaux (dossier, ou `vulkan:<dossier>` pour le GPU).
pub fn load_pipeline(lexicon: &Path, models: &[&str]) -> Result<Pipeline, Error> {
    let mut pipeline = Pipeline::default();
    if lexicon.is_dir() {
        let mut files: Vec<_> = std::fs::read_dir(lexicon)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .collect();
        files.sort();
        for f in files {
            let Some(ext) = f.extension().and_then(|e| e.to_str()) else {
                continue;
            };
            let Some(lang) = lang_of(&f) else { continue };
            match ext {
                "lex" => pipeline = pipeline.with_lexicon(lang, Lexicon::load(&f)?),
                "tag" => pipeline = pipeline.with_tagger(lang, Tagger::load(&f)?),
                _ => {}
            }
        }
    } else {
        pipeline = pipeline.with_lexicon(
            lang_of(lexicon).unwrap_or(Lang::FRA),
            Lexicon::load(lexicon)?,
        );
    }
    for spec in models {
        pipeline = add_model(pipeline, spec)?;
    }
    Ok(pipeline)
}

/// Un chemin de modèle relatif (`parser-multi`, `vulkan:parser-multi`) désigne un modèle du
/// dossier des modèles.
pub fn resolve(spec: &str, models_dir: &Path) -> String {
    let (prefix, path) = ["vulkan:", "f32:", "light:"]
        .into_iter()
        .find_map(|p| spec.strip_prefix(p).map(|rest| (p, rest)))
        .unwrap_or(("", spec));
    if Path::new(path).is_absolute() {
        spec.to_owned()
    } else {
        format!("{prefix}{}", models_dir.join(path).display())
    }
}

/// Précision de l'analyseur sur CPU.
#[derive(Debug, Clone, Copy)]
#[cfg_attr(not(feature = "neural"), allow(dead_code))]
enum Bits {
    F32,
    Standard,
    Light,
}

/// Ajoute un modèle : tagueur (fichier), analyseur GPU (`vulkan:<dossier>`) ou CPU (dossier :
/// précision standard ; `light:<dossier>` : allégé ; `f32:<dossier>` : pleine précision).
pub fn add_model(pipeline: Pipeline, spec: &str) -> Result<Pipeline, Error> {
    if is_parser_spec(spec) {
        let (parser, langs) = load_parser(spec)?;
        Ok(share_parser(pipeline, langs, parser))
    } else {
        let lang = lang_of(Path::new(spec)).unwrap_or(Lang::FRA);
        Ok(pipeline.with_tagger(lang, Tagger::load(spec)?))
    }
}

fn is_parser_spec(spec: &str) -> bool {
    ["vulkan:", "f32:", "light:"]
        .iter()
        .any(|p| spec.starts_with(p))
        || Path::new(spec).is_dir()
}

/// Analyseur partagé entre ses langues.
pub type Parser = std::sync::Arc<dyn nknown_nlp::SentenceParser>;

/// Charge un analyseur sans toucher au pipeline : en cas d'échec (pas de GPU…), le pipeline
/// reste intact et l'on peut essayer une autre spécification.
pub fn load_parser(spec: &str) -> Result<(Parser, Vec<Lang>), Error> {
    if let Some(dir) = spec.strip_prefix("vulkan:") {
        vulkan_parser(dir)
    } else if let Some(dir) = spec.strip_prefix("f32:") {
        cpu_parser(dir, Bits::F32)
    } else if let Some(dir) = spec.strip_prefix("light:") {
        cpu_parser(dir, Bits::Light)
    } else {
        cpu_parser(spec, Bits::Standard)
    }
}

/// Un analyseur multilingue, partagé entre toutes ses langues.
pub fn share_parser(mut pipeline: Pipeline, langs: Vec<Lang>, parser: Parser) -> Pipeline {
    let langs = if langs.is_empty() {
        vec![Lang::FRA]
    } else {
        langs
    };
    for lang in langs {
        pipeline = pipeline.with_shared_parser(lang, parser.clone());
    }
    pipeline
}

/// Un GPU Vulkan répond (pilote présent, périphérique utilisable).
pub fn gpu_available() -> bool {
    #[cfg(feature = "vulkan")]
    {
        nknown_neural::vulkan::available()
    }
    #[cfg(not(feature = "vulkan"))]
    {
        false
    }
}

#[cfg(feature = "neural")]
fn cpu_parser(dir: &str, bits: Bits) -> Result<(Parser, Vec<Lang>), Error> {
    use nknown_neural::{NeuralParser, Precision};
    let precision = match bits {
        Bits::F32 => Precision::F32,
        Bits::Standard => Precision::Standard,
        Bits::Light => Precision::Light,
    };
    let parser = NeuralParser::load_with(dir, precision).map_err(Error::model)?;
    let langs = parser.langs();
    Ok((std::sync::Arc::new(parser), langs))
}

#[cfg(not(feature = "neural"))]
fn cpu_parser(_: &str, _: Bits) -> Result<(Parser, Vec<Lang>), Error> {
    Err(Error::Unsupported(
        "analyseur neuronal (fonctionnalité « neural »)",
    ))
}

#[cfg(feature = "vulkan")]
fn vulkan_parser(dir: &str) -> Result<(Parser, Vec<Lang>), Error> {
    let parser = nknown_neural::vulkan::VulkanParser::load(dir).map_err(Error::model)?;
    let langs = parser.langs();
    Ok((std::sync::Arc::new(parser), langs))
}

#[cfg(not(feature = "vulkan"))]
fn vulkan_parser(_: &str) -> Result<(Parser, Vec<Lang>), Error> {
    Err(Error::Unsupported(
        "analyseur sur GPU (fonctionnalité « vulkan »)",
    ))
}

#[cfg(test)]
mod resolve_tests {
    use super::*;

    #[test]
    fn relative_specs_point_into_models_dir() {
        let m = Path::new("/d/models");
        assert_eq!(resolve("parser-multi", m), "/d/models/parser-multi");
        assert_eq!(
            resolve("vulkan:parser-multi", m),
            "vulkan:/d/models/parser-multi"
        );
        assert_eq!(
            resolve("light:parser-multi", m),
            "light:/d/models/parser-multi"
        );
        assert_eq!(resolve("/x/p", m), "/x/p");
        assert_eq!(resolve("vulkan:/x/p", m), "vulkan:/x/p");
    }
}
