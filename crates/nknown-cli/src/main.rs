use std::error::Error;
use std::fs::File;
use std::io::BufReader;
use std::process::ExitCode;
use std::time::Instant;

use nknown_core::analyze::Analyzer;
use nknown_core::lang::Lang;
use nknown_core::source::{Source, SourceId};
use nknown_nlp::{Lexicon, LexiconBuilder, Pipeline, wiktextract};
use nknown_sources::LocalFiles;

const USAGE: &str = "\
usage :
  nknown ingest <dossier> [lexique.lex]
  nknown lexicon build <dump-wiktextract.jsonl> <code-langue-wiktionary> <sortie.lex>
  nknown lexicon lookup <lexique.lex> <mot>...";

type Res = Result<(), Box<dyn Error>>;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["ingest", dir] => ingest(dir, None),
        ["ingest", dir, lexicon] => ingest(dir, Some(lexicon)),
        ["lexicon", "build", dump, lang, out] => lexicon_build(dump, lang, out),
        ["lexicon", "lookup", lexicon, words @ ..] if !words.is_empty() => {
            lexicon_lookup(lexicon, words)
        }
        _ => {
            eprintln!("{USAGE}");
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

fn ingest(dir: &str, lexicon: Option<&str>) -> Res {
    let mut pipeline = Pipeline::default();
    if let Some(path) = lexicon {
        pipeline = pipeline.with_lexicon(Lang::FRA, Lexicon::load(path)?);
    }
    let mut source = LocalFiles::new(SourceId("files".into()), dir);
    let (mut docs, mut words, mut known) = (0usize, 0usize, 0usize);
    for item in source.fetch(None)? {
        let item = item?;
        let lang = item.lang_hint.unwrap_or(Lang::FRA);
        let analysis = pipeline.analyze(lang, &item.text);
        words += analysis.tokens.len();
        known += analysis
            .tokens
            .iter()
            .filter(|t| t.pos != nknown_core::analyze::Pos::X)
            .count();
        docs += 1;
    }
    println!("{docs} documents, {words} mots, {known} reconnus par le lexique");
    Ok(())
}

fn lexicon_build(dump: &str, lang: &str, out: &str) -> Res {
    let start = Instant::now();
    let mut builder = LexiconBuilder::new();
    let stats = wiktextract::import(BufReader::new(File::open(dump)?), lang, &mut builder)?;
    eprintln!(
        "{} entrées lues ({} utilisées, {} invalides), {} formes distinctes [{:.1?}]",
        stats.entries,
        stats.used,
        stats.invalid,
        builder.form_count(),
        start.elapsed()
    );
    let lexicon = builder.build()?;
    lexicon.save(out)?;
    eprintln!(
        "{out} : {} formes, {} analyses [{:.1?}]",
        lexicon.form_count(),
        lexicon.analysis_count(),
        start.elapsed()
    );
    Ok(())
}

fn lexicon_lookup(path: &str, words: &[&str]) -> Res {
    let lexicon = Lexicon::load(path)?;
    for word in words {
        println!("{word}");
        let mut found = false;
        for e in lexicon.lookup(word) {
            found = true;
            let feats = [
                e.feats.gender.map(|g| format!("{g:?}")),
                e.feats.number.map(|n| format!("{n:?}")),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(",");
            println!("  {:<20} {:<6} {feats}", e.lemma, format!("{:?}", e.pos));
        }
        if !found {
            println!("  (inconnu)");
        }
    }
    Ok(())
}
