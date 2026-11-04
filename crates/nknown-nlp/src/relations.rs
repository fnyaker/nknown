//! Liens de sens entre lemmes : « élection » ↔ « scrutin », « voiture » ↔ « automobile ».
//!
//! Le dictionnaire donne rarement le lien voulu tel quel : « scrutin » n'est pas un synonyme
//! de « élection » dans le Wiktionnaire. Mais les indices s'accumulent : « élection » cite
//! « scrutin » parmi ses mots apparentés, la définition de « scrutin » parle d'« élection », et
//! les deux partagent des voisins (vote, électeur, dépouillement). Chaque indice a un poids ;
//! ils se combinent comme des probabilités indépendantes (« ou » bruité : 1 − Π(1 − pᵢ)).
//!
//! 1. Indices directs : synonyme (0,9), hyperonyme ou hyponyme (0,5), mot apparenté (0,45),
//!    mot de la définition (0,55, réduit pour les sens secondaires et pour les mots qui
//!    reviennent dans beaucoup de définitions : « action », « personne »).
//! 2. Voisinage : deux mots dont les voisins directs se ressemblent (cosinus) sont liés, même
//!    sans indice direct ; un lien direct confirmé par le voisinage se renforce.
//!
//! Le calcul se fait une fois, à la construction ; la requête n'a qu'à lire la liste.
//!
//! Format de fichier (entiers en little-endian) :
//!
//! ```text
//! MAGIC (8 octets) | len_fst u64 | len_links u64 | len_strings u64
//! fst     : lemme -> position dans links
//! links   : par lemme, varint nombre, puis par lien : varint position du lemme lié dans
//!           strings, u8 poids (0..=255 pour 0..=1), par poids décroissant
//! strings : lemmes, chacun préfixé de sa longueur en varint
//! ```

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::{fs, mem};

use crate::lexicon::{LexiconError, normalize, read_varint, write_varint};

const MAGIC: &[u8; 8] = b"NKREL\0\0\x01";
const HEADER_LEN: usize = MAGIC.len() + 3 * 8;

/// Nature d'un indice direct entre deux mots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Evidence {
    /// Le premier mot cite le second parmi ses synonymes.
    Synonym,
    /// Le second mot est plus général (hyperonyme : « voiture » → « véhicule »).
    Broader,
    /// Le second mot est plus précis (hyponyme : « véhicule » → « voiture »).
    Narrower,
    /// Mot apparenté (thème commun, famille).
    Related,
    /// Le second mot apparaît dans la définition du premier.
    Gloss,
}

impl Evidence {
    fn weight(self) -> f32 {
        match self {
            Evidence::Synonym => SYNONYM,
            Evidence::Broader | Evidence::Narrower => HYPERNYM,
            Evidence::Related => RELATED,
            Evidence::Gloss => GLOSS,
        }
    }

    /// Part de l'indice dans chaque sens du lien : (premier → second, second → premier).
    ///
    /// Élargir une recherche vers un mot plus général la noie : « élection » ne doit pas
    /// ramener tous les « choix ». Or un mot de définition est souvent le genre du mot défini
    /// (« élection : choix fait par la voie des suffrages »), et un synonyme cité d'un seul
    /// côté est souvent plus précis (« choix » cite « élection », pas l'inverse). Le lien vers
    /// le mot plus précis garde tout son poids, l'autre sens en perd.
    fn directions(self) -> (f32, f32) {
        match self {
            Evidence::Synonym => (1.0, REVERSE_SYNONYM),
            Evidence::Broader | Evidence::Gloss => (GENERALISE, 1.0),
            Evidence::Narrower => (1.0, GENERALISE),
            Evidence::Related => (1.0, 1.0),
        }
    }
}

/// Part d'un synonyme cité d'un seul côté, dans le sens non cité.
const REVERSE_SYNONYM: f32 = 0.5;
/// Part d'un lien vers un mot plus général.
const GENERALISE: f32 = 0.35;

