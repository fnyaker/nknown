//! Rattachement des adjectifs au nom qu'ils qualifient, par position et accord.
//!
//! Pour chaque adjectif, on cherche le nom le plus proche à gauche et à droite en sautant
//! adverbes et autres adjectifs (« les rues calmes et vides »), puis on garde le premier
//! candidat qui s'accorde en genre et en nombre, dans l'ordre préféré par la langue.
//! À défaut, un adjectif attribut (« Paris est calme ») est rattaché au sujet du verbe copule.

use nknown_core::analyze::{Feats, Pos, Relation};

use nknown_core::lang::Lang;

/// Ce que le rattachement sait d'un mot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    pub lemma: String,
    pub pos: Pos,
    pub feats: Feats,
}

#[derive(Debug, Clone, Copy)]
pub struct AttachRules {
    /// L'adjectif suit le plus souvent le nom (français) ou le précède (anglais).
    pub prefer_left: bool,
    /// Lemmes des verbes attributifs.
    pub copulas: &'static [&'static str],
}

impl AttachRules {
    pub const FRENCH: AttachRules = AttachRules {
        prefer_left: true,
        copulas: &[
            "être",
            "devenir",
            "sembler",
            "paraître",
            "rester",
            "demeurer",
        ],
    };

    pub fn for_lang(lang: Lang) -> Option<AttachRules> {
        (lang == Lang::FRA).then_some(Self::FRENCH)
    }
}

/// Relations `Qualifies` de la phrase `words` (indices relatifs à la phrase).
pub fn attach(words: &[Word], rules: &AttachRules) -> Vec<Relation> {
    let mut out = Vec::new();
    for (i, w) in words.iter().enumerate() {
        if w.pos != Pos::Adj {
            continue;
        }
        let agrees = |n: usize| agree(w.feats, words[n].feats);
        let (left, right) = (scan_left(words, i), scan_right(words, i));
        let order = if rules.prefer_left {
            [left, right]
        } else {
            [right, left]
        };
        let noun = order
            .into_iter()
            .flatten()
            .find(|&n| agrees(n))
            .or_else(|| predicative_subject(words, i, rules).filter(|&n| agrees(n)));
        if let Some(noun) = noun {
            out.push(Relation::Qualifies { adj: i, noun });
        }
    }
    out
}

fn is_noun(p: Pos) -> bool {
    matches!(p, Pos::Noun | Pos::Propn)
}

/// Nom à gauche, en sautant adverbes, adjectifs, et coordinations entre adjectifs.
fn scan_left(words: &[Word], i: usize) -> Option<usize> {
    let mut j = i;
    while j > 0 {
        j -= 1;
        match words[j].pos {
            p if is_noun(p) => return Some(j),
            Pos::Adv | Pos::Adj => {}
            // « calmes et vides », « calmes, vides » : seulement entre deux adjectifs.
            Pos::Cconj | Pos::Punct if j > 0 && words[j - 1].pos == Pos::Adj => {}
            _ => return None,
        }
    }
    None
}

/// Nom à droite (adjectif antéposé : « un grand homme »).
fn scan_right(words: &[Word], i: usize) -> Option<usize> {
    for (j, w) in words.iter().enumerate().skip(i + 1) {
        match w.pos {
            p if is_noun(p) => return Some(j),
            Pos::Adv | Pos::Adj => {}
            _ => return None,
        }
    }
    None
}

/// « Paris est calme » : adjectif → copule → sujet nominal.
fn predicative_subject(words: &[Word], i: usize, rules: &AttachRules) -> Option<usize> {
    let mut j = i;
    let verb = loop {
        j = j.checked_sub(1)?;
        match words[j].pos {
            Pos::Adv => {}
            Pos::Verb | Pos::Aux if rules.copulas.contains(&words[j].lemma.as_str()) => break j,
            _ => return None,
        }
    };
    // Sujet : en sautant la négation et les clitiques (« Paris ne lui semble pas »).
    for k in (0..verb).rev() {
        match words[k].pos {
            p if is_noun(p) => return Some(k),
            Pos::Adv | Pos::Pron | Pos::Part | Pos::Aux => {}
            _ => return None,
        }
    }
    None
}

/// Genre et nombre compatibles (un trait inconnu est compatible avec tout).
fn agree(a: Feats, b: Feats) -> bool {
    fn ok<T: PartialEq>(x: Option<T>, y: Option<T>) -> bool {
        x.is_none() || y.is_none() || x == y
    }
    ok(a.gender, b.gender) && ok(a.number, b.number)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nknown_core::analyze::{Gender::*, Number::*};

    fn w(
        lemma: &str,
        pos: Pos,
        g: Option<nknown_core::analyze::Gender>,
        n: Option<nknown_core::analyze::Number>,
    ) -> Word {
        Word {
            lemma: lemma.into(),
            pos,
            feats: Feats {
                gender: g,
                number: n,
            },
        }
    }

    fn pairs(words: &[Word]) -> Vec<(usize, usize)> {
        attach(words, &AttachRules::FRENCH)
            .into_iter()
            .map(|Relation::Qualifies { adj, noun }| (adj, noun))
            .collect()
    }

    #[test]
    fn postnominal_with_coordination() {
        // les rues calmes et vides
        let s = [
            w("le", Pos::Det, None, Some(Plur)),
            w("rue", Pos::Noun, Some(Fem), Some(Plur)),
            w("calme", Pos::Adj, None, Some(Plur)),
            w("et", Pos::Cconj, None, None),
            w("vide", Pos::Adj, None, Some(Plur)),
        ];
        assert_eq!(pairs(&s), [(2, 1), (4, 1)]);
    }

    #[test]
    fn agreement_picks_the_right_noun() {
        // le président de la République française / un taux de croissance élevé
        let s = [
            w("président", Pos::Noun, Some(Masc), Some(Sing)),
            w("de", Pos::Adp, None, None),
            w("république", Pos::Noun, Some(Fem), Some(Sing)),
            w("français", Pos::Adj, Some(Fem), Some(Sing)),
        ];
        assert_eq!(pairs(&s), [(3, 2)]);
        // Désaccord avec le nom à gauche : on prend le nom à droite.
        let s = [
            w("rue", Pos::Noun, Some(Fem), Some(Plur)),
            w("grand", Pos::Adj, Some(Masc), Some(Sing)),
            w("homme", Pos::Noun, Some(Masc), Some(Sing)),
        ];
        assert_eq!(pairs(&s), [(1, 2)]);
    }

    #[test]
    fn prenominal_and_predicative() {
        let s = [
            w("un", Pos::Det, None, None),
            w("grand", Pos::Adj, Some(Masc), Some(Sing)),
            w("homme", Pos::Noun, Some(Masc), Some(Sing)),
        ];
        assert_eq!(pairs(&s), [(1, 2)]);
        // Paris n'est pas calme
        let s = [
            w("Paris", Pos::Propn, None, None),
            w("ne", Pos::Adv, None, None),
            w("être", Pos::Aux, None, None),
            w("pas", Pos::Adv, None, None),
            w("calme", Pos::Adj, None, Some(Sing)),
        ];
        assert_eq!(pairs(&s), [(4, 0)]);
        // il est calme : sujet pronominal, pas de rattachement
        let s = [
            w("il", Pos::Pron, None, None),
            w("être", Pos::Aux, None, None),
            w("calme", Pos::Adj, None, Some(Sing)),
        ];
        assert!(pairs(&s).is_empty());
    }
}
