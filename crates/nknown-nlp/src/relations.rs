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
//! **Sens.** Un mot a souvent plusieurs sens (« portable » : téléphone, ordinateur ; « livre » :
//! ouvrage, monnaie). Chaque sens garde sa signature (mots de sa définition, ses propres
//! synonymes) et chaque lien est rattaché au sens dont il vient. Quand le dictionnaire ne le
//! dit pas (synonyme cité pour toute l'entrée, lien de voisinage), le lien va au sens dont la
//! signature recoupe le mieux les voisins du mot lié. À la requête, les autres mots choisissent
//! le sens ([`Record::in_context`]) : dans « ordinateur portable », la définition « Ordinateur
//! portable » l'emporte et « gsm » (sens téléphone) s'efface.
//!
//! **Antonymes.** Gardés à part, pour écarter des liens appris ailleurs (« hausse » et
//! « baisse » s'emploient dans les mêmes phrases sans être synonymes).
//!
//! Le calcul se fait une fois, à la construction ; la requête n'a qu'à lire la fiche du mot.
//!
//! Format de fichier (entiers en little-endian) :
//!
//! ```text
//! MAGIC (8 octets) | len_fst u64 | len_records u64 | len_strings u64
//! fst     : lemme -> position de sa fiche dans records
//! fiche   : varint nombre de sens (0 si un seul) ; par sens : varint entrée (groupe de sens
//!           d'une même entrée du dictionnaire), varint nombre de mots de signature, leurs
//!           positions dans strings ;
//!           varint nombre de liens ; par lien : varint position du lemme lié dans strings,
//!           u8 poids (0..=255 pour 0..=1), u8 rattachement (sens 0..=127, entrée 128 + n,
//!           255 : tous les sens), par poids décroissant ;
//!           varint nombre d'antonymes, leurs positions dans strings
//! strings : lemmes, chacun préfixé de sa longueur en varint
//! ```

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::{fs, mem};

use crate::lexicon::{LexiconError, normalize, read_varint, write_varint};

const MAGIC: &[u8; 8] = b"NKREL\0\0\x02";
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

/// D'où vient un indice, côté du premier mot : un sens précis, une entrée du dictionnaire
/// (tous ses sens), ou rien de connu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Origin {
    None,
    Entry(EntryId),
    Sense(SenseId),
}

/// Entrée du dictionnaire (un mot a une entrée par catégorie ou par étymologie).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EntryId(u32);

/// Sens d'une entrée.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SenseId(u32);

const NONE: u32 = u32::MAX;

