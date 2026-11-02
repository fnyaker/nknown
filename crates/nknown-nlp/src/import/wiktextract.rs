//! Import des dumps Wiktextract (JSONL extrait du Wiktionary anglais, https://kaikki.org).
//!
//! Un seul schéma pour toutes les langues : le même import sert au français, à l'allemand, etc.
//! Pour chaque entrée, on enregistre :
//! - le mot lui-même comme lemme, une analyse par genre rencontré dans ses sens ;
//! - ses formes fléchies (`forms`), avec les traits tirés de leurs tags ;
//! - les renvois « forme de » (`form_of`) et « variante de » (`alt_of`) : le mot est alors
//!   une flexion, une graphie alternative ou une élision (« l' » → le, la) d'un autre lemme.
//!
//! Seules les formes d'un seul mot (au sens de [`crate::tokenize::words`]) sont gardées,
//! sinon elles ne pourraient jamais être retrouvées à l'analyse.

use std::io::BufRead;

use nknown_core::analyze::{Feats, Gender, Number, Pos};
use serde::Deserialize;

use super::{ImportError, ImportStats, is_single_word, strip_stress};
use crate::lexicon::LexiconBuilder;

#[derive(Deserialize)]
struct Entry {
    word: String,
    lang_code: String,
    pos: String,
    #[serde(default)]
    forms: Vec<Form>,
    #[serde(default)]
    senses: Vec<Sense>,
}

#[derive(Deserialize)]
struct Form {
    form: String,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Deserialize)]
struct Sense {
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    form_of: Vec<FormOf>,
    #[serde(default)]
    alt_of: Vec<FormOf>,
}

#[derive(Deserialize)]
struct FormOf {
    word: String,
}

/// Tags de `forms` qui ne désignent pas une vraie forme fléchie (métadonnées de tableaux, etc.).
const SKIPPED_FORM_TAGS: &[&str] = &[
    "table-tags",
    "inflection-template",
    "romanization",
    "multiword-construction",
    "canonical",
    "class",
    "auxiliary",
];

/// Importe les entrées de langue `lang_code` (code Wiktionary : `fr`, `en`, `de`...).
pub fn import(
    reader: impl BufRead,
    lang_code: &str,
    builder: &mut LexiconBuilder,
) -> Result<ImportStats, ImportError> {
    let mut stats = ImportStats::default();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        stats.entries += 1;
        let Ok(entry) = serde_json::from_str::<Entry>(&line) else {
            stats.invalid += 1;
            continue;
        };
        if entry.lang_code != lang_code {
            continue;
        }
        let Some(pos) = map_pos(&entry.pos) else {
            continue;
        };
        stats.used += 1;
        import_entry(&entry, pos, &mut |form, lemma, feats| {
            // Accents toniques retirés avant tout contrôle : ce sont des caractères
            // combinants, qui couperaient le mot en deux (краси́вые).
            let form = strip_stress(form);
            if is_single_word(&form) {
                builder.insert(&form, &strip_stress(lemma), pos, feats);
                stats.analyses += 1;
            }
        });
    }
    Ok(stats)
}

