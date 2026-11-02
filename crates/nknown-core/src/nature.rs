//! Nature d'un contenu : un vecteur de scores, pas une étiquette unique.

/// Dimensions de nature. En ajouter une = ajouter une variante ici.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Nature {
    /// Actualité, événement daté.
    Event,
    /// Connaissance stable, encyclopédique.
    Reference,
    Opinion,
    Tutorial,
    /// Document personnel (notes, fichiers).
    Personal,
    Discussion,
}

impl Nature {
    pub const ALL: [Nature; 6] = [
        Nature::Event,
        Nature::Reference,
        Nature::Opinion,
        Nature::Tutorial,
        Nature::Personal,
        Nature::Discussion,
    ];
    pub const COUNT: usize = Self::ALL.len();
}

/// Score dans `[0, 1]` pour chaque [`Nature`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct NatureProfile([f32; Nature::COUNT]);

impl NatureProfile {
    pub fn get(&self, n: Nature) -> f32 {
        self.0[n as usize]
    }

    pub fn set(&mut self, n: Nature, score: f32) {
        self.0[n as usize] = score.clamp(0.0, 1.0);
    }

    pub fn from_scores(scores: [f32; Nature::COUNT]) -> Self {
        Self(scores.map(|s| s.clamp(0.0, 1.0)))
    }

    pub fn scores(&self) -> [f32; Nature::COUNT] {
        self.0
    }

    /// Pertinence vis-à-vis des poids d'un opérateur de requête (ex. `actu`).
    pub fn weigh(&self, weights: &NatureProfile) -> f32 {
        self.0
            .iter()
            .zip(weights.0.iter())
            .map(|(a, b)| a * b)
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weigh_favours_matching_nature() {
        let mut news = NatureProfile::default();
        news.set(Nature::Event, 0.9);
        let mut encyclo = NatureProfile::default();
        encyclo.set(Nature::Reference, 0.9);

        let mut actu = NatureProfile::default();
        actu.set(Nature::Event, 1.0);

        assert!(news.weigh(&actu) > encyclo.weigh(&actu));
    }
}
