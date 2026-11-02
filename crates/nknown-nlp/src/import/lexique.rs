//! Import de Lexique 3.83 (http://www.lexique.org, licence CC BY-SA 4.0).
//!
//! Base du français contemporain : ~140 000 lignes (forme, lemme, catégorie, genre, nombre)
//! avec des fréquences par million de mots, mesurées sur des sous-titres de films et des livres.
//! Son apport principal : classer les lectures d'une forme (« été » : être avant le nom).
//!
//! Les colonnes sont repérées par leur nom dans l'en-tête, pas par leur position.

use std::io::BufRead;

use nknown_core::analyze::{Feats, Gender, Number, Pos};

use super::{ImportError, ImportStats, is_single_word};
use crate::lexicon::LexiconBuilder;

const COLUMNS: [&str; 7] = [
    "ortho",
    "lemme",
    "cgram",
    "genre",
    "nombre",
    "freqfilms2",
    "freqlivres",
];

pub fn import(
    reader: impl BufRead,
    builder: &mut LexiconBuilder,
) -> Result<ImportStats, ImportError> {
    let mut lines = reader.lines();
    let header = lines
        .next()
        .ok_or_else(|| ImportError::Format("fichier vide".into()))??;
    let header: Vec<&str> = header.split('\t').collect();
    let mut idx = [0usize; COLUMNS.len()];
    for (slot, name) in idx.iter_mut().zip(COLUMNS) {
        *slot = header
            .iter()
            .position(|h| *h == name)
            .ok_or_else(|| ImportError::Format(format!("colonne « {name} » absente")))?;
    }
    let [ortho, lemme, cgram, genre, nombre, freq_films, freq_livres] = idx;

    let mut stats = ImportStats::default();
    for line in lines {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        stats.entries += 1;
        let cols: Vec<&str> = line.split('\t').collect();
        let col = |i: usize| cols.get(i).copied().unwrap_or("");
        let (form, lemma) = (col(ortho), col(lemme));
        if form.is_empty() || lemma.is_empty() {
            stats.invalid += 1;
            continue;
        }
        let Some(pos) = map_cgram(col(cgram)) else {
            continue;
        };
        if !is_single_word(form) {
            continue;
        }
        let feats = Feats {
            gender: match col(genre) {
                "m" => Some(Gender::Masc),
                "f" => Some(Gender::Fem),
                _ => None,
            },
            number: match col(nombre) {
                "s" => Some(Number::Sing),
                "p" => Some(Number::Plur),
                _ => None,
            },
        };
        // Moyenne films / livres : l'oral et l'écrit comptent autant.
        let freq = (parse_freq(col(freq_films)) + parse_freq(col(freq_livres))) / 2.0;
        builder.insert_with_frequency(form, lemma, pos, feats, freq);
        stats.used += 1;
        stats.analyses += 1;
    }
    Ok(stats)
}

/// Catégories de Lexique vers UPOS. Les « adjectifs » possessifs, démonstratifs, indéfinis
/// et interrogatifs de la grammaire scolaire (mon, ce, chaque, quel) sont des déterminants en UD.
fn map_cgram(cgram: &str) -> Option<Pos> {
    Some(match cgram {
        "NOM" => Pos::Noun,
        "VER" => Pos::Verb,
        "AUX" => Pos::Aux,
        "ADJ" => Pos::Adj,
        "ADJ:num" => Pos::Num,
        "ADJ:pos" | "ADJ:dem" | "ADJ:ind" | "ADJ:int" => Pos::Det,
        "ART:def" | "ART:ind" => Pos::Det,
        "ADV" => Pos::Adv,
        "PRE" => Pos::Adp,
        "CON" => Pos::Cconj,
        "ONO" => Pos::Intj,
        c if c.starts_with("PRO") => Pos::Pron,
        // LIA (consonne de liaison) et lignes sans catégorie.
        _ => return None,
    })
}

fn parse_freq(s: &str) -> f32 {
    s.parse::<f32>()
        .ok()
        .filter(|f| f.is_finite() && *f >= 0.0)
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexicon::Lexicon;

    // Extrait réel de Lexique383.tsv (colonnes réduites, ordre volontairement différent).
    const TSV: &str = "lemme\tortho\tcgram\tgenre\tnombre\tfreqfilms2\tfreqlivres\tislem
ferme\tferme\tADJ\t\ts\t22.93\t39.39\t1
ferme\tferme\tNOM\tf\ts\t73.53\t47.23\t1
fermer\tferme\tVER\t\t\t79.68\t28.38\t0
été\tété\tNOM\tm\ts\t61.80\t121.55\t1
être\tété\tVER\tm\ts\t1041.22\t818.99\t0
mon\tmon\tADJ:pos\tm\ts\t4321.0\t2000.0\t1
l\tl'\tLIA\t\t\t0.00\t412.57\t1
pomme de terre\tpomme de terre\tNOM\tf\ts\t5.0\t5.0\t1
\tvide\tNOM\t\t\t1\t1\t1
";

    fn build() -> (Lexicon, ImportStats) {
        let mut b = LexiconBuilder::new();
        let stats = import(TSV.as_bytes(), &mut b).unwrap();
        (b.build().unwrap(), stats)
    }

    #[test]
    fn ranks_readings_by_frequency() {
        let (lex, _) = build();
        let ferme: Vec<_> = lex.lookup("ferme").map(|e| (e.lemma, e.pos)).collect();
        // Moyennes films/livres : nom 60,4 > verbe 54,0 > adjectif 31,2.
        assert_eq!(
            ferme,
            [
                ("ferme", Pos::Noun),
                ("fermer", Pos::Verb),
                ("ferme", Pos::Adj)
            ]
        );
        assert_eq!(lex.lookup("été").next().unwrap().lemma, "être");
    }

    #[test]
    fn maps_categories_and_skips_noise() {
        let (lex, stats) = build();
        assert_eq!(lex.lookup("mon").next().unwrap().pos, Pos::Det);
        assert_eq!(lex.lookup("l'").count(), 0); // LIA ignorée
        assert_eq!(lex.lookup("pomme de terre").count(), 0);
        assert_eq!(stats.invalid, 1);
        assert_eq!(stats.used, 6);
    }

    #[test]
    fn missing_column_is_an_error() {
        let mut b = LexiconBuilder::new();
        assert!(matches!(
            import("ortho\tlemme\n".as_bytes(), &mut b),
            Err(ImportError::Format(_))
        ));
    }
}
