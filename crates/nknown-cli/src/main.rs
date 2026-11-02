use std::error::Error;
use std::process::ExitCode;

use nknown_core::analyze::Analyzer;
use nknown_core::lang::Lang;
use nknown_core::source::{Source, SourceId};
use nknown_nlp::Pipeline;
use nknown_sources::LocalFiles;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["ingest", dir] => ingest(dir),
        _ => {
            eprintln!("usage : nknown ingest <dossier>");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("erreur : {e}");
            ExitCode::FAILURE
        }
    }
}

fn ingest(dir: &str) -> Result<(), Box<dyn Error>> {
    let mut source = LocalFiles::new(SourceId("files".into()), dir);
    let pipeline = Pipeline::default();
    let (mut docs, mut words) = (0usize, 0usize);
    for item in source.fetch(None)? {
        let item = item?;
        let lang = item.lang_hint.unwrap_or(Lang::FRA);
        words += pipeline.analyze(lang, &item.text).tokens.len();
        docs += 1;
    }
    println!("{docs} documents, {words} mots");
    Ok(())
}
