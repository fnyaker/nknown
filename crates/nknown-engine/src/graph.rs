//! Modèle du graphe : nœuds typés, arêtes typées et pondérées.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeKind {
    Lemma,
    /// Un sens précis d'un lemme (« Paris » la ville ≠ « Paris » le prénom).
    Sense,
    Passage,
    Document,
    Source,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum EdgeKind {
    /// Sens → passage où il apparaît.
    Occurs = 1,
    /// Sens ↔ sens apparaissant ensemble.
    CoOccurs = 2,
    /// Adjectif → nom qu'il qualifie.
    Qualifies = 3,
    /// Passage ↔ passage proches.
    Similar = 4,
    /// Passage → document.
    PartOf = 5,
    /// Document → source.
    FromSource = 6,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Edge {
    pub from: NodeId,
    pub to: NodeId,
    pub kind: EdgeKind,
    pub weight: f32,
}

/// Préfixe d'espace de clés réservé aux arêtes.
pub const EDGE_KEYSPACE: u8 = b'E';

impl Edge {
    /// Clé de stockage `E | from | kind | to` (big-endian) : un scan par préfixe
    /// `E | from` donne tous les voisins sortants d'un nœud, groupés par type.
    pub fn key(&self) -> [u8; 18] {
        let mut k = [0u8; 18];
        k[0] = EDGE_KEYSPACE;
        k[1..9].copy_from_slice(&self.from.0.to_be_bytes());
        k[9] = self.kind as u8;
        k[10..].copy_from_slice(&self.to.0.to_be_bytes());
        k
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_keys_group_by_source_node() {
        let e = |from, to| Edge {
            from: NodeId(from),
            to: NodeId(to),
            kind: EdgeKind::Occurs,
            weight: 1.0,
        };
        assert!(e(1, 999).key() < e(2, 0).key());
        assert!(e(1, 1).key() < e(1, 2).key());
    }
}
