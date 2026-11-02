use std::error::Error;
use std::fs::File;
use std::io::BufReader;
use std::process::ExitCode;
use std::time::Instant;

use nknown_core::analyze::{Analyzer, Pos, Relation};
use nknown_core::lang::Lang;
use nknown_core::source::{Source, SourceId};
use nknown_nlp::import::{ImportStats, lexique, wiktextract};
use nknown_nlp::{
    Lexicon, LexiconBuilder, Pipeline, Tagger, TrainingSentence, conllu, eval, tokenize,
};
use nknown_sources::LocalFiles;

const USAGE: &str = "\
usage :
  nknown ingest <dossier> [lexique.lex]
  nknown lexicon build <sortie.lex> <ressource>...
      ressources : wiktextract <dump.jsonl> <code-langue-wiktionary>
                   lexique <Lexique383.tsv>
  nknown lexicon lookup <lexique.lex> <mot>...
  nknown analyze <lexique.lex> <texte> [modèle]...
  nknown eval <lexique.lex> <corpus.conllu> [modèle]...
  nknown tagger train <lexique.lex> <train.conllu> <sortie.tag> [passes]
  nknown parser train <xlm-roberta/> <train.conllu> <dev.conllu> <sortie/> [passes] [pas-max]

  modèle : fichier .tag (tagueur, couche 2) ou répertoire d'analyseur (couche 3)";

type Res = Result<(), Box<dyn Error>>;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["ingest", dir] => ingest(dir, None),
        ["ingest", dir, lexicon] => ingest(dir, Some(lexicon)),
        ["lexicon", "build", out, sources @ ..] if !sources.is_empty() => {
            lexicon_build(out, sources)
        }
        ["analyze", lexicon, text, models @ ..] => analyze(lexicon, text, models),
        ["eval", lexicon, corpus, models @ ..] => eval(lexicon, corpus, models),
        ["parser", "train", xlmr, train, dev, out, rest @ ..] if rest.len() <= 2 => {
            parser_train(xlmr, train, dev, out, rest)
        }
        ["tagger", "train", lexicon, corpus, out] => tagger_train(lexicon, corpus, out, "10"),
        ["tagger", "train", lexicon, corpus, out, passes] => {
            tagger_train(lexicon, corpus, out, passes)
        }
        ["tagger", "eval", lexicon, corpus, models @ ..] => eval(lexicon, corpus, models),
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
        let word_tokens = analysis
            .tokens
            .iter()
            .filter(|t| tokenize::is_word(&item.text[t.span.clone()]));
        for t in word_tokens {
            words += 1;
            known += usize::from(t.pos != Pos::X);
        }
        docs += 1;
    }
    println!("{docs} documents, {words} mots, {known} reconnus par le lexique");
    Ok(())
}

fn lexicon_build(out: &str, mut sources: &[&str]) -> Res {
    let start = Instant::now();
    let mut builder = LexiconBuilder::new();
    while !sources.is_empty() {
        let (name, stats): (&str, ImportStats) = match sources {
            ["wiktextract", dump, lang, rest @ ..] => {
                sources = rest;
                let file = BufReader::new(File::open(dump)?);
                (dump, wiktextract::import(file, lang, &mut builder)?)
            }
            ["lexique", path, rest @ ..] => {
                sources = rest;
                (
                    path,
                    lexique::import(BufReader::new(File::open(path)?), &mut builder)?,
                )
            }
            _ => return Err(format!("ressource invalide : {}\n{USAGE}", sources.join(" ")).into()),
        };
        eprintln!(
            "{name} : {} lignes ({} utilisées, {} invalides) -> {} formes au total [{:.1?}]",
            stats.entries,
            stats.used,
            stats.invalid,
            builder.form_count(),
            start.elapsed()
        );
    }
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
            println!(
                "  {:<20} {:<6} {:<10} {:>9.2}",
                e.lemma,
                format!("{:?}", e.pos),
                feats,
                e.freq
            );
        }
        if !found {
            println!("  (inconnu)");
        }
    }
    Ok(())
}

