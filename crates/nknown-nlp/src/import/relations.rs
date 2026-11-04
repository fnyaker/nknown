//! Indices de sens tirés d'un dump Wiktextract, vers un [`RelationsBuilder`].
//!
//! Par entrée : synonymes, hyperonymes et hyponymes, mots apparentés (au niveau de l'entrée
//! ou d'un sens), et mots de contenu des définitions, ramenés à leur lemme par le lexique de
//! la langue. Les définitions d'un dump dans une autre langue que celle des mots (Wiktionary
//! anglais pour le russe) ne donnent rien : le lexique n'en reconnaît pas les mots.
//!
//! Les sens « forme de » (« Pluriel de élection. ») sont sautés : ils ne disent rien du sens.

use std::io::BufRead;

use nknown_core::analyze::Pos;
use serde::Deserialize;

use super::{ImportError, ImportStats, is_single_word, strip_stress};
use crate::lexicon::Lexicon;
use crate::relations::{Evidence, RelationsBuilder, gloss_strength, list_strength};
use crate::tokenize;

#[derive(Deserialize)]
struct Entry {
    word: String,
    lang_code: String,
    pos: String,
    #[serde(default)]
    senses: Vec<Sense>,
    #[serde(flatten)]
    links: Links,
}

#[derive(Deserialize, Default)]
struct Links {
    #[serde(default)]
    synonyms: Vec<Item>,
    #[serde(default)]
    hypernyms: Vec<Item>,
    #[serde(default)]
    hyponyms: Vec<Item>,
    #[serde(default)]
    related: Vec<Item>,
}

#[derive(Deserialize)]
struct Sense {
    #[serde(default)]
    glosses: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    form_of: Vec<Item>,
    #[serde(default)]
    alt_of: Vec<Item>,
    #[serde(flatten)]
    links: Links,
}

#[derive(Deserialize)]
struct Item {
    #[serde(default)]
    word: String,
    /// Wiktionnaire français : sens (à partir de 1) auquel se rattache le mot lié.
    #[serde(default)]
    sense_index: Option<usize>,
}

/// Sens dont la définition est lue, au plus (les derniers sont rares et souvent techniques).
const GLOSSED_SENSES: usize = 8;

/// Rangs ajoutés à un sens vieilli, rare ou régional.
const MARGINAL_RANK: usize = 3;

fn is_marginal(tags: &[String]) -> bool {
    tags.iter().any(|t| {
        matches!(
            t.as_str(),
            "dated" | "archaic" | "obsolete" | "rare" | "historical" | "regional" | "dialectal"
        )
    })
}

/// Catégories dont on garde les liens : celles que la recherche indexe.
fn is_linked(pos: &str) -> bool {
    matches!(pos, "noun" | "name" | "verb" | "adj" | "adv")
}

fn is_content(pos: Pos) -> bool {
    matches!(
        pos,
        Pos::Noun | Pos::Propn | Pos::Verb | Pos::Adj | Pos::Adv
    )
}

