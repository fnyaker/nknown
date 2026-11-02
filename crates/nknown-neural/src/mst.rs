//! Décodage des scores d'arcs en arbre de dépendances : arbre couvrant de poids maximal
//! (Chu-Liu-Edmonds), avec un seul mot rattaché à la racine.
//!
//! Convention : nœud 0 = racine, mots 1..=n. `scores[d][h]` = score de « h gouverne d »
//! (la ligne 0 est ignorée). Résultat : `heads[d]` pour d dans 1..=n (`heads[0]` vaut 0).

/// Arbre de poids maximal avec un unique enfant de la racine.
pub fn decode(scores: &[Vec<f32>]) -> Vec<usize> {
    let n = scores.len();
    if n <= 1 {
        return vec![0; n];
    }
    let heads = chu_liu_edmonds(scores);
    let root_children: Vec<usize> = (1..n).filter(|&d| heads[d] == 0).collect();
    if root_children.len() <= 1 {
        return heads;
    }
    // Plusieurs mots rattachés à la racine : on essaie chacun comme unique racine
    // et on garde le meilleur arbre.
    let mut best: Option<(f32, Vec<usize>)> = None;
    for &root in &root_children {
        let mut constrained = scores.to_vec();
        for (d, row) in constrained.iter_mut().enumerate().skip(1) {
            if d != root {
                row[0] = f32::NEG_INFINITY;
            }
        }
        let candidate = chu_liu_edmonds(&constrained);
        let score = tree_score(scores, &candidate);
        if best.as_ref().is_none_or(|(s, _)| score > *s) {
            best = Some((score, candidate));
        }
    }
    best.map(|(_, h)| h).unwrap_or(heads)
}

fn tree_score(scores: &[Vec<f32>], heads: &[usize]) -> f32 {
    (1..heads.len()).map(|d| scores[d][heads[d]]).sum()
}

fn chu_liu_edmonds(scores: &[Vec<f32>]) -> Vec<usize> {
    let n = scores.len();
    // Meilleur gouverneur de chaque mot, hors boucle sur soi-même.
    let mut heads = vec![0usize; n];
    for (d, row) in scores.iter().enumerate().skip(1) {
        heads[d] = (0..n)
            .filter(|&h| h != d)
            .max_by(|&a, &b| row[a].total_cmp(&row[b]))
            .unwrap_or(0);
    }
    let Some(cycle) = find_cycle(&heads) else {
        return heads;
    };

    // Contraction du cycle en un nœud C.
    let mut in_cycle = vec![false; n];
    for &c in &cycle {
        in_cycle[c] = true;
    }
    let outside: Vec<usize> = (0..n).filter(|&v| !in_cycle[v]).collect(); // contient 0
    let c_new = outside.len();
    let cycle_score: f32 = cycle.iter().map(|&c| scores[c][heads[c]]).sum();

    let m = c_new + 1;
    let mut contracted = vec![vec![f32::NEG_INFINITY; m]; m];
    // Pour un mot hors cycle gouverné par C : quel nœud du cycle le gouverne réellement.
    let mut out_best = vec![0usize; m];
    // Pour C gouverné par h : quel nœud du cycle reçoit l'arc entrant.
    let mut in_best = vec![0usize; m];
    for (dn, &d) in outside.iter().enumerate().skip(1) {
        for (hn, &h) in outside.iter().enumerate() {
            if h != d {
                contracted[dn][hn] = scores[d][h];
            }
        }
        let (best_c, best) = cycle
            .iter()
            .map(|&c| (c, scores[d][c]))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .expect("cycle non vide");
        contracted[dn][c_new] = best;
        out_best[dn] = best_c;
    }
    for (hn, &h) in outside.iter().enumerate() {
        let (best_c, best) = cycle
            .iter()
            .map(|&c| (c, scores[c][h] - scores[c][heads[c]]))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .expect("cycle non vide");
        contracted[c_new][hn] = best + cycle_score;
        in_best[hn] = best_c;
    }

    let sub = chu_liu_edmonds(&contracted);

    // Expansion.
    let mut result = heads.clone();
    for (dn, &d) in outside.iter().enumerate().skip(1) {
        result[d] = if sub[dn] == c_new {
            out_best[dn]
        } else {
            outside[sub[dn]]
        };
    }
    let entering_from = sub[c_new];
    let entry = in_best[entering_from];
    result[entry] = outside[entering_from];
    result
}

/// Un cycle du graphe `heads` (sans la racine), s'il y en a un.
fn find_cycle(heads: &[usize]) -> Option<Vec<usize>> {
    let n = heads.len();
    // 0 = non visité, 1 = sur le chemin courant, 2 = terminé
    let mut state = vec![0u8; n];
    state[0] = 2;
    for start in 1..n {
        let mut path = Vec::new();
        let mut v = start;
        while state[v] == 0 {
            state[v] = 1;
            path.push(v);
            v = heads[v];
        }
        if state[v] == 1 {
            let pos = path
                .iter()
                .position(|&p| p == v)
                .expect("v est sur le chemin");
            return Some(path[pos..].to_vec());
        }
        for p in path {
            state[p] = 2;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const NEG: f32 = f32::NEG_INFINITY;

    fn is_tree(heads: &[usize]) -> bool {
        find_cycle(heads).is_none() && (1..heads.len()).filter(|&d| heads[d] == 0).count() == 1
    }

    #[test]
    fn greedy_tree_is_kept() {
        // racine -> 2 ; 2 -> 1 ; 2 -> 3
        let s = vec![
            vec![NEG; 4],
            vec![0.0, NEG, 5.0, 1.0],
            vec![5.0, 1.0, NEG, 1.0],
            vec![0.0, 1.0, 5.0, NEG],
        ];
        assert_eq!(decode(&s), [0, 2, 0, 2]);
    }

    #[test]
    fn breaks_cycles() {
        // 1 et 2 se veulent mutuellement : le meilleur arbre casse le cycle par la racine.
        let s = vec![vec![NEG; 3], vec![1.0, NEG, 10.0], vec![2.0, 10.0, NEG]];
        let h = decode(&s);
        assert!(is_tree(&h));
        assert_eq!(h, [0, 2, 0]);
    }

    #[test]
    fn single_root_is_enforced() {
        // Les deux mots préfèrent la racine ; un seul peut l'avoir.
        let s = vec![vec![NEG; 3], vec![9.0, NEG, 1.0], vec![8.0, 5.0, NEG]];
        assert_eq!(decode(&s), [0, 0, 1]);
    }

    #[test]
    fn random_matrices_give_valid_trees() {
        let mut x = 0x2545_f491_4f6c_dd1du64;
        for n in 2..12 {
            for _ in 0..30 {
                let s: Vec<Vec<f32>> = (0..n)
                    .map(|_| {
                        (0..n)
                            .map(|_| {
                                x ^= x << 13;
                                x ^= x >> 7;
                                x ^= x << 17;
                                (x % 1000) as f32 / 100.0
                            })
                            .collect()
                    })
                    .collect();
                assert!(is_tree(&decode(&s)), "n={n}");
            }
        }
    }
}
