//! Harnais d'évaluation de l'élargissement de requête : niveaux Strict / Normal / Diversifié.
//!
//! Chaque ligne de `eval/expansion.tsv` (ou du jeu tenu à l'écart) construit trois documents
//! contrôlés dans une base en mémoire, puis une recherche avec le niveau demandé. Le niveau
//! passe par les fonctions de production (`build_query_in_variety` et `add_learned`) : aucun
//! élargissement n'est appliqué à la main dans le test.
//!
//! Les modèles réels sont lus dans `NKNOWN_MODELS`, sinon dans
//! `~/.local/share/nknown/models`. Sans modèles, le test est ignoré (CI sans corpus). Le jeu
//! tenu à l'écart et la base réelle ne s'exécutent que sur demande (`--ignored`).

use std::path::{Path, PathBuf};

use nknown_core::item::RawItem;
use nknown_core::lang::Lang;
use nknown_core::source::{SourceId, SourceKind};
use nknown_core::time::Timestamp;
use nknown_engine::{Engine, MemStorage, RedbStorage, Variety};
use nknown_nlp::langid::LangDetector;
use nknown_service::models::load_pipeline;
use nknown_service::query;

#[derive(Debug, Clone)]
struct Row {
    lang: Lang,
    query: String,
    positive: String,
    negative: String,
    distractor: String,
    category: String,
    note: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    Strict,
    Normal,
    Diverse,
}

impl Level {
    const ALL: [Level; 3] = [Level::Strict, Level::Normal, Level::Diverse];

    fn name(self) -> &'static str {
        match self {
            Level::Strict => "strict",
            Level::Normal => "normal",
            Level::Diverse => "diverse",
        }
    }

    fn variety(self) -> Variety {
        match self {
            Level::Strict => Variety::Strict,
            Level::Normal => Variety::Normal,
            Level::Diverse => Variety::Diverse,
        }
    }
}

/// Phrase neutre propre à la langue : même longueur relative pour les trois documents.
fn suffix(lang: Lang) -> &'static str {
    match lang.as_str() {
        "fra" => "affecte le service d'authentification.",
        "rus" => "влияет на службу аутентификации.",
        "zho" => "影响认证服务。",
        _ => "affects the authentication service.",
    }
}

fn models_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("NKNOWN_MODELS") {
        let dir = PathBuf::from(dir);
        return dir.is_dir().then_some(dir);
    }
    let home = std::env::var_os("HOME")?;
    let dir = PathBuf::from(home).join(".local/share/nknown/models");
    dir.is_dir().then_some(dir)
}

fn read_rows(path: &Path) -> Vec<Row> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|line| {
            let cols: Vec<&str> = line.split('\t').collect();
            assert_eq!(cols.len(), 7, "ligne invalide : {line}");
            Row {
                lang: Lang::from_code(cols[0]).unwrap_or_else(|| panic!("langue : {line}")),
                query: cols[1].to_owned(),
                positive: cols[2].to_owned(),
                negative: cols[3].to_owned(),
                distractor: cols[4].to_owned(),
                category: cols[5].to_owned(),
                note: cols[6].to_owned(),
            }
        })
        .collect()
}

#[derive(Debug, Clone, Copy)]
struct Found {
    rank: Option<usize>,
    score: f64,
}

fn find(hits: &[nknown_engine::Hit], external_id: &str) -> Found {
    hits.iter()
        .enumerate()
        .find(|(_, h)| h.doc.external_id.as_deref() == Some(external_id))
        .map_or(
            Found {
                rank: None,
                score: 0.0,
            },
            |(i, h)| Found {
                rank: Some(i + 1),
                score: h.score,
            },
        )
}

fn index_doc(
    engine: &mut Engine<MemStorage>,
    pipeline: &nknown_nlp::Pipeline,
    lang: Lang,
    text: &str,
    external_id: &str,
) {
    let analysis = pipeline.analyze_light(lang, text);
    let item = RawItem {
        external_id: Some(external_id.to_owned()),
        ..RawItem::new(
            SourceId("test:expansion".to_owned()),
            text.to_owned(),
            Timestamp::now(),
        )
    };
    engine
        .ingest(nknown_engine::Document {
            item: &item,
            kind: SourceKind::LocalFile,
            lang,
            analysis: &analysis,
            title: None,
        })
        .unwrap();
}

