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
use crate::relations::{Evidence, Origin, RelationsBuilder, gloss_strength, list_strength};
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
    #[serde(default)]
    antonyms: Vec<Item>,
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

/// Vrai si `a` est un autre mot que `b` (sans tenir compte de la casse).
fn is_other_word(a: &str, b: &str) -> bool {
    a.to_lowercase() != b.to_lowercase()
}

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
        let keep = |target: &str| -> Option<String> {
            let target = strip_stress(target);
            // Une lettre seule (« G » pour « portable ») n'est pas un mot à chercher.
            let short = target.chars().count() < 2 && !target.chars().any(tokenize::is_cjk);
            (!short && is_single_word(&target)).then_some(target)
        };
        let entry_id = builder.entry(&word);
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
        // Sens retenus (ni « forme de », ni au-delà de GLOSSED_SENSES), par position.
        let mut sense_ids = Vec::with_capacity(entry.senses.len());
        let mut kept = 0usize;
        for sense in &entry.senses {
            let inflection = !sense.form_of.is_empty()
                || !sense.alt_of.is_empty()
                || sense.tags.iter().any(|t| t == "form-of" || t == "alt-of");
            if inflection || kept >= GLOSSED_SENSES {
                sense_ids.push(None);
            } else {
                sense_ids.push(Some(builder.sense(entry_id)));
                kept += 1;
            }
        }
        // Mots des définitions : indices et signature de leur sens.
        if let Some(lex) = lexicon {
            let mut rank = 0usize;
            for (i, sense) in entry.senses.iter().enumerate() {
                let Some(sid) = sense_ids[i] else { continue };
                let r = rank
                    + if is_marginal(&sense.tags) {
                        MARGINAL_RANK
                    } else {
                        0
                    };
                for gloss in &sense.glosses {
                    for span in tokenize::words(gloss) {
                        let form = &gloss[span];
                        // La lecture la plus fréquente : « la » est un article, pas la note.
                        if let Some(e) = lex.lookup(form).next().filter(|e| is_content(e.pos))
                            && let Some(lemma) = keep(e.lemma)
                            && is_other_word(&lemma, &word)
                        {
                            builder.sign(sid, &lemma);
                            builder.link_from(
                                &word,
                                &lemma,
                                Evidence::Gloss,
                                gloss_strength(r),
                                Origin::Sense(sid),
                            );
                            stats.analyses += 1;
                        }
                    }
                }
                rank += 1;
            }
        }
        // Listes : de l'entrée (avec ou sans numéro de sens) ou d'un sens.
        let mut lists: Vec<(&Links, Option<usize>)> = vec![(&entry.links, None)];
        for (i, sense) in entry.senses.iter().enumerate() {
            lists.push((&sense.links, Some(i)));
        }
        for (l, in_sense) in lists {
            for item in &l.antonyms {
                if let Some(t) = keep(&item.word) {
                    builder.antonym(&word, &t);
                }
            }
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
                    let Some(target) = keep(&item.word) else {
                        continue;
                    };
                    // Position du sens : celui de la liste, ou le numéro donné (à partir de 1).
                    let at = in_sense.or_else(|| item.sense_index?.checked_sub(1));
                    let rank = at.map(|i| ranks.get(i).copied().unwrap_or(i));
                    let sid = at.and_then(|i| sense_ids.get(i).copied().flatten());
                    let origin = match sid {
                        Some(sid) => {
                            builder.sign(sid, &target);
                            Origin::Sense(sid)
                        }
                        None => Origin::Entry(entry_id),
                    };
                    let len = sizes[&item.sense_index];
                    builder.link_from(
                        &word,
                        &target,
                        e,
                        list_strength(len, rank.unwrap_or(0)),
                        origin,
                    );
                    stats.analyses += 1;
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
{"word": "portable", "lang_code": "fr", "pos": "noun", "synonyms": [{"word": "gsm", "sense_index": 1}, {"word": "mobile", "sense_index": 1}], "antonyms": [{"word": "fixe"}], "senses": [{"glosses": ["Téléphone portable ; téléphone cellulaire."]}, {"glosses": ["Ordinateur portable."]}]}
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
            ("téléphone", "téléphone", Pos::Noun),
            ("ordinateur", "ordinateur", Pos::Noun),
            ("portable", "portable", Pos::Adj),
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
        assert_eq!(stats.used, 5);
        let r = b
            .build(
                BuildOptions {
                    per_lemma: 20,
                    min_weight: 0.01,
                },
                |_| 1.0,
            )
            .unwrap();
        let of = |w: &str| {
            r.related(w)
                .into_iter()
                .map(|(x, _)| x.to_owned())
                .collect::<Vec<_>>()
        };
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

        // Sens : « gsm » (synonyme du sens 1) s'efface devant « ordinateur ».
        let rec = r.record("portable");
        assert_eq!(rec.senses.len(), 2, "{rec:?}");
        assert!(rec.senses[1].signature.contains(&"ordinateur"));
        assert_eq!(rec.antonyms, ["fixe"]);
        let ctx = |w: &str| if w == "ordinateur" { 1.0 } else { 0.0 };
        let gsm = |list: Vec<(&str, f32)>| list.iter().find(|(w, _)| *w == "gsm").unwrap().1;
        assert!(gsm(rec.in_context(&ctx)) <= 0.51 * gsm(rec.in_context(&|_| 0.0)));
    }
}