fn import_entry(entry: &Entry, pos: Pos, add: &mut impl FnMut(&str, &str, Feats)) {
    // Sens « forme de » / « variante de » : le mot renvoie à un autre lemme.
    for sense in &entry.senses {
        for target in sense.form_of.iter().chain(&sense.alt_of) {
            add(&entry.word, &target.word, feats_from_tags(&sense.tags));
        }
    }

    let lemma_senses: Vec<&Sense> = entry
        .senses
        .iter()
        .filter(|s| s.form_of.is_empty() && s.alt_of.is_empty() && !has(&s.tags, "alt-of"))
        .collect();
    if lemma_senses.is_empty() {
        return;
    }

    // Un genre par sens possible : « livre » est masculin (ouvrage) et féminin (monnaie).
    let mut genders: Vec<Option<Gender>> = Vec::new();
    for sense in &lemma_senses {
        if let Some(g) = gender_from_tags(&sense.tags)
            && !genders.contains(&Some(g))
        {
            genders.push(Some(g));
        }
    }
    if genders.is_empty() {
        // Adjectif avec des formes féminines : le lemme est la forme masculine.
        let has_feminine_form = entry.forms.iter().any(|f| has(&f.tags, "feminine"));
        genders.push((pos == Pos::Adj && has_feminine_form).then_some(Gender::Masc));
    }

    let invariable = lemma_senses.iter().all(|s| has(&s.tags, "invariable"));
    let plurale_tantum = lemma_senses
        .iter()
        .all(|s| has(&s.tags, "plural") && !has(&s.tags, "singular"));
    let number = if plurale_tantum {
        Some(Number::Plur)
    } else if matches!(pos, Pos::Noun | Pos::Adj) && !invariable {
        Some(Number::Sing)
    } else {
        None
    };

    for &gender in &genders {
        add(&entry.word, &entry.word, Feats { gender, number });
    }

    for form in &entry.forms {
        if form
            .tags
            .iter()
            .any(|t| SKIPPED_FORM_TAGS.contains(&t.as_str()))
        {
            continue;
        }
        let feats = feats_from_tags(&form.tags);
        if feats.gender.is_none() && pos == Pos::Noun {
            // Un nom garde son genre à toutes ses formes (« livres » reste masculin).
            for &gender in &genders {
                add(&form.form, &entry.word, Feats { gender, ..feats });
            }
        } else {
            add(&form.form, &entry.word, feats);
        }
    }
}

fn map_pos(pos: &str) -> Option<Pos> {
    Some(match pos {
        "noun" => Pos::Noun,
        "name" => Pos::Propn,
        "verb" => Pos::Verb,
        "adj" => Pos::Adj,
        "adv" => Pos::Adv,
        "pron" => Pos::Pron,
        "det" | "article" => Pos::Det,
        "prep" | "postp" => Pos::Adp,
        "conj" => Pos::Cconj,
        "intj" => Pos::Intj,
        "num" => Pos::Num,
        "particle" => Pos::Part,
        "punct" => Pos::Punct,
        "symbol" => Pos::Sym,
        // Fiche de caractère (sinogrammes surtout) : souvent la seule entrée d'un mot
        // d'une syllabe (的, 在, 是). Mot connu, catégorie laissée au tagueur.
        "character" => Pos::X,
        // Affixes, locutions, proverbes, caractères... : pas des mots autonomes.
        _ => return None,
    })
}

fn has(tags: &[String], tag: &str) -> bool {
    tags.iter().any(|t| t == tag)
}

fn gender_from_tags(tags: &[String]) -> Option<Gender> {
    let (m, f, n) = (
        has(tags, "masculine"),
        has(tags, "feminine"),
        has(tags, "neuter"),
    );
    match (m, f, n) {
        (true, false, false) => Some(Gender::Masc),
        (false, true, false) => Some(Gender::Fem),
        (false, false, true) => Some(Gender::Neut),
        // Aucun genre, ou plusieurs (épicène) : non renseigné.
        _ => None,
    }
}