impl Origin {
    fn encode(self) -> (u32, u32) {
        match self {
            Origin::None => (NONE, NONE),
            Origin::Entry(e) => (e.0, NONE),
            Origin::Sense(s) => (NONE, s.0),
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

/// Sens gardés par mot, mots de signature par sens, antonymes par mot.
const MAX_SENSES: usize = 32;
const MAX_SIGNATURE: usize = 16;
const MAX_ANTONYMS: usize = 16;
/// Spécificité minimale d'un mot de définition dans une signature (« être », « faire » : non).
const SIGNATURE_SPEC: f32 = 0.3;
/// Part d'un sens qui doit dominer les indices pour qu'un lien lui soit rattaché d'office.
const DOMINANT: f32 = 0.6;
/// Un sens l'emporte sur un autre par affinité s'il le dépasse de ce facteur.
const CLEAR_MARGIN: f32 = 1.5;

/// Rattachement d'un lien : à tous les sens du mot.
pub const SHARED: u8 = 255;
const GROUP_BASE: u8 = 128;

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

/// Lien calculé : (lemme lié, poids, rattachement).
type TaggedLink = (u32, f32, u8);

#[derive(Debug)]
struct SenseData {
    entry: u32,
    signature: Vec<u32>,
}

/// Accumule les indices puis calcule les liens.
#[derive(Debug, Default)]
pub struct RelationsBuilder {
    ids: HashMap<String, u32>,
    names: Vec<String>,
    /// Mot de chaque entrée.
    entries: Vec<u32>,
    senses: Vec<SenseData>,
    /// (de, vers, nature, force 0..=255, entrée, sens) ; entrée et sens côté `de`.
    evidence: Vec<(u32, u32, Evidence, u8, u32, u32)>,
    antonyms: Vec<(u32, u32)>,
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

    /// Nouvelle entrée du dictionnaire pour `word`.
    pub fn entry(&mut self, word: &str) -> EntryId {
        let w = self.id(word);
        self.entries.push(w);
        EntryId(u32::try_from(self.entries.len() - 1).expect("trop d'entrées"))
    }

    /// Nouveau sens de l'entrée `entry`, dans l'ordre du dictionnaire.
    pub fn sense(&mut self, entry: EntryId) -> SenseId {
        self.senses.push(SenseData {
            entry: entry.0,
            signature: Vec::new(),
        });
        SenseId(u32::try_from(self.senses.len() - 1).expect("trop de sens"))
    }

    /// Ajoute `lemma` à la signature du sens (mot de sa définition, synonyme propre).
    pub fn sign(&mut self, sense: SenseId, lemma: &str) {
        let id = self.id(lemma);
        let sig = &mut self.senses[sense.0 as usize].signature;
        if sig.len() < MAX_SIGNATURE * 2 && !sig.contains(&id) {
            sig.push(id);
        }
    }

    /// Indice de `from` vers `to`, de force `strength` (0..=1, voir [`list_strength`] et
    /// [`gloss_strength`]). Le lien vaut dans les deux sens, avec la part que donne la nature
    /// de l'indice.
    pub fn link(&mut self, from: &str, to: &str, evidence: Evidence, strength: f32) {
        self.link_from(from, to, evidence, strength, Origin::None);
    }

    /// Comme [`Self::link`], en disant de quel sens de `from` vient l'indice.
    pub fn link_from(
        &mut self,
        from: &str,
        to: &str,
        evidence: Evidence,
        strength: f32,
        origin: Origin,
    ) {
        let (a, b) = (self.id(from), self.id(to));
        let q = (strength.clamp(0.0, 1.0) * 255.0).round() as u8;
        let (entry, sense) = origin.encode();
        if a != b && q > 0 {
            self.evidence.push((a, b, evidence, q, entry, sense));
        }
    }

    /// `a` et `b` sont de sens contraires.
    pub fn antonym(&mut self, a: &str, b: &str) {
        let (a, b) = (self.id(a), self.id(b));
        if a != b {
            self.antonyms.push((a, b));
            self.antonyms.push((b, a));
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
        for &(a, b, e, ..) in &self.evidence {
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

        // Indices directs, dans les deux sens, chacun avec sa part ; l'origine (sens, entrée)
        // n'est connue que du côté du mot qui cite.
        let mut adj: Vec<Vec<(u32, f32, u32, u32)>> = vec![Vec::new(); n];
        for &(a, b, e, q, entry, sense) in &self.evidence {
            let mut p = e.weight() * f32::from(q) / 255.0;
            if e == Evidence::Gloss {
                p *= spec(b);
            }
            let (forward, backward) = e.directions();
            if p > 0.0 {
                adj[a as usize].push((b, p * forward, entry, sense));
                adj[b as usize].push((a, p * backward, NONE, NONE));
            }
        }
        drop(mem::take(&mut self.evidence));
        // « Ou » bruité des indices de chaque paire ; vecteurs triés par voisin. À part, la
        // masse d'indices venue de chaque sens ou entrée connus.
        let mut origins: Vec<Vec<(u32, u32, u32, f32)>> = vec![Vec::new(); n];
        let direct: Vec<Vec<(u32, f32)>> = adj
            .into_iter()
            .enumerate()
            .map(|(a, mut v)| {
                v.sort_unstable_by_key(|&(b, ..)| b);
                let mut out: Vec<(u32, f32)> = Vec::with_capacity(v.len());
                for (b, p, entry, sense) in v {
                    match out.last_mut() {
                        Some((last, q)) if *last == b => *q = 1.0 - (1.0 - *q) * (1.0 - p),
                        _ => out.push((b, p)),
                    }
                    if entry != NONE || sense != NONE {
                        origins[a].push((b, entry, sense, p));
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

        // Sens de chaque mot, dans l'ordre ; signatures triées pour les intersections.
        let mut word_senses: Vec<Vec<u32>> = vec![Vec::new(); n];
        for (s, data) in self.senses.iter_mut().enumerate() {
            let w = self.entries[data.entry as usize] as usize;
            if word_senses[w].len() < MAX_SENSES {
                word_senses[w].push(s as u32);
            }
            data.signature.sort_unstable();
            data.signature.dedup();
        }

        let familiar: Vec<f32> = self
            .names
            .iter()
            .map(|w| familiarity(w).clamp(0.0, 1.0))
            .collect();
        let mut lists: BTreeMap<String, (u32, Vec<TaggedLink>)> = BTreeMap::new();
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
            if links.is_empty() {
                continue;
            }
            let senses = &word_senses[a];
            let tagged = links
                .into_iter()
                .map(|(c, w)| {
                    let tag = if senses.len() < 2 {
                        SHARED
                    } else {
                        self.attach(senses, &origins[a], c, &direct[c as usize])
                    };
                    (c, w, tag)
                })
                .collect();
            lists.insert(self.names[a].clone(), (a as u32, tagged));
        }
        // Mots qui n'ont que des antonymes.
        self.antonyms.sort_unstable();
        self.antonyms.dedup();
        for &(a, _) in &self.antonyms {
            lists
                .entry(self.names[a as usize].clone())
                .or_insert_with(|| (a, Vec::new()));
        }

        // Écriture : seuls les lemmes cités sont gardés dans la table des chaînes.
        let mut strings = Vec::new();
        let mut offsets: HashMap<u32, u64> = HashMap::new();
        let mut offset_of = |c: u32, strings: &mut Vec<u8>| -> u64 {
            *offsets.entry(c).or_insert_with(|| {
                let off = strings.len() as u64;
                let name = &self.names[c as usize];
                write_varint(strings, name.len() as u64);
                strings.extend_from_slice(name.as_bytes());
                off
            })
        };
        let mut records = Vec::new();
        let mut fst = fst::MapBuilder::memory();
        for (lemma, (a, list)) in &lists {
            fst.insert(lemma, records.len() as u64)?;
            let senses = &word_senses[*a as usize];
            let used = senses.len() >= 2 && list.iter().any(|&(_, _, t)| t != SHARED);
            if used {
                let groups = groups_of(&self.senses, senses);
                write_varint(&mut records, senses.len() as u64);
                for (i, &s) in senses.iter().enumerate() {
                    write_varint(&mut records, u64::from(groups[i]));
                    let sig = &self.senses[s as usize].signature;
                    // Les mots de signature les plus spécifiques d'abord.
                    let mut sig: Vec<u32> = sig
                        .iter()
                        .copied()
                        .filter(|&x| gloss_df[x as usize] == 0 || spec(x) >= SIGNATURE_SPEC)
                        .collect();
                    sig.sort_by(|&x, &y| spec(y).total_cmp(&spec(x)).then(x.cmp(&y)));
                    sig.truncate(MAX_SIGNATURE);
                    write_varint(&mut records, sig.len() as u64);
                    for c in sig {
                        let off = offset_of(c, &mut strings);
                        write_varint(&mut records, off);
                    }
                }
            } else {
                write_varint(&mut records, 0);
            }
            write_varint(&mut records, list.len() as u64);
            for &(c, w, tag) in list {
                let off = offset_of(c, &mut strings);
                write_varint(&mut records, off);
                records.push((w.clamp(0.0, 1.0) * 255.0).round() as u8);
                records.push(if used { tag } else { SHARED });
            }
            let start = self.antonyms.partition_point(|&(x, _)| x < *a);
            let ants: Vec<u32> = self.antonyms[start..]
                .iter()
                .take_while(|&&(x, _)| x == *a)
                .map(|&(_, b)| b)
                .take(MAX_ANTONYMS)
                .collect();
            write_varint(&mut records, ants.len() as u64);
            for b in ants {
                let off = offset_of(b, &mut strings);
                write_varint(&mut records, off);
            }
        }
        Ok(Relations {
            fst: fst.into_map(),
            records,
            strings,
        })
    }

    /// Sens de `senses` (ceux du mot) auquel rattacher le lien vers `c`.
    ///
    /// Si les indices viennent surtout d'un sens, c'est lui. Sinon, parmi les sens des entrées
    /// d'où viennent les indices (tous, si aucune n'est connue), celui dont la signature
    /// contient `c` ou recoupe le mieux ses voisins (`neighbours`), s'il se détache ; à défaut,
    /// l'entrée seule, ou tous les sens.
    fn attach(
        &self,
        senses: &[u32],
        origins: &[(u32, u32, u32, f32)],
        c: u32,
        neighbours: &[(u32, f32)],
    ) -> u8 {
        let start = origins.partition_point(|o| o.0 < c);
        let from_c = origins[start..].iter().take_while(|o| o.0 == c);
        let mut by_sense: HashMap<u32, f32> = HashMap::new();
        let mut entries: Vec<u32> = Vec::new();
        let mut total = 0.0;
        for &(_, entry, sense, p) in from_c {
            total += p;
            if sense != NONE {
                *by_sense.entry(sense).or_default() += p;
                entries.push(self.senses[sense as usize].entry);
            } else if entry != NONE {
                entries.push(entry);
            }
        }
        let local = |s: u32| senses.iter().position(|&x| x == s);
        if let Some((&s, &mass)) = by_sense.iter().max_by(|x, y| x.1.total_cmp(y.1))
            && mass >= DOMINANT * total
            && let Some(i) = local(s)
        {
            return i as u8;
        }
        entries.sort_unstable();
        entries.dedup();
        let pool: Vec<(usize, u32)> = senses
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                entries.is_empty() || entries.contains(&self.senses[**s as usize].entry)
            })
            .map(|(i, &s)| (i, s))
            .collect();
        let affinity = |s: u32| {
            let sig = &self.senses[s as usize].signature;
            let own = if sig.binary_search(&c).is_ok() {
                1.0
            } else {
                0.0
            };
            let mut overlap = 0.0;
            for &(x, w) in neighbours {
                if sig.binary_search(&x).is_ok() {
                    overlap += w;
                }
            }
            own + by_sense.get(&s).copied().unwrap_or(0.0)
                + overlap / (sig.len().max(1) as f32).sqrt()
        };
        let mut scored: Vec<(usize, f32)> = pool.iter().map(|&(i, s)| (i, affinity(s))).collect();
        scored.sort_by(|x, y| y.1.total_cmp(&x.1));
        match scored.as_slice() {
            [(i, best), rest @ ..]
                if *best > 0.0 && rest.first().is_none_or(|(_, b)| *best >= CLEAR_MARGIN * b) =>
            {
                *i as u8
            }
            _ if entries.len() == 1 => {
                let groups = groups_of(&self.senses, senses);
                let first = senses
                    .iter()
                    .position(|&s| self.senses[s as usize].entry == entries[0])
                    .unwrap_or(0);
                GROUP_BASE + groups[first].min(126) as u8
            }
            _ => SHARED,
        }
    }
}

/// Numéro d'entrée local (0, 1…) de chaque sens d'un mot.
fn groups_of(all: &[SenseData], senses: &[u32]) -> Vec<u32> {
    let mut seen: Vec<u32> = Vec::new();
    senses
        .iter()
        .map(|&s| {
            let e = all[s as usize].entry;
            match seen.iter().position(|&x| x == e) {
                Some(i) => i as u32,
                None => {
                    seen.push(e);
                    (seen.len() - 1) as u32
                }
            }
        })
        .collect()
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

/// Lien d'une fiche.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Link<'a> {
    pub word: &'a str,
    pub weight: f32,
    /// Sens auquel le lien se rattache (0..=127), entrée (128 + n), ou [`SHARED`].
    pub tag: u8,
}

/// Sens d'un mot : son entrée (numéro local) et sa signature.
#[derive(Debug, Clone, PartialEq)]
pub struct SenseSig<'a> {
    pub group: u32,
    pub signature: Vec<&'a str>,
}

/// Fiche d'un mot : ses sens (vide s'il n'en a qu'un), ses liens, ses antonymes.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Record<'a> {
    pub senses: Vec<SenseSig<'a>>,
    pub links: Vec<Link<'a>>,
    pub antonyms: Vec<&'a str>,
}

/// Poids gardé par le sens le moins soutenu par le contexte.
const SENSE_FLOOR: f32 = 0.15;
/// Part du soutien d'un sens donnée aux autres sens de la même entrée.
const GROUP_SHARE: f32 = 0.5;
/// Lissage du soutien des sens (un sens sans soutien face à un sens soutenu à 1 : ≈ 0,1).
const SENSE_SMOOTHING: f32 = 0.1;

impl Record<'_> {
    /// Part de chaque sens selon le contexte : `context(mot)` dit combien un mot est présent
    /// autour (autres mots de la requête et leurs mots liés, 0..=1). Tous à 1 sans contexte
    /// qui départage.
    /// Soutien du mot entier par le contexte (signatures et liens qu'on y retrouve) : sert à
    /// choisir entre deux lemmes d'une même forme (« livre » nom, « livrer » verbe).
    pub fn support(&self, context: &dyn Fn(&str) -> f32) -> f32 {
        let sig: f32 = self
            .senses
            .iter()
            .flat_map(|s| &s.signature)
            .map(|w| context(w))
            .sum();
        let links: f32 = self.links.iter().map(|l| context(l.word) * l.weight).sum();
        sig + links
    }

    pub fn sense_weights(&self, context: &dyn Fn(&str) -> f32) -> Vec<f32> {
        let mut support = vec![0.0f32; self.senses.len()];
        for (i, s) in self.senses.iter().enumerate() {
            for w in &s.signature {
                support[i] += context(w);
            }
        }
        for l in &self.links {
            let c = context(l.word);
            if c == 0.0 {
                continue;
            }
            for (i, s) in self.senses.iter().enumerate() {
                if self.belongs(l.tag, i, s.group) {
                    support[i] += c * l.weight;
                }
            }
        }
        let best = support.iter().copied().fold(0.0, f32::max);
        if best <= 0.0 {
            return vec![1.0; self.senses.len()];
        }
        let own: Vec<f32> = support
            .iter()
            .map(|&s| ((SENSE_SMOOTHING + s) / (SENSE_SMOOTHING + best)).max(SENSE_FLOOR))
            .collect();
        // Les sens d'une même entrée se soutiennent en partie : « livre » a huit sens
        // « ouvrage », que le contexte ne départage guère.
        self.senses
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let group = self
                    .senses
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| t.group == s.group)
                    .map(|(j, _)| own[j])
                    .fold(0.0, f32::max);
                own[i].max(GROUP_SHARE * group)
            })
            .collect()
    }