/// Force d'un indice tiré d'une liste du dictionnaire : un mot parmi trente synonymes
/// (« voiture » : bagnole, caisse, veau, cage…) est un indice plus faible qu'un mot parmi
/// trois ; un sens secondaire (`rank` ≥ 1) compte un peu moins que le premier.
pub fn list_strength(len: usize, rank: usize) -> f32 {
    let dilution = (LIST_LEN as f32 / len.max(1) as f32).sqrt().min(1.0);
    dilution / (1.0 + LIST_RANK_DECAY * rank as f32)
}

/// Force d'un mot de définition selon le rang du sens (0 = le premier, le plus courant).
pub fn gloss_strength(rank: usize) -> f32 {
    1.0 / (1.0 + GLOSS_RANK_DECAY * rank as f32)
}

/// Longueur de liste au-delà de laquelle chaque élément compte moins.
const LIST_LEN: usize = 6;
const LIST_RANK_DECAY: f32 = 0.25;

const SYNONYM: f32 = 0.9;
const HYPERNYM: f32 = 0.5;
const RELATED: f32 = 0.45;
const GLOSS: f32 = 0.55;
/// Baisse du poids d'une définition par rang de sens.
const GLOSS_RANK_DECAY: f32 = 0.4;
/// Poids du voisinage partagé (cosinus) dans le lien final.
const NEIGHBOURHOOD: f32 = 0.8;
/// Voisins directs suivis pour trouver les liens de second ordre, et poids minimal.
const FANOUT: usize = 8;
const FANOUT_MIN: f32 = 0.2;

/// Réglages de la construction.
#[derive(Debug, Clone, Copy)]
pub struct BuildOptions {
    /// Liens gardés par lemme, au plus.
    pub per_lemma: usize,
    /// Poids minimal d'un lien gardé.
    pub min_weight: f32,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            per_lemma: 16,
            min_weight: 0.25,
        }
    }
}

/// Accumule les indices puis calcule les liens.
#[derive(Debug, Default)]
pub struct RelationsBuilder {
    ids: HashMap<String, u32>,
    names: Vec<String>,
    /// (de, vers, nature, force 0..=255).
    evidence: Vec<(u32, u32, Evidence, u8)>,
}

