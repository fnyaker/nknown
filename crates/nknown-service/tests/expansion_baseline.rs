//! Harnais d'évaluation de l'élargissement de requête, niveau Normal (comportement actuel).
//!
//! Chaque ligne de `eval/expansion.tsv` construit trois documents contrôlés (positif,
//! négatif, distrayant) dans une base en mémoire, puis une recherche de référence.
//! On mesure : le positif bat-il le négatif, le négatif gagne-t-il (faux positif),
//! le distrayant apparaît-il, et combien de temps coûte la recherche.
//!
//! Les modèles réels sont lus dans `NKNOWN_MODELS`, sinon dans
//! `~/.local/share/nknown/models`. Sans modèles, le test est ignoré (CI sans corpus).
//! Le jeu tenu à l'écart ne s'exécute que sur demande :
//! `cargo test --release -p nknown-service --test expansion_baseline -- --ignored`.

use std::path::{Path, PathBuf};

use nknown_core::item::RawItem;
use nknown_core::lang::Lang;
use nknown_core::source::{SourceId, SourceKind};
use nknown_core::time::Timestamp;
use nknown_engine::{Engine, MemStorage};
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

/// Lit un TSV : `lang query positif négatif distrayant catégorie note`.
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

/// Résultat d'une recherche pour un document (rang 1-based, score).
#[derive(Debug, Clone, Copy)]
struct Found {
    rank: Option<usize>,
    score: f64,
}

