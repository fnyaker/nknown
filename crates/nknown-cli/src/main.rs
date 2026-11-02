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
  nknown index <base.redb> <lexique.lex> <dossier> [modèle]...
  nknown search <base.redb> <lexique.lex> <requête> [nombre]
  nknown lexicon build <sortie.lex> <ressource>...
      ressources : wiktextract <dump.jsonl> <code-langue-wiktionary>
                   lexique <Lexique383.tsv>
  nknown lexicon lookup <lexique.lex> <mot>...
  nknown analyze <lexique.lex> <texte> [modèle]...
  nknown eval <lexique.lex> <corpus.conllu> [modèle]...
  nknown tagger train <lexique.lex> <train.conllu> <sortie.tag> [passes]
  nknown parser train [vulkan] <xlm-roberta/> <train.conllu> <dev.conllu> <sortie/> [passes] [pas-max]

  modèle : fichier .tag (tagueur, couche 2), répertoire d'analyseur (couche 3, CPU)
           ou vulkan:<répertoire> (couche 3 sur GPU)";

type Res = Result<(), Box<dyn Error>>;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["index", base, lexicon, dir, models @ ..] => index(base, lexicon, dir, models),
        ["search", base, lexicon, query] => search(base, lexicon, query, "5"),
        ["search", base, lexicon, query, limit] => search(base, lexicon, query, limit),
        ["ingest", dir] => ingest(dir, None),
        ["ingest", dir, lexicon] => ingest(dir, Some(lexicon)),
        ["lexicon", "build", out, sources @ ..] if !sources.is_empty() => {
            lexicon_build(out, sources)
        }
        ["analyze", lexicon, text, models @ ..] => analyze(lexicon, text, models),
        ["eval", lexicon, corpus, models @ ..] => eval(lexicon, corpus, models),
        [
            "parser",
            "train",
            "vulkan",
            xlmr,
            train,
            dev,
            out,
            rest @ ..,
        ] if rest.len() <= 2 => parser_train(true, xlmr, train, dev, out, rest),
        ["parser", "train", xlmr, train, dev, out, rest @ ..] if rest.len() <= 2 => {
            parser_train(false, xlmr, train, dev, out, rest)
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
        if let Some(dir) = path.strip_prefix("vulkan:") {
            pipeline = with_vulkan_parser(pipeline, dir)?;
        } else if std::path::Path::new(path).is_dir() {
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

#[cfg(feature = "vulkan")]
fn with_vulkan_parser(pipeline: Pipeline, dir: &str) -> Result<Pipeline, Box<dyn Error>> {
    let start = Instant::now();
    let parser = nknown_neural::vulkan::VulkanParser::load(dir)?;
    eprintln!(
        "analyseur chargé sur GPU (Vulkan) [{:.1?}]",
        start.elapsed()
    );
    Ok(pipeline.with_parser(Lang::FRA, Box::new(parser)))
}

#[cfg(not(feature = "vulkan"))]
fn with_vulkan_parser(_: Pipeline, _: &str) -> Result<Pipeline, Box<dyn Error>> {
    Err("compilé sans la fonctionnalité « vulkan »".into())
}

#[cfg(feature = "neural")]
fn parser_train(vulkan: bool, xlmr: &str, train: &str, dev: &str, out: &str, rest: &[&str]) -> Res {
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
    let (xlmr, out) = (std::path::Path::new(xlmr), std::path::Path::new(out));
    let (train, dev) = (read_corpus(train)?, read_corpus(dev)?);
    let log = |line: &str| eprintln!("{line}");
    let best = if vulkan {
        train_vulkan(xlmr, &train, &dev, out, &opts)?
    } else {
        nknown_neural::train(xlmr, &train, &dev, out, &opts, log)?
    };
    eprintln!("meilleur sur dev : {best:?}");
    Ok(())
}

#[cfg(feature = "vulkan")]
fn train_vulkan(
    xlmr: &std::path::Path,
    train: &[conllu::GoldSentence],
    dev: &[conllu::GoldSentence],
    out: &std::path::Path,
    opts: &nknown_neural::TrainOptions,
) -> Result<nknown_neural::train::DevScores, Box<dyn Error>> {
    Ok(nknown_neural::vulkan::train(
        xlmr,
        train,
        dev,
        out,
        opts,
        |l| eprintln!("{l}"),
    )?)
}

#[cfg(all(feature = "neural", not(feature = "vulkan")))]
fn train_vulkan(
    _: &std::path::Path,
    _: &[conllu::GoldSentence],
    _: &[conllu::GoldSentence],
    _: &std::path::Path,
    _: &nknown_neural::TrainOptions,
) -> Result<nknown_neural::train::DevScores, Box<dyn Error>> {
    Err("compilé sans la fonctionnalité « vulkan »".into())
}

#[cfg(not(feature = "neural"))]
fn parser_train(_: bool, _: &str, _: &str, _: &str, _: &str, _: &[&str]) -> Res {
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

fn index(base: &str, lexicon: &str, dir: &str, models: &[&str]) -> Res {
    use nknown_engine::{Document, Engine, Ingested, RedbStorage};
    let start = Instant::now();
    let pipeline = pipeline(lexicon, models)?;
    let mut engine = Engine::new(RedbStorage::open(base)?);
    let mut source = LocalFiles::new(SourceId("files".into()), dir);
    let kind = source.kind();
    let (mut added, mut passages, mut duplicates) = (0usize, 0usize, 0usize);
    for item in source.fetch(None)? {
        let item = item?;
        let lang = item.lang_hint.unwrap_or(Lang::FRA);
        let analysis = pipeline.analyze(lang, &item.text);
        match engine.ingest(Document {
            item: &item,
            kind,
            lang,
            analysis: &analysis,
        })? {
            Ingested::Added { passages: p, .. } => {
                added += 1;
                passages += p;
            }
            Ingested::Duplicate(_) => duplicates += 1,
        }
    }
    let stats = engine.stats()?;
    println!(
        "{added} documents ajoutés ({passages} passages), {duplicates} déjà présents [{:.1?}]",
        start.elapsed()
    );
    println!(
        "base : {} documents, {} passages, {} mots indexés",
        stats.docs, stats.passages, stats.indexed_words
    );
    Ok(())
}

/// Requête → mots et lectures possibles (lexique, plus la forme brute pour les mots inconnus
/// et les noms propres), et intention éventuelle (« actu », « info »...).
fn build_query(lexicon: &Lexicon, text: &str) -> nknown_engine::Query {
    use nknown_engine::{QueryTerm, intent_for};
    let mut intent = None;
    let mut terms = Vec::new();
    for span in tokenize::words(text) {
        let form = &text[span];
        if intent.is_none()
            && let Some(i) = intent_for(form)
        {
            intent = Some(i);
            continue;
        }
        let mut readings: Vec<(Pos, String)> = lexicon
            .lookup(form)
            .filter(|e| nknown_engine::ingest::is_content(e.pos))
            .map(|e| (e.pos, e.lemma.to_owned()))
            .collect();
        for pos in [Pos::Propn, Pos::Noun, Pos::X] {
            readings.push((pos, form.to_owned()));
        }
        readings.dedup();
        terms.push(QueryTerm {
            text: form.to_owned(),
            readings,
        });
    }
    nknown_engine::Query {
        lang: Lang::FRA,
        terms,
        intent,
    }
}

fn search(base: &str, lexicon: &str, query: &str, limit: &str) -> Res {
    use nknown_core::time::Timestamp;
    use nknown_engine::{Engine, RedbStorage};
    let limit: usize = limit
        .parse()
        .map_err(|_| format!("nombre invalide : {limit}"))?;
    let lexicon = Lexicon::load(lexicon)?;
    let engine = Engine::new(RedbStorage::open(base)?);
    let q = build_query(&lexicon, query);
    if let Some(i) = &q.intent {
        println!("intention : {}", i.name);
    }
    let start = Instant::now();
    let hits = engine.search(&q, Timestamp::now(), limit)?;
    println!("{} résultat(s) [{:.1?}]", hits.len(), start.elapsed());
    for h in hits {
        let (y, m, d) = Timestamp(h.doc.date).ymd();
        let title = h.doc.title.as_deref().unwrap_or("(sans titre)");
        println!("\n{:.3}  {y}-{m:02}-{d:02}  {title}", h.score);
        let mut info = format!("  mots : {}", h.matched.join(", "));
        for (adj, noun) in &h.qualified {
            info.push_str(&format!(" ; « {adj} » qualifie « {noun} »"));
        }
        println!("{info}");
        let snippet: String = h.text.chars().take(220).collect();
        let more = if h.text.chars().count() > 220 {
            "…"
        } else {
            ""
        };
        println!("  {}{more}", snippet.replace('\n', " "));
    }
    Ok(())
}