/// Importe les indices des entrées de langue `lang_code`. `lexicon` (de la même langue)
/// ramène les mots des définitions à leur lemme ; sans lui, les définitions sont ignorées.
pub fn import(
    reader: impl BufRead,
    lang_code: &str,
    lexicon: Option<&Lexicon>,
    builder: &mut RelationsBuilder,
) -> Result<ImportStats, ImportError> {
    let mut stats = ImportStats::default();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        stats.entries += 1;
        // Filtre rapide avant d'analyser le JSON : la plupart des lignes d'un dump complet
        // sont d'une autre langue.
        if !line.contains(lang_code) {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Entry>(&line) else {
            stats.invalid += 1;
            continue;
        };
        if entry.lang_code != lang_code || !is_linked(&entry.pos) {
            continue;
        }
        let word = strip_stress(&entry.word);
        if !is_single_word(&word) {
            continue;
        }
        stats.used += 1;
        let mut add = |target: &str, e: Evidence, strength: f32| {
            let target = strip_stress(target);
            // Une lettre seule (« G » pour « portable ») n'est pas un mot à chercher.
            let short = target.chars().count() < 2 && !target.chars().any(tokenize::is_cjk);
            if !short && is_single_word(&target) {
                builder.link(&word, &target, e, strength);
                stats.analyses += 1;
            }
        };
        // Rang effectif de chaque sens : un sens vieilli ou rare compte comme plus lointain.
        let ranks: Vec<usize> = entry
            .senses
            .iter()
            .enumerate()
            .map(|(i, s)| {
                i + if is_marginal(&s.tags) {
                    MARGINAL_RANK
                } else {
                    0
                }
            })
            .collect();
        // (liens, rang du sens s'ils en dépendent).
        let mut links = vec![(&entry.links, None)];
        let mut rank = 0usize;
        for (i, sense) in entry.senses.iter().enumerate() {
            links.push((&sense.links, Some(ranks[i])));
            let inflection = !sense.form_of.is_empty()
                || !sense.alt_of.is_empty()
                || sense.tags.iter().any(|t| t == "form-of" || t == "alt-of");
            if inflection || rank >= GLOSSED_SENSES {
                continue;
            }
            if let Some(lex) = lexicon {
                for gloss in &sense.glosses {
                    for span in tokenize::words(gloss) {
                        let form = &gloss[span];
                        if let Some(e) = lex.lookup(form).find(|e| is_content(e.pos)) {
                            let r = rank
                                + if is_marginal(&sense.tags) {
                                    MARGINAL_RANK
                                } else {
                                    0
                                };
                            add(e.lemma, Evidence::Gloss, gloss_strength(r));
                        }
                    }
                }
            }
            rank += 1;
        }
        for (l, sense_rank) in links {
            for (items, e) in [
                (&l.synonyms, Evidence::Synonym),
                (&l.hypernyms, Evidence::Broader),
                (&l.hyponyms, Evidence::Narrower),
                (&l.related, Evidence::Related),
            ] {
                // Longueur de la liste du même sens.
                let mut sizes: std::collections::HashMap<Option<usize>, usize> =
                    std::collections::HashMap::new();
                for item in items {
                    *sizes.entry(item.sense_index).or_default() += 1;
                }
                for item in items {
                    let rank = sense_rank.or_else(|| {
                        let i = item.sense_index?.checked_sub(1)?;
                        Some(ranks.get(i).copied().unwrap_or(i))
                    });
                    let len = sizes[&item.sense_index];
                    add(&item.word, e, list_strength(len, rank.unwrap_or(0)));
                }
            }
        }
    }
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LexiconBuilder;
    use crate::relations::BuildOptions;
    use nknown_core::analyze::Feats;

    const DUMP: &str = r#"
{"word": "élection", "lang_code": "fr", "pos": "noun", "related": [{"word": "scrutin"}, {"word": "bureau de vote"}], "senses": [{"glosses": ["Choix fait par la voie des suffrages."]}]}
{"word": "élections", "lang_code": "fr", "pos": "noun", "senses": [{"glosses": ["Pluriel de élection."], "tags": ["form-of"], "form_of": [{"word": "élection"}]}]}
{"word": "scrutin", "lang_code": "fr", "pos": "noun", "synonyms": [{"word": "votation"}], "senses": [{"glosses": ["Vote au moyen de bulletins."]}, {"glosses": ["Ensemble des opérations qui constituent une élection."]}]}
{"word": "voiture", "lang_code": "fr", "pos": "noun", "senses": [{"glosses": ["Véhicule."], "synonyms": [{"word": "automobile"}]}]}
{"word": "car", "lang_code": "en", "pos": "noun", "synonyms": [{"word": "automobile"}]}
"#;

    fn lexicon() -> Lexicon {
        let mut b = LexiconBuilder::new();
        for (form, lemma, pos) in [
            ("choix", "choix", Pos::Noun),
            ("suffrages", "suffrage", Pos::Noun),
            ("vote", "vote", Pos::Noun),
            ("bulletins", "bulletin", Pos::Noun),
            ("élection", "élection", Pos::Noun),
            ("pluriel", "pluriel", Pos::Noun),
            ("la", "le", Pos::Det),
            ("des", "de", Pos::Adp),
        ] {
            b.insert(form, lemma, pos, Feats::default());
        }
        b.build().unwrap()
    }

    #[test]
    fn reads_links_and_glosses() {
        let lex = lexicon();
        let mut b = RelationsBuilder::new();
        let stats = import(DUMP.as_bytes(), "fr", Some(&lex), &mut b).unwrap();
        assert_eq!(stats.used, 4);
        let r = b
            .build(
                BuildOptions {
                    per_lemma: 20,
                    min_weight: 0.01,
                },
                |_| 1.0,
            )
            .unwrap();
        let of = |w: &str| r.related(w).map(|(x, _)| x.to_owned()).collect::<Vec<_>>();
        let election = of("élection");
        assert!(election.contains(&"scrutin".to_owned()), "{election:?}");
        assert!(election.contains(&"suffrage".to_owned()), "{election:?}");
        // Locution, mot outil, sens « forme de » : rien.
        assert!(
            !election
                .iter()
                .any(|w| w == "bureau de vote" || w == "le" || w == "pluriel")
        );
        assert!(of("scrutin").contains(&"votation".to_owned()));
        // Synonyme au niveau d'un sens.
        assert!(of("voiture").contains(&"automobile".to_owned()));
        // Autre langue : ignorée.
        assert!(!of("automobile").contains(&"car".to_owned()));
    }
}