impl RelationsBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    fn id(&mut self, lemma: &str) -> u32 {
        let key = normalize(lemma);
        if let Some(&id) = self.ids.get(&key) {
            return id;
        }
        let id = u32::try_from(self.names.len()).expect("plus de 4 milliards de lemmes");
        self.names.push(key.clone());
        self.ids.insert(key, id);
        id
    }

    /// Indice de `from` vers `to`, de force `strength` (0..=1, voir [`list_strength`] et
    /// [`gloss_strength`]). Le lien vaut dans les deux sens, avec la part que donne la nature
    /// de l'indice.
    pub fn link(&mut self, from: &str, to: &str, evidence: Evidence, strength: f32) {
        let (a, b) = (self.id(from), self.id(to));
        let q = (strength.clamp(0.0, 1.0) * 255.0).round() as u8;
        if a != b && q > 0 {
            self.evidence.push((a, b, evidence, q));
        }
    }

    pub fn evidence_count(&self) -> usize {
        self.evidence.len()
    }

    /// Calcule les liens. `familiarity` (0..=1) dit si un mot est courant : à poids voisin,
    /// les mots courants passent devant les variantes rares (« toubibe », « chomdu »), qu'aucun
    /// texte ne contiendra.
    pub fn build(
        mut self,
        opts: BuildOptions,
        familiarity: impl Fn(&str) -> f32,
    ) -> Result<Relations, LexiconError> {
        let n = self.names.len();
        // Un même indice déclaré deux fois (deux entrées du même mot) ne compte qu'une fois,
        // à sa plus grande force.
        self.evidence
            .sort_unstable_by(|x, y| (x.0, x.1, x.2, y.3).cmp(&(y.0, y.1, y.2, x.3)));
        self.evidence
            .dedup_by(|x, y| x.0 == y.0 && x.1 == y.1 && x.2 == y.2);
        // Spécificité des mots de définition : un mot présent dans beaucoup de définitions
        // (« action », « personne ») en dit peu.
        let mut gloss_df = vec![0u32; n];
        let mut glossed = 0usize;
        let mut last = u32::MAX;
        for &(a, b, e, _) in &self.evidence {
            if e == Evidence::Gloss {
                gloss_df[b as usize] += 1;
                if a != last {
                    glossed += 1;
                    last = a;
                }
            }
        }
        let docs = glossed as f32 + 1.0;
        let spec = |b: u32| {
            ((docs / (gloss_df[b as usize] as f32).max(1.0)).ln() / docs.ln()).clamp(0.0, 1.0)
        };

        // Indices directs, dans les deux sens, chacun avec sa part.
        let mut adj: Vec<Vec<(u32, f32)>> = vec![Vec::new(); n];
        for &(a, b, e, q) in &self.evidence {
            let mut p = e.weight() * f32::from(q) / 255.0;
            if e == Evidence::Gloss {
                p *= spec(b);
            }
            let (forward, backward) = e.directions();
            if p > 0.0 {
                adj[a as usize].push((b, p * forward));
                adj[b as usize].push((a, p * backward));
            }
        }
        drop(mem::take(&mut self.evidence));
        // « Ou » bruité des indices de chaque paire ; vecteurs triés par voisin.
        let direct: Vec<Vec<(u32, f32)>> = adj
            .into_iter()
            .map(|mut v| {
                v.sort_unstable_by_key(|&(b, _)| b);
                let mut out: Vec<(u32, f32)> = Vec::with_capacity(v.len());
                for (b, p) in v {
                    match out.last_mut() {
                        Some((last, q)) if *last == b => *q = 1.0 - (1.0 - *q) * (1.0 - p),
                        _ => out.push((b, p)),
                    }
                }
                out
            })
            .collect();
        let norms: Vec<f32> = direct
            .iter()
            .map(|v| v.iter().map(|(_, w)| w * w).sum::<f32>().sqrt())
            .collect();
        let strongest = |a: usize| {
            let mut top: Vec<(u32, f32)> = direct[a]
                .iter()
                .copied()
                .filter(|&(_, w)| w >= FANOUT_MIN)
                .collect();
            top.sort_unstable_by(|x, y| y.1.total_cmp(&x.1));
            top.truncate(FANOUT);
            top
        };

        let familiar: Vec<f32> = self
            .names
            .iter()
            .map(|w| familiarity(w).clamp(0.0, 1.0))
            .collect();
        let mut lists: BTreeMap<String, Vec<(u32, f32)>> = BTreeMap::new();
        let mut candidates: Vec<u32> = Vec::new();
        for a in 0..n {
            if direct[a].is_empty() {
                continue;
            }
            candidates.clear();
            candidates.extend(direct[a].iter().map(|&(b, _)| b));
            for (b, _) in strongest(a) {
                candidates.extend(
                    strongest(b as usize)
                        .into_iter()
                        .map(|(c, _)| c)
                        .filter(|&c| c as usize != a),
                );
            }
            candidates.sort_unstable();
            candidates.dedup();
            let mut links: Vec<(u32, f32)> = candidates
                .iter()
                .map(|&c| {
                    let base = direct[a]
                        .binary_search_by_key(&c, |&(b, _)| b)
                        .map_or(0.0, |i| direct[a][i].1);
                    let denom = norms[a] * norms[c as usize];
                    let cos = if denom > 0.0 {
                        dot(&direct[a], &direct[c as usize]) / denom
                    } else {
                        0.0
                    };
                    (c, 1.0 - (1.0 - base) * (1.0 - NEIGHBOURHOOD * cos))
                })
                .filter(|&(_, w)| w >= opts.min_weight)
                .collect();
            let rank = |&(c, w): &(u32, f32)| w * (0.5 + 0.5 * familiar[c as usize]);
            links.sort_unstable_by(|x, y| rank(y).total_cmp(&rank(x)).then(x.0.cmp(&y.0)));
            links.truncate(opts.per_lemma);
            links.sort_by(|x, y| y.1.total_cmp(&x.1));
            if !links.is_empty() {
                lists.insert(self.names[a].clone(), links);
            }
        }

        // Écriture : seuls les lemmes cités sont gardés dans la table des chaînes.
        let mut strings = Vec::new();
        let mut offsets: HashMap<u32, u64> = HashMap::new();
        let mut links = Vec::new();
        let mut fst = fst::MapBuilder::memory();
        for (lemma, list) in &lists {
            fst.insert(lemma, links.len() as u64)?;
            write_varint(&mut links, list.len() as u64);
            for &(c, w) in list {
                let off = *offsets.entry(c).or_insert_with(|| {
                    let off = strings.len() as u64;
                    let name = &self.names[c as usize];
                    write_varint(&mut strings, name.len() as u64);
                    strings.extend_from_slice(name.as_bytes());
                    off
                });
                write_varint(&mut links, off);
                links.push((w.clamp(0.0, 1.0) * 255.0).round() as u8);
            }
        }
        Ok(Relations {
            fst: fst.into_map(),
            links,
            strings,
        })
    }
}