    fn belongs(&self, tag: u8, sense: usize, group: u32) -> bool {
        match tag {
            SHARED => false,
            t if t >= GROUP_BASE => u32::from(t - GROUP_BASE) == group,
            t => usize::from(t) == sense,
        }
    }

    /// Liens pondérés par le sens que le contexte désigne (voir [`Self::sense_weights`]).
    pub fn in_context(&self, context: &dyn Fn(&str) -> f32) -> Vec<(&str, f32)> {
        let weights = self.sense_weights(context);
        self.links
            .iter()
            .map(|l| {
                let f = match l.tag {
                    SHARED => 1.0,
                    _ => self
                        .senses
                        .iter()
                        .enumerate()
                        .filter(|(i, s)| self.belongs(l.tag, *i, s.group))
                        .map(|(i, _)| weights[i])
                        .fold(0.0, f32::max),
                };
                // Rattachement à un sens inconnu (fichier abîmé) : poids plein.
                let f = if self.senses.is_empty() || f == 0.0 {
                    1.0
                } else {
                    f
                };
                (l.word, l.weight * f)
            })
            .collect()
    }
}

/// Liens compilés, en lecture seule.
pub struct Relations {
    fst: fst::Map<Vec<u8>>,
    records: Vec<u8>,
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

    fn string(&self, off: u64) -> Option<&str> {
        let mut s = self.strings.get(usize::try_from(off).ok()?..)?;
        let len = usize::try_from(read_varint(&mut s)?).ok()?;
        std::str::from_utf8(s.get(..len)?).ok()
    }

