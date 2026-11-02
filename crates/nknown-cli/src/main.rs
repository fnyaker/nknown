use std::error::Error;
use std::fs::File;
use std::io::BufReader;
use std::process::ExitCode;
use std::time::Instant;

use nknown_core::analyze::{Analyzer, Pos, Relation};
use nknown_core::lang::Lang;
use nknown_core::source::{Source, SourceId};
use nknown_nlp::import::{ImportStats, lexique, wiktextract};
use nknown_nlp::langid::{self, LangDetector};
use nknown_nlp::{
    Lexicon, LexiconBuilder, Pipeline, Tagger, TrainingSentence, conllu, eval, tokenize,
};
use nknown_sources::LocalFiles;

const USAGE: &str = "\
usage :
  nknown ingest <dossier> [lexique.lex]
  nknown index <base.redb> <lexique.lex> <source> [modèle]...
  nknown search <base.redb> <lexique.lex> <requête> [nombre]
  nknown watch <base.redb> <lexique.lex> <source> <intervalle> [modèle]...   (ex. 30m, 2h)
  nknown lexicon build <sortie.lex> <ressource>...
      ressources : wiktextract <dump.jsonl> <code-langue-wiktionary>
                   lexique <Lexique383.tsv>
  nknown lexicon lookup <lexique.lex> <mot>...
  nknown analyze <lexique.lex> <texte> [modèle]...
  nknown eval <lexique.lex> <corpus.conllu> [modèle]...
  nknown tagger train <lexique.lex> <train.conllu> <sortie.tag> [passes]
  nknown parser train [vulkan] <xlm-roberta/> <train.conllu> <dev.conllu> <sortie/> [passes] [pas-max]

  source : <dossier>                      fichiers .txt et .md
           rss:<url>                       flux RSS / Atom / JSON Feed
           wiki:<édition>:<Titre>[|<Titre>...]   articles Wikipédia (ex. wiki:fr:Paris|Lyon)
           wiki-search:<édition>:<requête>      les 10 premiers résultats d'une recherche
           wiki-cat:<édition>:<Catégorie>       les 50 premiers articles d'une catégorie
           @<fichier>                      une source par ligne (# = commentaire)
  modèle : fichier .tag (tagueur, couche 2), répertoire d'analyseur (couche 3, CPU)
           ou vulkan:<répertoire> (couche 3 sur GPU)";

type Res = Result<(), Box<dyn Error>>;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["index", base, lexicon, spec, models @ ..] => index(base, lexicon, spec, models),
        ["watch", base, lexicon, spec, every, models @ ..] => {
            watch(base, lexicon, spec, every, models)
        }
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

/// Sources désignées par une spécification (voir l'aide).
fn open_sources(spec: &str) -> Result<Vec<Box<dyn Source>>, Box<dyn Error>> {
    use nknown_sources::{RssSource, Selection, WikipediaSource};
    if let Some(path) = spec.strip_prefix('@') {
        let list = std::fs::read_to_string(path)?;
        let mut out = Vec::new();
        for line in list.lines().map(str::trim) {
            if !line.is_empty() && !line.starts_with('#') {
                out.extend(open_sources(line)?);
            }
        }
        return Ok(out);
    }
    let wiki = |rest: &str| -> Result<(String, String), Box<dyn Error>> {
        let (edition, arg) = rest
            .split_once(':')
            .ok_or_else(|| format!("édition manquante : {spec}"))?;
        Ok((edition.to_owned(), arg.to_owned()))
    };
    let source: Box<dyn Source> = if let Some(url) = spec.strip_prefix("rss:") {
        Box::new(RssSource::new(url))
    } else if let Some(rest) = spec.strip_prefix("wiki:") {
        let (edition, titles) = wiki(rest)?;
        let titles = titles.split('|').map(|t| t.trim().to_owned()).collect();
        Box::new(WikipediaSource::new(edition, Selection::Titles(titles)))
    } else if let Some(rest) = spec.strip_prefix("wiki-search:") {
        let (edition, query) = wiki(rest)?;
        Box::new(WikipediaSource::new(
            edition,
            Selection::Search { query, limit: 10 },
        ))
    } else if let Some(rest) = spec.strip_prefix("wiki-cat:") {
        let (edition, name) = wiki(rest)?;
        Box::new(WikipediaSource::new(
            edition,
            Selection::Category { name, limit: 50 },
        ))
    } else if std::path::Path::new(spec).is_dir() {
        Box::new(LocalFiles::new(SourceId(format!("files:{spec}")), spec))
    } else {
        return Err(format!("source inconnue : {spec}").into());
    };
    Ok(vec![source])
}

fn index(base: &str, lexicon: &str, spec: &str, models: &[&str]) -> Res {
    let pipeline = pipeline(lexicon, models)?;
    let mut engine = nknown_engine::Engine::new(nknown_engine::RedbStorage::open(base)?);
    collect(&mut engine, &pipeline, spec)
}

/// Collecte périodique : relit les sources toutes les `every` (ex. `30m`, `2h`, `90s`).
fn watch(base: &str, lexicon: &str, spec: &str, every: &str, models: &[&str]) -> Res {
    let secs: u64 = match every.char_indices().last() {
        Some((i, 's')) => every[..i].parse()?,
        Some((i, 'm')) => every[..i].parse::<u64>()? * 60,
        Some((i, 'h')) => every[..i].parse::<u64>()? * 3600,
        _ => return Err(format!("intervalle invalide : {every} (ex. 30m, 2h)").into()),
    };
    let pipeline = pipeline(lexicon, models)?;
    let mut engine = nknown_engine::Engine::new(nknown_engine::RedbStorage::open(base)?);
    loop {
        let (y, m, d) = nknown_core::time::Timestamp::now().ymd();
        println!("--- collecte du {y}-{m:02}-{d:02}");
        if let Err(e) = collect(&mut engine, &pipeline, spec) {
            eprintln!("collecte interrompue : {e}");
        }
        std::thread::sleep(std::time::Duration::from_secs(secs.max(60)));
    }
}

/// Collecte incrémentale de toutes les sources de `spec` : seuls les contenus nouveaux
/// ou modifiés sont analysés ; les versions modifiées remplacent les anciennes.
fn collect(
    engine: &mut nknown_engine::Engine<nknown_engine::RedbStorage>,
    pipeline: &Pipeline,
    spec: &str,
) -> Res {
    use nknown_core::time::Timestamp;
    use nknown_engine::{Document, Ingested, ItemStatus};
    let start = Instant::now();
    let detector = LangDetector::new();
    for mut source in open_sources(spec)? {
        let (kind, name, checkpoint) = (
            source.kind(),
            source.id().0.clone(),
            source.checkpoint_key(),
        );
        // Marge de 2 jours : un flux peut publier en retard ; les doublons sont reconnus.
        let since = engine
            .checkpoint(&checkpoint)?
            .map(|t| Timestamp(t.0 - 2 * 86_400));
        let fetched_at = Timestamp::now();
        let (mut added, mut updated, mut unchanged, mut errors) = (0usize, 0usize, 0usize, 0usize);
        let known = |ext: &str| engine.known(&name, ext).ok().flatten();
        let items = match source.fetch_new(since, &known) {
            Ok(items) => items,
            Err(e) => {
                eprintln!("{name} : {e}");
                continue;
            }
        };
        for item in items {
            let item = match item {
                Ok(item) => item,
                Err(e) => {
                    eprintln!("{name} : {e}");
                    errors += 1;
                    continue;
                }
            };
            if let ItemStatus::Unchanged(_) = engine.status(&item)? {
                unchanged += 1;
                continue;
            }
            let lang = item.lang_hint.unwrap_or(Lang::FRA);
            let format = item
                .meta
                .iter()
                .find(|(k, _)| k == "format")
                .map_or(nknown_nlp::Format::Plain, |(_, v)| {
                    nknown_nlp::Format::from_hint(v)
                });
            let analysis =
                nknown_nlp::analyze_document(pipeline, lang, &item.text, &format, Some(&detector));
            let lang = main_lang(&analysis).unwrap_or(lang);
            let title = item.title.as_deref().map(|t| pipeline.analyze(lang, t));
            match engine.ingest(Document {
                item: &item,
                kind,
                lang,
                analysis: &analysis,
                title: title.as_ref(),
            })? {
                Ingested::Added { .. } => added += 1,
                Ingested::Updated { .. } => updated += 1,
                Ingested::Duplicate(_) => unchanged += 1,
            }
        }
        if errors == 0 {
            engine.set_checkpoint(&checkpoint, fetched_at)?;
        }
        println!(
            "{name} : {added} nouveaux, {updated} mis à jour, {unchanged} inchangés{}",
            if errors > 0 {
                format!(", {errors} erreurs")
            } else {
                String::new()
            }
        );
    }
    let stats = engine.stats()?;
    println!(
        "base : {} documents, {} passages, {} mots indexés [{:.1?}]",
        stats.docs,
        stats.passages,
        stats.indexed_words,
        start.elapsed()
    );
    Ok(())
}

/// Requête → mots et lectures possibles (lexique, plus la forme brute pour les mots inconnus
/// et les noms propres), et intention éventuelle (« actu », « info »...).
fn build_query(lexicon: &Lexicon, text: &str) -> nknown_engine::Query {
    use nknown_engine::schema::TermKind;
    use nknown_engine::{QueryTerm, intent_for};
    use nknown_nlp::segment;
    let mut intent = None;
    let mut terms = Vec::new();
    for chunk in text.split_whitespace() {
        if intent.is_none()
            && let Some(i) = intent_for(chunk)
        {
            intent = Some(i);
            continue;
        }
        // Identifiant manifeste (`read_to_string`, `HashMap::new`, `f()`) : on cherche
        // l'identifiant exact, pas ses morceaux (« string » est partout dans le code).
        if !segment::inline_code(chunk, 0..chunk.len()).is_empty() {
            let ident = chunk.trim_end_matches("()").trim_matches('`');
            let readings = vec![(Lang::NONE, TermKind::Code, ident.to_lowercase())];
            terms.push(QueryTerm {
                text: chunk.to_owned(),
                readings,
            });
            continue;
        }
        // Mot ordinaire : ses lectures en prose (lexique), et le même mot comme terme de code.
        for span in tokenize::words(chunk) {
            let form = &chunk[span];
            let mut readings: Vec<(Lang, TermKind, String)> = lexicon
                .lookup(form)
                .filter(|e| nknown_engine::ingest::is_content(e.pos))
                .map(|e| (Lang::FRA, TermKind::Word(e.pos), e.lemma.to_owned()))
                .collect();
            for pos in [Pos::Propn, Pos::Noun, Pos::X] {
                readings.push((Lang::FRA, TermKind::Word(pos), form.to_owned()));
            }
            // Autres langues détectables, indexées par racines (pas de lexique).
            for lang in LangDetector::languages()
                .into_iter()
                .filter(|l| *l != Lang::FRA)
            {
                let stem = langid::stem(lang, form).unwrap_or_else(|| form.to_lowercase());
                readings.push((lang, TermKind::Word(Pos::X), stem));
            }
            readings.push((Lang::NONE, TermKind::Code, form.to_lowercase()));
            readings.dedup();
            terms.push(QueryTerm {
                text: form.to_owned(),
                readings,
            });
        }
    }
    nknown_engine::Query { terms, intent }
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
        let kind = match (h.is_code, &h.code_lang) {
            (true, Some(l)) => format!("  [code {l}]"),
            (true, None) => "  [code]".to_owned(),
            _ => String::new(),
        };
        println!("\n{:.3}  {y}-{m:02}-{d:02}  {title}{kind}", h.score);
        let mut info = format!("  mots : {}", h.matched.join(", "));
        for (adj, noun) in &h.qualified {
            info.push_str(&format!(" ; « {adj} » qualifie « {noun} »"));
        }
        println!("{info}");
        println!("  {}", snippet(&h.text, &h.matched, 220));
        for r in h.related.iter().take(3) {
            let (y, m, d) = Timestamp(r.date).ymd();
            println!(
                "  ↳ aussi ({:.0} %) : {} — {} ({y}-{m:02}-{d:02})",
                r.similarity * 100.0,
                r.title.as_deref().unwrap_or("(sans titre)"),
                r.source
            );
        }
    }
    Ok(())
}

