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
    let (prefix, path) = match spec.strip_prefix("vulkan:") {
        Some(p) => ("vulkan:", p),
        None => ("", spec),
    };
    if Path::new(path).is_absolute() {
        spec.to_owned()
    } else {
        format!("{prefix}{}", models_dir.join(path).display())
    }
}

/// Ajoute un modèle : tagueur (fichier), analyseur CPU (dossier) ou GPU (`vulkan:<dossier>`).
pub fn add_model(pipeline: Pipeline, spec: &str) -> Result<Pipeline, Error> {
    if let Some(dir) = spec.strip_prefix("vulkan:") {
        with_vulkan_parser(pipeline, dir)
    } else if Path::new(spec).is_dir() {
        with_parser(pipeline, spec)
    } else {
        let lang = lang_of(Path::new(spec)).unwrap_or(Lang::FRA);
        Ok(pipeline.with_tagger(lang, Tagger::load(spec)?))
    }
}

/// Un analyseur multilingue, partagé entre toutes ses langues.
#[cfg(feature = "neural")]
fn share_parser(
    mut pipeline: Pipeline,
    langs: Vec<Lang>,
    parser: std::sync::Arc<dyn nknown_nlp::SentenceParser>,
) -> Pipeline {
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

#[cfg(feature = "neural")]
fn with_parser(pipeline: Pipeline, dir: &str) -> Result<Pipeline, Error> {
    let parser = nknown_neural::NeuralParser::load(dir).map_err(Error::model)?;
    let langs = parser.langs();
    Ok(share_parser(pipeline, langs, std::sync::Arc::new(parser)))
}

#[cfg(not(feature = "neural"))]
fn with_parser(_: Pipeline, _: &str) -> Result<Pipeline, Error> {
    Err(Error::Unsupported(
        "analyseur neuronal (fonctionnalité « neural »)",
    ))
}

#[cfg(feature = "vulkan")]
fn with_vulkan_parser(pipeline: Pipeline, dir: &str) -> Result<Pipeline, Error> {
    let parser = nknown_neural::vulkan::VulkanParser::load(dir).map_err(Error::model)?;
    let langs = parser.langs();
    Ok(share_parser(pipeline, langs, std::sync::Arc::new(parser)))
}

#[cfg(not(feature = "vulkan"))]
fn with_vulkan_parser(_: Pipeline, _: &str) -> Result<Pipeline, Error> {
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
        assert_eq!(resolve("/x/p", m), "/x/p");
        assert_eq!(resolve("vulkan:/x/p", m), "vulkan:/x/p");
    }
}