    /// Fiche de `lemma` ; vide s'il n'en a pas.
    pub fn record<'a>(&'a self, lemma: &str) -> Record<'a> {
        self.parse(lemma).unwrap_or_default()
    }

    fn parse<'a>(&'a self, lemma: &str) -> Option<Record<'a>> {
        let off = usize::try_from(self.fst.get(normalize(lemma))?).ok()?;
        let mut rest = self.records.get(off..)?;
        let words = |rest: &mut &[u8]| -> Option<Vec<&'a str>> {
            let n = read_varint(rest)?;
            (0..n).map(|_| self.string(read_varint(rest)?)).collect()
        };
        let n_senses = read_varint(&mut rest)?;
        let mut senses = Vec::new();
        for _ in 0..n_senses {
            let group = u32::try_from(read_varint(&mut rest)?).ok()?;
            senses.push(SenseSig {
                group,
                signature: words(&mut rest)?,
            });
        }
        let n_links = read_varint(&mut rest)?;
        let mut links = Vec::new();
        for _ in 0..n_links {
            let word = self.string(read_varint(&mut rest)?)?;
            let (&w, tail) = rest.split_first()?;
            let (&tag, tail) = tail.split_first()?;
            rest = tail;
            links.push(Link {
                word,
                weight: f32::from(w) / 255.0,
                tag,
            });
        }
        let antonyms = words(&mut rest)?;
        Some(Record {
            senses,
            links,
            antonyms,
        })
    }

    /// Lemmes liés à `lemma`, du plus proche au plus lointain, avec le poids du lien (0..=1),
    /// tous sens confondus.
    pub fn related<'a>(&'a self, lemma: &str) -> Vec<(&'a str, f32)> {
        self.record(lemma)
            .links
            .into_iter()
            .map(|l| (l.word, l.weight))
            .collect()
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let fst = self.fst.as_fst().as_bytes();
        let mut out =
            Vec::with_capacity(HEADER_LEN + fst.len() + self.records.len() + self.strings.len());
        out.extend_from_slice(MAGIC);
        for len in [fst.len(), self.records.len(), self.strings.len()] {
            out.extend_from_slice(&(len as u64).to_le_bytes());
        }
        out.extend_from_slice(fst);
        out.extend_from_slice(&self.records);
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
        let records = take(lens[1])?;
        let strings = take(lens[2])?;
        if !rest.is_empty() {
            return Err(LexiconError::Format("octets en trop"));
        }
        Ok(Self {
            fst,
            records,
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
        r.related(a)
            .into_iter()
            .find(|(x, _)| *x == b)
            .map_or(0.0, |(_, w)| w)
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
        let list = r.related("Élection");
        assert!(list.windows(2).all(|w| w[0].1 >= w[1].1), "{list:?}");
        let back = Relations::from_bytes(&r.to_bytes()).unwrap();
        assert_eq!(back.related("élection"), list);
        assert!(back.related("inconnu").is_empty());
    }

    /// « portable » : téléphone (sens 1, synonymes propres) ou ordinateur (sens 2).
    fn portable() -> Relations {
        let mut b = RelationsBuilder::new();
        let e = b.entry("portable");
        let phone = b.sense(e);
        let laptop = b.sense(e);
        for w in ["téléphone", "cellulaire"] {
            b.sign(phone, w);
            b.link_from("portable", w, Evidence::Gloss, 1.0, Origin::Sense(phone));
        }
        for w in ["gsm", "mobile"] {
            b.sign(phone, w);
            b.link_from("portable", w, Evidence::Synonym, 1.0, Origin::Sense(phone));
        }
        b.sign(laptop, "ordinateur");
        b.link_from(
            "portable",
            "ordinateur",
            Evidence::Gloss,
            gloss_strength(1),
            Origin::Sense(laptop),
        );
        // Synonyme cité pour toute l'entrée : son voisinage (« ordinateur ») le rattache au
        // sens 2.
        b.link_from(
            "portable",
            "laptop",
            Evidence::Synonym,
            1.0,
            Origin::Entry(e),
        );
        b.link("laptop", "ordinateur", Evidence::Synonym, 1.0);
        b.antonym("portable", "fixe");
        b.build(
            BuildOptions {
                per_lemma: 16,
                min_weight: 0.05,
            },
            |_| 1.0,
        )
        .unwrap()
    }

    #[test]
    fn links_are_attached_to_senses() {
        let r = Relations::from_bytes(&portable().to_bytes()).unwrap();
        let rec = r.record("portable");
        assert_eq!(rec.senses.len(), 2, "{rec:?}");
        let tag = |w: &str| rec.links.iter().find(|l| l.word == w).map(|l| l.tag);
        assert_eq!(tag("gsm"), Some(0));
        assert_eq!(tag("laptop"), Some(1), "{rec:?}");
        assert_eq!(rec.antonyms, ["fixe"]);
        assert_eq!(r.record("fixe").antonyms, ["portable"]);
    }

    #[test]
    fn context_picks_the_sense() {
        let r = portable();
        let rec = r.record("portable");
        let get = |list: &[(&str, f32)], w: &str| {
            list.iter().find(|(x, _)| *x == w).map_or(0.0, |(_, p)| *p)
        };
        // Seul : tous les sens comptent.
        let alone = rec.in_context(&|_| 0.0);
        assert!(get(&alone, "gsm") > 0.5);
        // Avec « ordinateur » : le sens 2.
        let ctx = |w: &str| if w == "ordinateur" { 1.0 } else { 0.0 };
        let with = rec.in_context(&ctx);
        // Même entrée du dictionnaire : le sens écarté garde la moitié.
        assert!(get(&with, "gsm") <= 0.51 * get(&alone, "gsm"), "{with:?}");
        assert!((get(&with, "laptop") - get(&alone, "laptop")).abs() < 1e-6);
    }

    #[test]
    fn senses_of_other_entries_fade_more() {
        // « livre » : l'ouvrage (entrée 1) et la monnaie (entrée 2).
        let mut b = RelationsBuilder::new();
        let book = b.entry("livre");
        let s_book = b.sense(book);
        let money = b.entry("livre");
        let s_money = b.sense(money);
        b.sign(s_book, "ouvrage");
        b.link_from(
            "livre",
            "bouquin",
            Evidence::Synonym,
            1.0,
            Origin::Sense(s_book),
        );
        b.sign(s_money, "sterling");
        b.link_from(
            "livre",
            "shilling",
            Evidence::Related,
            1.0,
            Origin::Sense(s_money),
        );
        let r = b.build(BuildOptions::default(), |_| 1.0).unwrap();
        let rec = r.record("livre");
        let ctx = |w: &str| if w == "sterling" { 1.0 } else { 0.0 };
        let list = rec.in_context(&ctx);
        let get = |w: &str| list.iter().find(|(x, _)| *x == w).map_or(0.0, |(_, p)| *p);
        let alone = r.related("livre");
        let base = |w: &str| alone.iter().find(|(x, _)| *x == w).map_or(0.0, |(_, p)| *p);
        assert!(get("bouquin") <= 0.16 * base("bouquin"), "{list:?}");
        assert!((get("shilling") - base("shilling")).abs() < 1e-6);
    }
}