/// Extrait de `text` d'environ `width` caractères, centré sur la première occurrence
/// d'un des mots trouvés.
fn snippet(text: &str, words: &[String], width: usize) -> String {
    let lower = text.to_lowercase();
    let hit = words
        .iter()
        .filter_map(|w| lower.find(&w.to_lowercase()))
        .min()
        .unwrap_or(0);
    let hit_chars = lower.get(..hit).map_or(0, |s| s.chars().count());
    let start = hit_chars.saturating_sub(width / 3);
    let total = text.chars().count();
    let body = text
        .chars()
        .skip(start)
        .take(width)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let before = if start > 0 { "…" } else { "" };
    let after = if start + width < total { "…" } else { "" };
    format!("{before}{body}{after}")
}

/// Langue qui couvre le plus de tokens de prose.
fn main_lang(a: &nknown_core::analyze::Analysis) -> Option<Lang> {
    let mut counts: Vec<(Lang, usize)> = Vec::new();
    for seg in &a.segments {
        let Some(l) = seg.lang else { continue };
        let n = seg.tokens.len();
        match counts.iter_mut().find(|(c, _)| *c == l) {
            Some((_, c)) => *c += n,
            None => counts.push((l, n)),
        }
    }
    counts.into_iter().max_by_key(|&(_, n)| n).map(|(l, _)| l)
}