#[derive(Debug, Clone, Copy)]
struct Metrics {
    positives: usize,
    positive_total: usize,
    hard_false_positives: usize,
    negative_total: usize,
    distractors: usize,
    rows: usize,
    median_ms: f64,
    max_ms: f64,
}

fn run_level(rows: &[Row], pipeline: &nknown_nlp::Pipeline, level: Level) -> Metrics {
    let detector = LangDetector::new();
    let now = Timestamp::now();
    let (mut pos_ok, mut pos_total) = (0usize, 0usize);
    let (mut fp, mut neg_total) = (0usize, 0usize);
    let mut dist_hits = 0usize;
    let mut times = Vec::new();
    let mut failures = Vec::new();
    for row in rows {
        let mut engine = Engine::new(MemStorage::default()).with_stemmer(nknown_nlp::langid::stem);
        let suffix = suffix(row.lang);
        index_doc(
            &mut engine,
            pipeline,
            row.lang,
            &format!("Note : {} {suffix}", row.positive),
            "positive",
        );
        index_doc(
            &mut engine,
            pipeline,
            row.lang,
            &format!("Note : {} {suffix}", row.negative),
            "negative",
        );
        index_doc(
            &mut engine,
            pipeline,
            row.lang,
            &format!("Note : {}", row.distractor),
            "distractor",
        );
        let start = std::time::Instant::now();
        let mut q = query::build_query_in_variety(
            pipeline,
            Some(&detector),
            &row.query,
            &engine,
            level.variety(),
        );
        query::add_learned(pipeline, &engine, &mut q);
        let hits = engine.search(&q, now, 10).unwrap();
        let elapsed = start.elapsed();
        times.push(elapsed.as_secs_f64() * 1000.0);
        let pos = find(&hits, "positive");
        let neg = find(&hits, "negative");
        let dist = find(&hits, "distractor");
        let ok = pos.rank.is_some() && pos.score > neg.score * 1.05;
        let neg_wins = neg.rank.is_some() && neg.score > pos.score * 1.05;
        let is_positive = !matches!(
            row.category.as_str(),
            "antonym" | "homonym" | "broad" | "code" | "crosslang"
        );
        if is_positive {
            pos_total += 1;
            pos_ok += usize::from(ok);
        } else {
            neg_total += 1;
            fp += usize::from(neg_wins);
        }
        dist_hits += usize::from(dist.rank.is_some());
        if std::env::var_os("NKNOWN_ALL").is_some() {
            let pos_words: Vec<&str> = hits
                .iter()
                .find(|h| h.doc.external_id.as_deref() == Some("positive"))
                .map(|h| h.matched.iter().map(String::as_str).collect())
                .unwrap_or_default();
            let neg_words: Vec<&str> = hits
                .iter()
                .find(|h| h.doc.external_id.as_deref() == Some("negative"))
                .map(|h| h.matched.iter().map(String::as_str).collect())
                .unwrap_or_default();
            println!(
                "  {:<7} {:<28} pos {:?} {:.3} {:?} / neg {:?} {:.3} {:?}",
                level.name(),
                row.query,
                pos.rank,
                pos.score,
                pos_words,
                neg.rank,
                neg.score,
                neg_words
            );
        }
        if !ok {
            failures.push(format!(
                "{} {:<30} {:<8} pos {} {:.3} / neg {} {:.3}",
                row.lang.as_str(),
                format!("\"{}\"", row.query),
                row.category,
                pos.rank.map_or("-".to_owned(), |r| r.to_string()),
                pos.score,
                neg.rank.map_or("-".to_owned(), |r| r.to_string()),
                neg.score
            ));
            if let Some(last) = failures.last_mut() {
                last.push_str(&format!("  [{}]", row.note));
            }
        }
    }
    times.sort_by(f64::total_cmp);
    let metrics = Metrics {
        positives: pos_ok,
        positive_total: pos_total,
        hard_false_positives: fp,
        negative_total: neg_total,
        distractors: dist_hits,
        rows: rows.len(),
        median_ms: times.get(times.len() / 2).copied().unwrap_or(0.0),
        max_ms: times.last().copied().unwrap_or(0.0),
    };
    println!(
        "{:<8} positifs {:>2}/{:<2}  faux positifs durs {:>2}/{:<2}  distrayants {:>2}/{:<2}  \
         latence médiane {:.2} ms  max {:.2} ms",
        level.name(),
        metrics.positives,
        metrics.positive_total,
        metrics.hard_false_positives,
        metrics.negative_total,
        metrics.distractors,
        metrics.rows,
        metrics.median_ms,
        metrics.max_ms
    );
    if std::env::var_os("NKNOWN_VERBOSE").is_some() {
        for failure in &failures {
            println!("  échec {failure}");
        }
    }
    metrics
}

