use std::error::Error;
use std::fs::File;
use std::io::BufReader;
use std::process::ExitCode;
use std::time::Instant;

use nknown_core::analyze::{Analyzer, Pos, Relation};
use nknown_core::lang::Lang;
use nknown_core::source::{Source, SourceId};
use nknown_nlp::import::{ImportStats, lexique, wiktextract};
use nknown_nlp::langid::LangDetector;
use nknown_nlp::{
    Lexicon, LexiconBuilder, Pipeline, Tagger, TrainingSentence, conllu, eval, tokenize,
};
use nknown_sources::LocalFiles;

const USAGE: &str = "\
usage :
  nknown daemon [--socket]                      (démon local ; aussi lancé sous le nom nknownd)
  nknown models status|fetch [dossier]          (modèles du premier lancement ; défaut :
                                                 dossier des modèles de la configuration)
  nknown ingest <dossier> [lexique.lex]
  nknown index <base.redb> <lexique.lex> <source> [modèle]...
  nknown search <base.redb> <lexique.lex> <requête> [nombre] [--explain]
  nknown rank <base.redb>                         (autorité des documents, PageRank)
  nknown stats <base.redb>                        (occupation de la base, par type de donnée)
  nknown learn <base.redb> [lexiques/] [mot]...  (liens appris des documents ; démon arrêté)
  nknown compact <base.redb>                      (réécrit la base, sans place perdue ; démon arrêté)
  nknown watch <base.redb> <lexique.lex> <source> <intervalle> [modèle]...   (ex. 30m, 2h)
  nknown lexicon build <sortie.lex> <ressource>...
      ressources : wiktextract <dump.jsonl> <code-langue-wiktionary>
                   lexique <Lexique383.tsv>
  nknown lexicon lookup <lexique.lex> <mot>...
  nknown relations build <sortie.rel> <lexique.lex> <dump.jsonl|-> <code-langue-wiktionary>
                                                (liens de sens entre mots ; - : entrée standard)
  nknown relations lookup <liens.rel> <mot>...
  nknown analyze <lexique.lex> <texte> [modèle]...
  nknown eval <lexique.lex> <corpus.conllu> [modèle]...
  nknown tagger train <lexique.lex> <train.conllu> <sortie.tag> [passes]
  nknown parser train [vulkan] <xlm-roberta/> <train.conllu> <dev.conllu> <sortie/> [passes] [pas-max] [--langs=fra,eng]

  source : <dossier>                      fichiers .txt et .md
           rss:<url>                       flux RSS / Atom / JSON Feed
           wiki:<édition>:<Titre>[|<Titre>...]   articles Wikipédia (ex. wiki:fr:Paris|Lyon)
           wiki-search:<édition>:<requête>      les 10 premiers résultats d'une recherche
           wiki-cat:<édition>:<Catégorie>       les 50 premiers articles d'une catégorie
           \"web:<url> [pages=<n>] [depth=<n>]\"  exploration d'un site (robots.txt,
                                           arrêt devant un captcha)
           @<fichier>                      une source par ligne (# = commentaire)
  modèle : fichier .tag (tagueur, couche 2), répertoire d'analyseur (couche 3, CPU)
           ou vulkan:<répertoire> (couche 3 sur GPU)";

type Res = Result<(), Box<dyn Error>>;

/// Modèles du manifeste : ce qui manque selon la configuration, et leur téléchargement.
fn models_cmd(action: &str, dir: Option<&str>) -> Res {
    use nknown_service::{config, fetch};
    let path = config::config_path();
    let cfg = config::Config::load(&path)?;
    let dir = match dir {
        Some(d) => std::path::PathBuf::from(d),
        None => cfg.models_dir(&config::data_dir()),
    };
    let manifest = fetch::manifest();
    let missing = fetch::missing(&manifest, &dir, &fetch::groups(&cfg));
    let total: u64 = missing.iter().map(|f| f.size).sum();
    if missing.is_empty() {
        println!("modèles complets dans {}", dir.display());
        return Ok(());
    }
    println!(
        "{} fichiers manquants dans {} ({:.0} Mo) :",
        missing.len(),
        dir.display(),
        total as f64 / 1e6
    );
    for f in &missing {
        println!("  {} ({:.1} Mo)", f.path, f.size as f64 / 1e6);
    }
    if action == "fetch" {
        let mut shown = 0;
        fetch::fetch(&manifest, &dir, &missing, &mut |done, total| {
            let pct = done * 100 / total.max(1);
            if pct >= shown + 5 || done == total {
                shown = pct;
                eprint!(
                    "\r{pct:3} %  {:.0} / {:.0} Mo",
                    done as f64 / 1e6,
                    total as f64 / 1e6
                );
            }
        })?;
        eprintln!("\nmodèles prêts");
    }
    Ok(())
}

fn main() -> ExitCode {
    // Un seul binaire pour la ligne de commande et le démon (les modèles de détection de langue
    // embarqués pèsent ~40 Mo) : `nknownd` est un lien vers `nknown`.
    let invoked = std::env::args_os().next().unwrap_or_default();
    let name = std::path::Path::new(&invoked)
        .file_name()
        .unwrap_or_default();
    if name == "nknownd" || std::env::args().nth(1).as_deref() == Some("daemon") {
        return nknown_server::daemon::main();
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["--version" | "-V"] => {
            println!("nknown {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        ["index", base, lexicon, spec, models @ ..] => index(base, lexicon, spec, models),
        ["watch", base, lexicon, spec, every, models @ ..] => {
            watch(base, lexicon, spec, every, models)
        }
        ["search", base, lexicon, query, rest @ ..] if rest.len() <= 2 => {
            let explain = rest.contains(&"--explain");
            let limit = rest
                .iter()
                .find(|a| **a != "--explain")
                .copied()
                .unwrap_or("5");
            search(base, lexicon, query, limit, explain)
        }
        ["rank", base] => rank(base),
        ["stats", base] => stats(base),
        ["compact", base] => compact(base),
        ["learn", base, rest @ ..] => learn(base, rest),
        ["models", action @ ("status" | "fetch"), dir @ ..] if dir.len() <= 1 => {
            models_cmd(action, dir.first().copied())
        }
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
        ["parser", "train", xlmr, train, dev, out, rest @ ..] if rest.len() <= 3 => {
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
        ["relations", "build", out, lexicon, dump, lang] => {
            relations_build(out, lexicon, dump, lang)
        }
        ["relations", "lookup", rel, words @ ..] if !words.is_empty() => {
            relations_lookup(rel, words)
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

/// Apprend les liens entre mots des documents de la base, puis montre ceux de `words`.
fn learn(base: &str, rest: &[&str]) -> Res {
    use nknown_engine::{Engine, LearnOptions, RedbStorage};
    let mut engine = Engine::new(RedbStorage::open(base)?).with_stemmer(nknown_nlp::langid::stem);
    if let Some(note) = engine.migrate()? {
        eprintln!("base mise à jour : {note}");
    }
    let start = Instant::now();
    let s = engine.learn(LearnOptions::default())?;
    println!(
        "{} paires nom-adjectif → {} liens syntaxiques ; {} passages ({} répétés écartés), \
         {} documents, {} mots → {} liens thématiques [{:.1?}]",
        s.qualified,
        s.syntactic,
        s.passages,
        s.boilerplate,
        s.docs,
        s.vocab,
        s.links,
        start.elapsed()
    );
    if rest.iter().all(|w| std::path::Path::new(w).is_dir()) {
        for (lemma, lang, links) in engine.learned_sample(40)? {
            let list: Vec<String> = links
                .iter()
                .filter(|l| l.kind == nknown_engine::LinkKind::Syntactic)
                .map(|l| format!("{} {:.2} ({})", l.lemma, l.cosine, l.shared))
                .collect();
            if !list.is_empty() {
                println!("{lemma} ({lang}) : {}", list.join(", "));
            }
        }
    }
    let (words, lexicon) = match rest {
        [dir, words @ ..] if std::path::Path::new(dir).is_dir() => (words, Some(*dir)),
        words => (words, None),
    };
    let pipeline = match lexicon {
        Some(dir) => Some(pipeline(dir, &[])?),
        None => None,
    };
    for word in words {
        // Lemme du mot selon le lexique, s'il y en a un ; sinon le mot tel quel.
        let mut lemmas: Vec<(Lang, Pos, String)> = Vec::new();
        if let Some(p) = &pipeline {
            for lang in p.lexicon_langs() {
                for e in p.lexicon(lang).into_iter().flat_map(|l| l.lookup(word)) {
                    lemmas.push((lang, e.pos, e.lemma.to_owned()));
                }
            }
        }
        if lemmas.is_empty() {
            for pos in [Pos::Noun, Pos::Adj, Pos::Verb] {
                lemmas.push((Lang::FRA, pos, word.to_string()));
            }
        }
        lemmas.dedup();
        for (lang, pos, lemma) in lemmas {
            let links = engine.learned(lang, nknown_engine::schema::TermKind::Word(pos), &lemma)?;
            if links.is_empty() {
                continue;
            }
            let list: Vec<String> = links
                .iter()
                .map(|l| {
                    let kind = match l.kind {
                        nknown_engine::LinkKind::Syntactic => "synt.",
                        nknown_engine::LinkKind::Topical => "thème",
                    };
                    format!("{} {:.2} ({kind}, {})", l.lemma, l.cosine, l.shared)
                })
                .collect();
            println!("{lemma} ({pos:?}, {}) : {}", lang.as_str(), list.join(", "));
        }
    }
    Ok(())
}

fn relations_build(out: &str, lexicon: &str, dump: &str, lang: &str) -> Res {
    use nknown_nlp::import::relations;
    use nknown_nlp::relations::BuildOptions;
    let start = Instant::now();
    let lexicon = Lexicon::load(lexicon)?;
    let mut builder = nknown_nlp::RelationsBuilder::new();
    let stats = if dump == "-" {
        relations::import(std::io::stdin().lock(), lang, Some(&lexicon), &mut builder)?
    } else {
        relations::import(
            BufReader::new(File::open(dump)?),
            lang,
            Some(&lexicon),
            &mut builder,
        )?
    };
    eprintln!(
        "{dump} : {} lignes ({} entrées utilisées, {} invalides) -> {} indices [{:.1?}]",
        stats.entries,
        stats.used,
        stats.invalid,
        builder.evidence_count(),
        start.elapsed()
    );
    // Mot courant : fréquence d'usage (par million) du lexique, 100 et plus = 1.
    let familiarity = |w: &str| {
        let f: f32 = lexicon.lookup(w).map(|e| e.freq).sum();
        if lexicon.lookup(w).next().is_none() {
            0.0
        } else {
            (0.2 + (1.0 + f).log10() / 2.5).min(1.0)
        }
    };
    let rel = builder.build(BuildOptions::default(), familiarity)?;
    rel.save(out)?;
    eprintln!(
        "{out} : {} lemmes liés [{:.1?}]",
        rel.lemma_count(),
        start.elapsed()
    );
    Ok(())
}

fn relations_lookup(path: &str, words: &[&str]) -> Res {
    let rel = nknown_nlp::Relations::load(path)?;
    for word in words {
        let rec = rel.record(word);
        if rec.links.is_empty() && rec.antonyms.is_empty() {
            println!("{word} : (aucun lien)");
            continue;
        }
        println!("{word}");
        for (i, s) in rec.senses.iter().enumerate() {
            println!(
                "  sens {} (entrée {}) : {}",
                i + 1,
                s.group + 1,
                s.signature.join(", ")
            );
        }
        let tag = |t: u8| match t {
            nknown_nlp::relations::SHARED => String::new(),
            t if t >= 128 => format!(" [entrée {}]", t - 127),
            t => format!(" [sens {}]", t + 1),
        };
        let list: Vec<String> = rec
            .links
            .iter()
            .map(|l| format!("{} {:.2}{}", l.word, l.weight, tag(l.tag)))
            .collect();
        println!("  liens : {}", list.join(", "));
        if !rec.antonyms.is_empty() {
            println!("  contraires : {}", rec.antonyms.join(", "));
        }
    }
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

/// Langue d'un fichier de modèle d'après son nom (`fr.lex`, `eng.tag`, `fr_gsd-ud-test`).
fn lang_of(path: &str) -> Option<Lang> {
    nknown_service::models::lang_of(std::path::Path::new(path))
}

/// Pipeline : `lexicon` est un fichier (lexique français) ou un dossier dont chaque
/// `<langue>.lex` et `<langue>.tag` est chargé ; `models` ajoute tagueurs et analyseurs.
fn pipeline(lexicon: &str, models: &[&str]) -> Result<Pipeline, Box<dyn Error>> {
    let start = Instant::now();
    let p = nknown_service::models::load_pipeline(std::path::Path::new(lexicon), models)?;
    if !models.is_empty() {
        eprintln!("modèles chargés : {p:?} [{:.1?}]", start.elapsed());
    }
    Ok(p)
}

#[cfg(feature = "neural")]
fn parser_train(vulkan: bool, xlmr: &str, train: &str, dev: &str, out: &str, rest: &[&str]) -> Res {
    let mut opts = nknown_neural::TrainOptions::default();
    // --langs=fra,eng,rus,zho : langues déclarées du modèle (multilingue).
    let (flags, rest): (Vec<&str>, Vec<&str>) = rest.iter().partition(|a| a.starts_with("--"));
    for f in flags {
        match f.strip_prefix("--langs=") {
            Some(l) => opts.langs = l.split(',').map(str::to_owned).collect(),
            None => return Err(format!("option inconnue : {f}").into()),
        }
    }
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
    // Segmentation prose / code et langue par bloc, comme à l'indexation.
    let pipeline = pipeline(lexicon, models)?;
    let detector = LangDetector::new();
    let analysis = nknown_nlp::analyze_document(
        &pipeline,
        detector.detect(text).unwrap_or(Lang::FRA),
        text,
        &nknown_nlp::Format::Markdown,
        Some(&detector),
    );
    for seg in &analysis.segments {
        let kind = match &seg.kind {
            nknown_core::analyze::SegmentKind::Prose => "prose".to_owned(),
            nknown_core::analyze::SegmentKind::Code { lang } => {
                format!("code {}", lang.as_deref().unwrap_or("?"))
            }
        };
        let lang = seg.lang.map_or("-".to_owned(), |l| l.to_string());
        println!("[bloc {kind}, langue {lang}, {} tokens]", seg.tokens.len());
    }
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
    let lang = lang_of(corpus).unwrap_or(Lang::FRA);
    eprintln!("langue évaluée : {lang}");
    let report = eval::evaluate(&pipeline, lang, &read_corpus(corpus)?);
    print!("{report}");
    Ok(())
}

/// Sources désignées par une spécification (voir l'aide).
fn open_sources(spec: &str) -> Result<Vec<Box<dyn Source>>, Box<dyn Error>> {
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
    Ok(vec![nknown_service::sources::open(spec)?])
}

fn index(base: &str, lexicon: &str, spec: &str, models: &[&str]) -> Res {
    let pipeline = pipeline(lexicon, models)?;
    let mut engine = nknown_engine::Engine::new(nknown_engine::RedbStorage::open(base)?)
        .with_stemmer(nknown_nlp::langid::stem);
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
    let mut engine = nknown_engine::Engine::new(nknown_engine::RedbStorage::open(base)?)
        .with_stemmer(nknown_nlp::langid::stem);
    loop {
        let (y, m, d) = nknown_core::time::Timestamp::now().ymd();
        println!("--- collecte du {y}-{m:02}-{d:02}");
        if let Err(e) = collect(&mut engine, &pipeline, spec) {
            eprintln!("collecte interrompue : {e}");
        }
        match engine.compute_authority() {
            Ok(s) => println!(
                "autorité recalculée : {} documents, {} paires",
                s.docs, s.links
            ),
            Err(e) => eprintln!("autorité : {e}"),
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

fn stats(base: &str) -> Res {
    let file = std::fs::metadata(base)?.len();
    let r = nknown_engine::RedbStorage::open(base)?.report(file)?;
    let mb = |b: u64| b as f64 / 1e6;
    println!(
        "fichier {:.1} Mo · données {:.1} Mo · métadonnées {:.1} Mo · fragmenté {:.1} Mo",
        mb(r.file_bytes),
        mb(r.stored_bytes),
        mb(r.metadata_bytes),
        mb(r.fragmented_bytes)
    );
    let name = |s: &str| match s {
        "M" => "compteurs",
        "H" => "empreintes",
        "D" => "documents",
        "P" => "passages (texte)",
        "L" => "lexique (clé → lemme)",
        "N" => "lemmes",
        "S" => "fréquences (df)",
        "Q" => "qualifications",
        "q" => "qualifications (inverse)",
        "X" => "contenus vus",
        "C" => "points de reprise",
        "A" => "autorité",
        "E1" => "arêtes lemme → passage",
        "E2" => "arêtes cooccurrence",
        "E3" => "arêtes qualifie",
        "E4" => "arêtes similarité",
        "E5" => "arêtes passage → document",
        "E6" => "arêtes document → source",
        "E7" => "arêtes passage → lemme",
        "E8" => "arêtes titre",
        _ => "?",
    };
    let total: u64 = r.spaces.iter().map(|s| s.2 + s.3).sum();
    let mut spaces = r.spaces;
    spaces.sort_by_key(|s| std::cmp::Reverse(s.2 + s.3));
    for (space, n, k, v) in spaces {
        println!(
            "{:>5.1} %  {:>8.1} Mo  {:>10} paires  clés {:>7.1} Mo  valeurs {:>7.1} Mo  {space} {}",
            100.0 * (k + v) as f64 / total.max(1) as f64,
            mb(k + v),
            n,
            mb(k),
            mb(v),
            name(&space)
        );
    }
    Ok(())
}

/// Réécrit la base dans l'ordre des clés (le démon le fait seul quand elle a beaucoup perdu).
fn compact(base: &str) -> Res {
    let (_, note) = nknown_engine::RedbStorage::open_tidy(base, true)?;
    println!("{}", note.unwrap_or_default());
    Ok(())
}

fn rank(base: &str) -> Res {
    let start = Instant::now();
    let mut engine = nknown_engine::Engine::new(nknown_engine::RedbStorage::open(base)?)
        .with_stemmer(nknown_nlp::langid::stem);
    let s = engine.compute_authority()?;
    println!(
        "autorité : {} documents, {} paires reliées, maximum {:.2} (moyenne 1) [{:.1?}]",
        s.docs,
        s.links,
        s.max,
        start.elapsed()
    );
    Ok(())
}

fn search(base: &str, lexicon: &str, query: &str, limit: &str, explain: bool) -> Res {
    use nknown_core::time::Timestamp;
    use nknown_engine::{Engine, RedbStorage};
    let limit: usize = limit
        .parse()
        .map_err(|_| format!("nombre invalide : {limit}"))?;
    let pipeline = pipeline(lexicon, &[])?;
    let mut engine = Engine::new(RedbStorage::open(base)?).with_stemmer(nknown_nlp::langid::stem);
    if let Some(note) = engine.migrate()? {
        eprintln!("base mise à jour : {note}");
    }
    // Comme le démon : langue détectée, mots étiquetés en contexte.
    let mut q =
        nknown_service::query::build_query_with(&pipeline, Some(&LangDetector::new()), query);
    if let Some(i) = &q.intent {
        println!("intention : {}", i.name);
    }
    let notes = nknown_service::query::add_learned(&pipeline, &engine, &mut q);
    if explain {
        for n in &notes {
            println!(
                "{} « {} » : {} {:.2}",
                if n.confirmed {
                    "confirmé par les documents, pour"
                } else {
                    "appris des documents, pour"
                },
                q.terms[n.term].text,
                n.word,
                n.weight
            );
        }
        for t in q.terms.iter().filter(|t| !t.related.is_empty()) {
            let list: Vec<String> = t
                .related
                .iter()
                .map(|(_, w, p)| format!("{w} {p:.2}"))
                .collect();
            println!("liés à « {} » : {}", t.text, list.join(", "));
        }
    }
    let start = Instant::now();
    let result = engine.search_explained(&q, Timestamp::now(), limit)?;
    let hits = result.hits;
    println!(
        "{} résultat(s) sur {} passages candidats [{:.1?}]",
        hits.len(),
        result.candidates,
        start.elapsed()
    );
    if explain {
        let t = result.thresholds;
        let mut line = format!(
            "seuils : couverture^{}, coupure {:.0} % du meilleur, doublons ≥ {:.0} %",
            t.coverage_exponent,
            t.cutoff * 100.0,
            t.duplicate * 100.0
        );
        if let (Some(hl), Some(w)) = (t.half_life_days, t.burst_window_days) {
            line.push_str(&format!(", demi-vie {hl:.1} j, emballement sur {w:.1} j"));
        }
        println!("{line}");
    }
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
        if explain {
            let p = h.parts;
            println!(
                "  score = texte {:.2} × couverture {:.2} × qualif {:.2} × titre {:.2} × nature {:.2} × fraîcheur {:.2} × emballement {:.2} × graphe {:.2} × autorité {:.2}",
                p.text,
                p.coverage,
                p.qualified,
                p.title,
                p.nature,
                p.freshness,
                p.burst,
                p.graph,
                p.authority
            );
        }
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

use nknown_service::query::{main_lang, snippet};