fn pipeline(lexicon: &str, models: &[&str]) -> Result<Pipeline, Box<dyn Error>> {
    let mut pipeline = Pipeline::default().with_lexicon(Lang::FRA, Lexicon::load(lexicon)?);
    for path in models {
        if std::path::Path::new(path).is_dir() {
            pipeline = with_parser(pipeline, path)?;
        } else {
            pipeline = pipeline.with_tagger(Lang::FRA, Tagger::load(path)?);
        }
    }
    Ok(pipeline)
}

#[cfg(feature = "neural")]
fn with_parser(pipeline: Pipeline, dir: &str) -> Result<Pipeline, Box<dyn Error>> {
    let start = Instant::now();
    let parser = nknown_neural::NeuralParser::load(dir)?;
    eprintln!("analyseur chargé [{:.1?}]", start.elapsed());
    Ok(pipeline.with_parser(Lang::FRA, Box::new(parser)))
}

#[cfg(not(feature = "neural"))]
fn with_parser(_: Pipeline, _: &str) -> Result<Pipeline, Box<dyn Error>> {
    Err("compilé sans la fonctionnalité « neural » (couche 3)".into())
}

#[cfg(feature = "neural")]
fn parser_train(xlmr: &str, train: &str, dev: &str, out: &str, rest: &[&str]) -> Res {
    let mut opts = nknown_neural::TrainOptions::default();
    if let Some(p) = rest.first() {
        opts.epochs = p
            .parse()
            .map_err(|_| format!("nombre de passes invalide : {p}"))?;
    }
    if let Some(m) = rest.get(1) {
        opts.max_steps = Some(
            m.parse()
                .map_err(|_| format!("nombre de pas invalide : {m}"))?,
        );
    }
    let best = nknown_neural::train(
        std::path::Path::new(xlmr),
        &read_corpus(train)?,
        &read_corpus(dev)?,
        std::path::Path::new(out),
        &opts,
        |line| eprintln!("{line}"),
    )?;
    eprintln!("meilleur sur dev : {best:?}");
    Ok(())
}

#[cfg(not(feature = "neural"))]
fn parser_train(_: &str, _: &str, _: &str, _: &str, _: &[&str]) -> Res {
    Err("compilé sans la fonctionnalité « neural » (couche 3)".into())
}

fn analyze(lexicon: &str, text: &str, models: &[&str]) -> Res {
    let analysis = pipeline(lexicon, models)?.analyze(Lang::FRA, text);
    let form = |i: usize| &text[analysis.tokens[i].span.clone()];
    for (i, t) in analysis.tokens.iter().enumerate() {
        println!("{:<16} {:<16} {:?}", form(i), t.lemma, t.pos);
    }
    for r in &analysis.relations {
        match r {
            Relation::Qualifies { adj, noun } => {
                println!("{} -> qualifie -> {}", form(*adj), form(*noun))
            }
            Relation::Dependency { dep, head, label } => {
                let head = head.map_or("RACINE", form);
                println!("  {:<14} --{label}--> {head}", form(*dep));
            }
        }
    }
    Ok(())
}

fn read_corpus(path: &str) -> Result<Vec<conllu::GoldSentence>, Box<dyn Error>> {
    Ok(conllu::parse(BufReader::new(File::open(path)?))?)
}

fn tagger_train(lexicon: &str, corpus: &str, out: &str, passes: &str) -> Res {
    let passes: usize = passes
        .parse()
        .map_err(|_| format!("nombre de passes invalide : {passes}"))?;
    let lexicon = Lexicon::load(lexicon)?;
    let sentences: Vec<TrainingSentence> = read_corpus(corpus)?
        .iter()
        .map(|s| s.to_training())
        .collect();
    let start = Instant::now();
    eprintln!("{} phrases d'entraînement", sentences.len());
    let tagger = Tagger::train(&sentences, Some(&lexicon), passes, |epoch, acc| {
        eprintln!(
            "passe {epoch:>2} : {:.2} % [{:.1?}]",
            acc * 100.0,
            start.elapsed()
        );
    });
    tagger.save(out)?;
    eprintln!("{out} : {} traits", tagger.feature_count());
    Ok(())
}

fn eval(lexicon: &str, corpus: &str, models: &[&str]) -> Res {
    let pipeline = pipeline(lexicon, models)?;
    let report = eval::evaluate(&pipeline, Lang::FRA, &read_corpus(corpus)?);
    print!("{report}");
    Ok(())
}