fn metrics_for(rows: &[Row], pipeline: &nknown_nlp::Pipeline) -> [Metrics; 3] {
    let mut out = [Metrics {
        positives: 0,
        positive_total: 0,
        hard_false_positives: 0,
        negative_total: 0,
        distractors: 0,
        rows: 0,
        median_ms: 0.0,
        max_ms: 0.0,
    }; 3];
    for (slot, level) in out.iter_mut().zip(Level::ALL) {
        *slot = run_level(rows, pipeline, level);
    }
    out
}

#[test]
fn expansion_levels_dev() {
    let Some(models) = models_dir() else {
        eprintln!("modèles absents : test ignoré (définir NKNOWN_MODELS)");
        return;
    };
    let pipeline = load_pipeline(&models, &[]).unwrap();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../eval/expansion.tsv");
    let rows = read_rows(&path);
    println!("\n=== développement ({}) ===", rows.len());
    let [strict, normal, diverse] = metrics_for(&rows, &pipeline);
    assert!(
        normal.positives >= 14,
        "Normal doit reproduire le comportement historique : {}/{}, attendu ≥ 14",
        normal.positives,
        normal.positive_total
    );
    assert!(
        strict.positives >= 13,
        "Strict : {}/{}, attendu ≥ 13",
        strict.positives,
        strict.positive_total
    );
    assert!(
        diverse.positives >= 16,
        "Diversifié : {}/{}, attendu ≥ 16",
        diverse.positives,
        diverse.positive_total
    );
    assert!(
        diverse.hard_false_positives <= 4,
        "Diversifié : {} faux positifs durs, attendu ≤ 4",
        diverse.hard_false_positives
    );
    assert_eq!(diverse.distractors, 0, "un distrayant ne doit pas sortir");
}

#[test]
#[ignore = "jeu tenu à l'écart : vérification finale seulement"]
fn expansion_levels_heldout() {
    let Some(models) = models_dir() else {
        eprintln!("modèles absents : test ignoré (définir NKNOWN_MODELS)");
        return;
    };
    let pipeline = load_pipeline(&models, &[]).unwrap();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../eval/expansion-heldout.tsv");
    let rows = read_rows(&path);
    println!("\n=== tenu à l'écart ({}) ===", rows.len());
    let [_strict, normal, diverse] = metrics_for(&rows, &pipeline);
    // H4 : pas de régression de rappel ni de faux positifs durs par rapport à Normal.
    assert!(
        diverse.positives >= normal.positives,
        "Diversifié {} < Normal {} (positifs)",
        diverse.positives,
        normal.positives
    );
    assert!(
        diverse.hard_false_positives <= normal.hard_false_positives,
        "Diversifié {} > Normal {} (faux positifs durs)",
        diverse.hard_false_positives,
        normal.hard_false_positives
    );
}

/// Documents PHP du cas `php flaw` dans le top 10 : ceux qui contiennent « vulnerability »,
/// hors document de requête `debug_query`.
fn php_documents(hits: &[nknown_engine::Hit]) -> usize {
    hits.iter()
        .filter(|h| {
            h.doc.title.as_deref() != Some("debug_query")
                && h.matched.iter().any(|w| w == "vulnerability")
        })
        .count()
}