/// Produit scalaire de deux vecteurs creux triés par indice.
fn dot(x: &[(u32, f32)], y: &[(u32, f32)]) -> f32 {
    let (mut i, mut j, mut s) = (0, 0, 0.0);
    while i < x.len() && j < y.len() {
        match x[i].0.cmp(&y[j].0) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                s += x[i].1 * y[j].1;
                i += 1;
                j += 1;
            }
        }
    }
    s
}

/// Liens compilés, en lecture seule.
pub struct Relations {
    fst: fst::Map<Vec<u8>>,
    links: Vec<u8>,
    strings: Vec<u8>,
}

impl std::fmt::Debug for Relations {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Relations")
            .field("lemmas", &self.lemma_count())
            .finish()
    }
}

impl Relations {
    pub fn lemma_count(&self) -> usize {
        self.fst.len()
    }

    /// Lemmes liés à `lemma`, du plus proche au plus lointain, avec le poids du lien (0..=1).
    pub fn related<'a>(&'a self, lemma: &str) -> impl Iterator<Item = (&'a str, f32)> + use<'a> {
        let mut rest: &[u8] = match self.fst.get(normalize(lemma)) {
            Some(off) => usize::try_from(off)
                .ok()
                .and_then(|o| self.links.get(o..))
                .unwrap_or(&[]),
            None => &[],
        };
        let count = read_varint(&mut rest).unwrap_or(0);
        (0..count).map_while(move |_| {
            let off = usize::try_from(read_varint(&mut rest)?).ok()?;
            let (&w, tail) = rest.split_first()?;
            rest = tail;
            let mut s = self.strings.get(off..)?;
            let len = usize::try_from(read_varint(&mut s)?).ok()?;
            let name = std::str::from_utf8(s.get(..len)?).ok()?;
            Some((name, f32::from(w) / 255.0))
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let fst = self.fst.as_fst().as_bytes();
        let mut out =
            Vec::with_capacity(HEADER_LEN + fst.len() + self.links.len() + self.strings.len());
        out.extend_from_slice(MAGIC);
        for len in [fst.len(), self.links.len(), self.strings.len()] {
            out.extend_from_slice(&(len as u64).to_le_bytes());
        }
        out.extend_from_slice(fst);
        out.extend_from_slice(&self.links);
        out.extend_from_slice(&self.strings);
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, LexiconError> {
        if bytes.len() < HEADER_LEN || &bytes[..MAGIC.len()] != MAGIC {
            return Err(LexiconError::Format("en-tête absent ou version inconnue"));
        }
        let mut lens = [0usize; 3];
        for (i, len) in lens.iter_mut().enumerate() {
            let at = MAGIC.len() + i * 8;
            let raw = u64::from_le_bytes(bytes[at..at + 8].try_into().expect("8 octets"));
            *len = usize::try_from(raw).map_err(|_| LexiconError::Format("taille de section"))?;
        }
        let mut rest = &bytes[HEADER_LEN..];
        let mut take = |len: usize| -> Result<Vec<u8>, LexiconError> {
            if rest.len() < len {
                return Err(LexiconError::Format("fichier tronqué"));
            }
            let (head, tail) = rest.split_at(len);
            rest = tail;
            Ok(head.to_vec())
        };
        let fst = fst::Map::new(take(lens[0])?)?;
        let links = take(lens[1])?;
        let strings = take(lens[2])?;
        if !rest.is_empty() {
            return Err(LexiconError::Format("octets en trop"));
        }
        Ok(Self {
            fst,
            links,
            strings,
        })
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), LexiconError> {
        fs::write(path, self.to_bytes())?;
        Ok(())
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, LexiconError> {
        Self::from_bytes(&fs::read(path)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn weight(r: &Relations, a: &str, b: &str) -> f32 {
        r.related(a).find(|(x, _)| *x == b).map_or(0.0, |(_, w)| w)
    }

    /// Le cas « élection » / « scrutin » : aucun synonyme direct, mais un mot apparenté, une
    /// définition et des voisins communs.
    fn election() -> Relations {
        let mut b = RelationsBuilder::new();
        b.link("élection", "scrutin", Evidence::Related, 1.0);
        b.link("élection", "vote", Evidence::Related, 1.0);
        b.link("élection", "électeur", Evidence::Related, 1.0);
        b.link("élection", "suffrage", Evidence::Gloss, 1.0);
        b.link("scrutin", "élection", Evidence::Gloss, gloss_strength(1));
        b.link("scrutin", "vote", Evidence::Gloss, 1.0);
        b.link("scrutin", "électeur", Evidence::Related, 1.0);
        b.link("scrutin", "votation", Evidence::Synonym, 1.0);
        b.link("voiture", "automobile", Evidence::Synonym, 1.0);
        // Mots qui reviennent dans toutes les définitions : peu spécifiques.
        for w in [
            "voiture",
            "automobile",
            "scrutin",
            "élection",
            "vote",
            "suffrage",
        ] {
            b.link(w, "chose", Evidence::Gloss, 1.0);
        }
        b.build(
            BuildOptions {
                per_lemma: 12,
                min_weight: 0.05,
            },
            |_| 1.0,
        )
        .unwrap()
    }

    #[test]
    fn indirect_evidence_adds_up() {
        let r = election();
        let direct_only = 1.0 - (1.0 - RELATED);
        let w = weight(&r, "élection", "scrutin");
        assert!(w > direct_only, "{w}");
        // « scrutin » se définit par « élection » : il est plus précis. Le lien vers lui
        // est plus fort que le lien inverse, vers le genre.
        let back = weight(&r, "scrutin", "élection");
        assert!(back > 0.0 && back < w, "{back} {w}");
        // Synonyme du dictionnaire : lien fort.
        assert!(weight(&r, "voiture", "automobile") >= 0.85);
    }

    #[test]
    fn second_order_links_through_shared_neighbours() {
        let r = election();
        // « suffrage » et « scrutin » ne sont jamais cités ensemble, mais partagent
        // « élection » (et « chose », trop banal pour compter).
        assert!(weight(&r, "suffrage", "scrutin") > 0.0);
        assert!(weight(&r, "suffrage", "scrutin") < weight(&r, "élection", "scrutin"));
    }

    #[test]
    fn frequent_gloss_words_weigh_little() {
        let r = election();
        assert!(weight(&r, "chose", "voiture") < weight(&r, "suffrage", "élection"));
    }

    #[test]
    fn links_towards_broader_words_are_weaker() {
        let mut b = RelationsBuilder::new();
        // « choix » cite « élection » parmi ses synonymes ; l'inverse n'est pas vrai.
        b.link("choix", "élection", Evidence::Synonym, 1.0);
        b.link("véhicule", "voiture", Evidence::Narrower, 1.0);
        let r = b.build(BuildOptions::default(), |_| 1.0).unwrap();
        assert!(weight(&r, "élection", "choix") < weight(&r, "choix", "élection"));
        assert!(weight(&r, "voiture", "véhicule") < weight(&r, "véhicule", "voiture"));
    }

    #[test]
    fn sorted_by_weight_and_roundtrip() {
        let r = election();
        let list: Vec<_> = r.related("Élection").collect();
        assert!(list.windows(2).all(|w| w[0].1 >= w[1].1), "{list:?}");
        let back = Relations::from_bytes(&r.to_bytes()).unwrap();
        assert_eq!(back.related("élection").collect::<Vec<_>>(), list);
        assert_eq!(back.related("inconnu").count(), 0);
    }
}