fn feats_from_tags(tags: &[String]) -> Feats {
    let number = match (has(tags, "singular"), has(tags, "plural")) {
        (true, false) => Some(Number::Sing),
        (false, true) => Some(Number::Plur),
        _ => None,
    };
    Feats {
        gender: gender_from_tags(tags),
        number,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexicon::Lexicon;

    const DUMP: &str = r#"
{"word": "livre", "lang_code": "fr", "pos": "noun", "forms": [{"form": "livres", "tags": ["plural"]}], "senses": [{"tags": ["masculine"]}]}
{"word": "livre", "lang_code": "fr", "pos": "noun", "forms": [{"form": "livres", "tags": ["plural"]}], "senses": [{"tags": ["feminine"]}]}
{"word": "livre", "lang_code": "fr", "pos": "verb", "senses": [{"tags": ["form-of", "singular", "present"], "form_of": [{"word": "livrer"}]}]}
{"word": "grand", "lang_code": "fr", "pos": "adj", "forms": [{"form": "grande", "tags": ["feminine", "singular"]}, {"form": "grands", "tags": ["masculine", "plural"]}, {"form": "fr-adj", "tags": ["inflection-template"]}], "senses": [{}]}
{"word": "l'", "lang_code": "fr", "pos": "article", "senses": [{"tags": ["alt-of", "feminine", "masculine", "singular"], "alt_of": [{"word": "le"}, {"word": "la"}]}]}
{"word": "pomme de terre", "lang_code": "fr", "pos": "noun", "senses": [{"tags": ["feminine"]}]}
{"word": "-isme", "lang_code": "fr", "pos": "suffix", "senses": [{}]}
{"word": "book", "lang_code": "en", "pos": "noun", "senses": [{}]}
pas du json
"#;

    fn build() -> (Lexicon, ImportStats) {
        let mut b = LexiconBuilder::new();
        let stats = import(DUMP.as_bytes(), "fr", &mut b).unwrap();
        (b.build().unwrap(), stats)
    }

    #[test]
    fn stats() {
        let (_, stats) = build();
        assert_eq!(stats.entries, 9);
        assert_eq!(stats.invalid, 1);
        assert_eq!(stats.used, 6); // suffixe et anglais exclus
    }

    #[test]
    fn noun_genders_and_inflections() {
        let (lex, _) = build();
        let livre: Vec<_> = lex.lookup("livre").collect();
        assert_eq!(livre.len(), 3);
        assert!(
            livre
                .iter()
                .any(|e| e.pos == Pos::Verb && e.lemma == "livrer")
        );
        let livres: Vec<_> = lex.lookup("livres").collect();
        assert_eq!(livres.len(), 2);
        assert!(livres.iter().all(|e| e.lemma == "livre"
            && e.feats.number == Some(Number::Plur)
            && e.feats.gender.is_some()));
    }

    #[test]
    fn adjective_forms() {
        let (lex, _) = build();
        let grand = lex.lookup("grand").next().unwrap();
        assert_eq!(grand.feats.gender, Some(Gender::Masc));
        let grande = lex.lookup("grande").next().unwrap();
        assert_eq!(
            (grande.lemma, grande.feats.gender, grande.feats.number),
            ("grand", Some(Gender::Fem), Some(Number::Sing))
        );
        assert_eq!(lex.lookup("fr-adj").count(), 0);
    }

    #[test]
    fn elisions_point_to_their_lemmas() {
        let (lex, _) = build();
        let l: Vec<_> = lex.lookup("l\u{2019}").map(|e| (e.lemma, e.pos)).collect();
        assert_eq!(l, [("le", Pos::Det), ("la", Pos::Det)]);
    }

    #[test]
    fn multiword_entries_are_skipped() {
        let (lex, _) = build();
        assert_eq!(lex.lookup("pomme de terre").count(), 0);
        assert_eq!(lex.lookup("book").count(), 0);
    }

    #[test]
    fn russian_stress_marks_are_removed() {
        let dump = r#"{"word": "красивый", "lang_code": "ru", "pos": "adj", "forms": [{"form": "краси́вые", "tags": ["nominative", "plural"]}], "senses": [{}]}"#;
        let mut b = LexiconBuilder::new();
        import(dump.as_bytes(), "ru", &mut b).unwrap();
        let lex = b.build().unwrap();
        let e = lex
            .lookup("красивые")
            .next()
            .expect("forme trouvée sans accent");
        assert_eq!((e.lemma, e.feats.number), ("красивый", Some(Number::Plur)));
    }
}