/// Sonde sur la base réelle : rangs des documents PHP pour « php flaw » et « php vulnerability ».
#[test]
#[ignore = "base réelle : NKNOWN_BASE=base.redb"]
fn expansion_levels_real_corpus() {
    let (Some(models), Some(base)) = (models_dir(), std::env::var_os("NKNOWN_BASE")) else {
        return;
    };
    let pipeline = load_pipeline(&models, &[]).unwrap();
    let engine =
        Engine::new(RedbStorage::open(base).unwrap()).with_stemmer(nknown_nlp::langid::stem);
    let detector = LangDetector::new();
    let now = Timestamp::now();
    for text in ["php flaw", "flaw", "php vulnerability", "php faille"] {
        println!("\n=== requête \"{text}\" ===");
        for level in Level::ALL {
            let start = std::time::Instant::now();
            let mut q = query::build_query_in_variety(
                &pipeline,
                Some(&detector),
                text,
                &engine,
                level.variety(),
            );
            query::add_learned(&pipeline, &engine, &mut q);
            let hits = engine.search(&q, now, 10).unwrap();
            let ms = start.elapsed().as_secs_f64() * 1000.0;
            println!("{:<7} {:.1} ms", level.name(), ms);
            if level == Level::Diverse || std::env::var_os("NKNOWN_REL").is_some() {
                for t in &q.terms {
                    println!(
                        "  mot {} liés {:?}",
                        t.text,
                        t.related
                            .iter()
                            .take(24)
                            .map(|(_, w, p)| format!("{w}:{p:.2}"))
                            .collect::<Vec<_>>()
                    );
                }
            }
            for (i, h) in hits.iter().take(10).enumerate() {
                println!(
                    "  {}. {:.3} {}  mots {:?}  texte {:.2} couv {:.2} titre {:.2} graphe {:.2}",
                    i + 1,
                    h.score,
                    h.doc.title.as_deref().unwrap_or("(sans titre)"),
                    h.matched,
                    h.parts.text,
                    h.parts.coverage,
                    h.parts.title,
                    h.parts.graph
                );
            }
            if text == "php flaw" {
                println!(
                    "  documents PHP dans le top 10 : {}/5",
                    php_documents(&hits)
                );
                match level {
                    Level::Normal => assert_eq!(
                        php_documents(&hits),
                        0,
                        "Normal ne doit trouver aucun document PHP pour « php flaw »"
                    ),
                    Level::Diverse => assert!(
                        php_documents(&hits) >= 4,
                        "Diversifié doit trouver au moins 4 des 5 documents PHP"
                    ),
                    Level::Strict => {}
                }
            }
        }
    }
}

/// Embouteillage des mots liés : huit liens du sens « chat » de `car` occupent les
/// `RELATED_KEPT` places avant `automobile`, le seul lien du bon sens présent dans la base.
#[test]
fn related_kept_crowding() {
    let Some(models) = models_dir() else {
        return;
    };
    let pipeline = load_pipeline(&models, &[]).unwrap();
    let detector = LangDetector::new();
    let now = Timestamp::now();
    let crowd = [
        "flawless",
        "tumult",
        "uproar",
        "quarrel",
        "imperfection",
        "windflaw",
        "imperfect",
        "unflawed",
        "flawed",
    ];
    for level in Level::ALL {
        let mut engine = Engine::new(MemStorage::default()).with_stemmer(nknown_nlp::langid::stem);
        for (i, w) in crowd.iter().enumerate() {
            index_doc(
                &mut engine,
                &pipeline,
                Lang::from_code("eng").unwrap(),
                &format!("Note : {w} affects the authentication service."),
                &format!("crowd-{i}"),
            );
        }
        index_doc(
            &mut engine,
            &pipeline,
            Lang::from_code("eng").unwrap(),
            "Note : defect affects the authentication service.",
            "defect",
        );
        let mut q = query::build_query_in_variety(
            &pipeline,
            Some(&detector),
            "flaw",
            &engine,
            level.variety(),
        );
        query::add_learned(&pipeline, &engine, &mut q);
        if std::env::var_os("NKNOWN_CROWD").is_some() {
            println!(
                "{:<7} liés {:?}",
                level.name(),
                q.terms[0]
                    .related
                    .iter()
                    .take(14)
                    .map(|(_, w, p)| format!("{w}:{p:.2}"))
                    .collect::<Vec<_>>()
            );
        }
        let hits = engine.search(&q, now, 10).unwrap();
        let defect = find(&hits, "defect");
        println!(
            "  tous {:?}",
            hits.iter()
                .map(|h| (h.doc.external_id.as_deref().unwrap_or(""), h.score))
                .collect::<Vec<_>>()
        );
        println!(
            "{:<7} defect rang {:?} score {:.3} ; premiers {:?}",
            level.name(),
            defect.rank,
            defect.score,
            hits.iter()
                .take(3)
                .map(|h| h.doc.external_id.as_deref().unwrap_or(""))
                .collect::<Vec<_>>()
        );
        match level {
            // Huit liens du mauvais sens remplissent les places retenues : le bon lien
            // n'apparaît qu'en Diversifié.
            Level::Strict | Level::Normal => assert_eq!(defect.rank, None, "{level:?}"),
            Level::Diverse => assert!(defect.rank.is_some(), "Diversifié doit garder defect"),
        }
    }
}