/// Métriques du harnais Normal, pour des seuils explicites par jeu.
#[derive(Debug, Clone, Copy)]
struct Metrics {
    positives: usize,
    positive_total: usize,
    hard_false_positives: usize,
    negative_total: usize,
    distractors: usize,
    rows: usize,
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
    if std::env::var_os("NKNOWN_DEBUG").is_some() {
        println!(
            "  doc {external_id}: {:?}",
            analysis
                .tokens
                .iter()
                .map(|t| (
                    t.lemma.as_str(),
                    format!("{:?}", t.pos),
                    format!("{:?}", t.domain)
                ))
                .collect::<Vec<_>>()
        );
    }
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

fn main_metrics(rows: &[Row], pipeline: &nknown_nlp::Pipeline, level: &str) -> Metrics {
    let detector = LangDetector::new();
    let now = Timestamp::now();
    let (mut pos_ok, mut pos_total) = (0usize, 0usize);
    let (mut neg_ok, mut neg_total) = (0usize, 0usize);
    let (mut dist_hits, mut dist_total) = (0usize, 0usize);
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
        let mut q = query::build_query_in(pipeline, Some(&detector), &row.query, &engine);
        query::add_learned(pipeline, &engine, &mut q);
        let start = std::time::Instant::now();
        let hits = engine.search(&q, now, 10).unwrap();
        let elapsed = start.elapsed();
        times.push(elapsed.as_secs_f64() * 1000.0);
        let pos = find(&hits, "positive");
        let neg = find(&hits, "negative");
        let dist = find(&hits, "distractor");
        // Un écart de moins de 5 % n'est pas une victoire : les égalités (aucun
        // élargissement) doivent compter comme des échecs, pas comme un ordre stable.
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
            neg_ok += usize::from(!neg_wins);
        }
        dist_total += 1;
        dist_hits += usize::from(dist.rank.is_some());
        if !ok {
            failures.push(format!(
                "{} {:<28} {:<18} pos {} {:.3} / neg {} {:.3}",
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
            if std::env::var_os("NKNOWN_DEBUG").is_some() {
                for t in &q.terms {
                    println!(
                        "  mot « {} » lectures={:?} liés={:?}",
                        t.text,
                        t.readings
                            .iter()
                            .map(|(l, k, w)| (l.as_str(), format!("{k:?}"), w.as_str()))
                            .collect::<Vec<_>>(),
                        t.related
                            .iter()
                            .map(|(l, w, p)| (l.as_str(), w.as_str(), *p))
                            .collect::<Vec<_>>()
                    );
                }
                for h in &hits {
                    println!(
                        "  doc {:?} score {:.3} mots {:?} texte {:.3} couv {:.3}",
                        h.doc.external_id, h.score, h.matched, h.parts.text, h.parts.coverage
                    );
                }
            }
        }
        println!(
            "niveau={level} lang={} cat={:<8} requête=\"{}\" pos={:?}/{:.3} neg={:?}/{:.3} dist={:?}/{:.3} {}",
            row.lang.as_str(),
            row.category,
            row.query,
            pos.rank,
            pos.score,
            neg.rank,
            neg.score,
            dist.rank,
            dist.score,
            if ok { "OK" } else { "ÉCHEC" }
        );
    }
    times.sort_by(f64::total_cmp);
    let metrics = Metrics {
        positives: pos_ok,
        positive_total: pos_total,
        hard_false_positives: neg_total - neg_ok,
        negative_total: neg_total,
        distractors: dist_hits,
        rows: rows.len(),
    };
    println!(
        "\nniveau={level} : positifs {pos_ok}/{pos_total} ; faux positifs durs {}/{neg_total} ; \
         distrayants retournés {dist_hits}/{dist_total} ; latence recherche médiane {:.2} ms, max {:.2} ms",
        metrics.hard_false_positives,
        times.get(times.len() / 2).copied().unwrap_or(0.0),
        times.last().copied().unwrap_or(0.0)
    );
    if !failures.is_empty() {
        println!("échecs et égalités :");
        for f in failures {
            println!("  {f}");
        }
    }
    assert!(
        pos_total > 0,
        "jeu vide : aucun cas positif dans l'évaluation"
    );
    metrics
}

#[test]
fn expansion_baseline_dev() {
    let Some(models) = models_dir() else {
        eprintln!("modèles absents : test ignoré (définir NKNOWN_MODELS)");
        return;
    };
    let pipeline = load_pipeline(&models, &[]).unwrap();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../eval/expansion.tsv");
    let rows = read_rows(&path);
    let metrics = main_metrics(&rows, &pipeline, "normal");
    // Seuils de non-régression du niveau Normal sur le jeu de développement.
    assert!(
        metrics.positives >= 14,
        "Normal doit reproduire le comportement historique : {}/{}, attendu ≥ 14",
        metrics.positives,
        metrics.positive_total
    );
    assert!(
        metrics.hard_false_positives <= 3,
        "Normal : {} faux positifs durs, attendu ≤ 3 (sur {})",
        metrics.hard_false_positives,
        metrics.negative_total
    );
    assert_eq!(
        metrics.distractors, 0,
        "Normal ne doit retourner aucun distrayant (sur {})",
        metrics.rows
    );
}

#[test]
#[ignore = "jeu tenu à l'écart : vérification finale seulement"]
fn expansion_baseline_heldout() {
    let Some(models) = models_dir() else {
        eprintln!("modèles absents : test ignoré (définir NKNOWN_MODELS)");
        return;
    };
    let pipeline = load_pipeline(&models, &[]).unwrap();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../eval/expansion-heldout.tsv");
    let rows = read_rows(&path);
    main_metrics(&rows, &pipeline, "normal-heldout");
}

/// Sonde : liens appris du corpus pour quelques mots (base réelle, lecture seule).
#[test]
#[ignore = "diagnostic manuel : NKNOWN_BASE=base.redb"]
fn learned_probe() {
    use nknown_core::analyze::Pos;
    use nknown_engine::RedbStorage;
    use nknown_engine::schema::TermKind;
    let Some(base) = std::env::var_os("NKNOWN_BASE") else {
        return;
    };
    let engine = Engine::new(RedbStorage::open(base).unwrap());
    for (code, pos, word) in [
        ("eng", Pos::Noun, "flaw"),
        ("eng", Pos::Noun, "vulnerability"),
        ("eng", Pos::Noun, "php"),
        ("eng", Pos::Noun, "bug"),
        ("fra", Pos::Noun, "faille"),
        ("fra", Pos::Noun, "vulnérabilité"),
        ("fra", Pos::Noun, "défaut"),
    ] {
        let lang = Lang::from_code(code).unwrap();
        let links = engine.learned(lang, TermKind::Word(pos), word).unwrap();
        println!(
            "{code} {word}: {:?}",
            links
                .iter()
                .map(|l| (l.lemma.as_str(), l.cosine, l.shared, l.kind))
                .collect::<Vec<_>>()
        );
    }
}

/// Sonde : liens, sens et mots plus généraux d'un lemme dans le dictionnaire.
#[test]
#[ignore = "diagnostic manuel"]
fn relations_probe() {
    let Some(models) = models_dir() else {
        return;
    };
    let pipeline = load_pipeline(&models, &[]).unwrap();
    for (lang, word) in [
        (Lang::from_code("eng").unwrap(), "flaw"),
        (Lang::from_code("eng").unwrap(), "vulnerability"),
        (Lang::FRA, "faille"),
    ] {
        let Some(rel) = pipeline.relations(lang) else {
            continue;
        };
        let rec = rel.record(word);
        println!(
            "{word} ({}): liens {:?} ; contraires {:?} ; plus généraux {:?}",
            lang.as_str(),
            rec.links
                .iter()
                .map(|l| (l.word, l.weight))
                .collect::<Vec<_>>(),
            rec.antonyms,
            rec.broader
        );
        for (i, s) in rec.senses.iter().enumerate() {
            println!(
                "  sens {} (entrée {}) : {:?}",
                i + 1,
                s.group + 1,
                s.signature
            );
        }
    }
}
