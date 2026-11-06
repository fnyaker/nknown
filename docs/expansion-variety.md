# Élargissement de requête : audit et niveaux de variété

Statut : implémenté dans la source de production (moteur, façade, API, CLI, application Qt
et interface web). Les mesures ci-dessous ont été refaites avec les fonctions de production ;
les écarts avec le prototype sont signalés. Le harnais de niveaux de production est
`crates/nknown-service/tests/expansion_levels.rs`. L'injection des *textes liés* reste en
phase 2 et n'est ni implémentée ni mesurée.

## Verdict

1. Le problème est réel et reproductible sur la base de l'utilisateur (copie de
   `~/.local/share/nknown/base.redb`, 2,1 Go). Avec le niveau Normal actuel,
   `php flaw` ne place **aucun** des 5 documents « PHP vulnerability » dans le top 10 ;
   4 des 5 sont aux rangs 28–31 sur 50 (le cinquième, `linkedin_post`, est absent du
   top 50), trouvés seulement par le mot `php` (couverture 0,25), avec un score de 2,5
   contre 4,95 pour le premier résultat. `php vulnerability`, lui, place les 5 mêmes
   documents aux rangs 1–5.
2. La cause n'est pas le partage `RELATED_SHARE = 0.5` ni la coupure : c'est un lien
   **directionnel manquant** dans le dictionnaire (`flaw` n'a pas de lien sortant vers
   `vulnerability`, alors que `vulnerability → flaw` vaut 0,73). Ce lien existe pourtant
   déjà dans la base sous forme de lien **appris thématique** (`flaw ↔ vulnerability`,
   cosinus 0,464, 27 passages partagés), mais `add_learned` le jette : il est `Topical`
   et le dictionnaire de `flaw` n'est pas vide. Voir aussi la section « où un lien se perd ».
3. Un modèle à 3 niveaux est faisable avec les données existantes :
   - **Strict** : liens du dictionnaire ≥ 0,55 et poids effectif ≥ 0,50, part 0,35,
     4 liens retenus, couverture d'un lien lié = 0,5. Mesuré : 13/19 positifs,
     3/13 faux positifs durs, 0/32 distrayants.
   - **Normal** : comportement actuel inchangé. Mesuré : 14/19, 3/13, 0/32.
   - **Diversifié** : liens faibles, signatures de définitions validées par un lien
     retour, second pas validé, liens appris thématiques autorisés. Mesuré : 16/19,
     3/13, 0/32 ; sur la base réelle, top 10 de `php flaw` : 0/5 → 5/5 documents PHP,
     latence release 3,4 → 5,6 ms (+62 %).
4. Le jeu tenu à l'écart ne montre **ni gain ni régression** de Diversifié (9/10 positifs,
   0/6 faux positif dur, identique à Normal ; Strict 8/10). Le jeu a été remplacé après
   revue : 10 de ses 16 lignes partageaient une requête ou un document avec le
   développement. Les échantillons restent petits : le gain de 14/19 à 16/19 n'est pas
   significatif à lui seul ; la preuve forte est la mesure sur la base réelle et le cas
   contrôlé `php flaw`.
5. Le français reste un échec de données : `faille` n'a aucun lien (appris ou non) vers
   `vulnérabilité` ; `php faille` ne renvoie qu'un résultat, quel que soit le niveau.
   Aucun slider ne corrige un lien absent ; il faut corriger la construction des
   relations ou élargir la politique d'apprentissage.

## Comportement actuel, avec preuves

### Base contrôlée (2 documents, modèles réels)

Documents : « PHP flaw … serious security defect » et « PHP vulnerability … serious
security defect ». Index construit avec la source courante (pas le binaire installé).

| Requête | Document trouvé | Score | Mots trouvés | Couverture |
|---|---|---:|---|---:|
| `php flaw` | flaw | 0,753 | php, flaw | 1,00 |
| `php flaw` | vuln | 0,369 | php, **defect** | 1,00 |
| `php vulnerability` | vuln | 0,901 | php, vulnerability | 1,00 |
| `php vulnerability` | flaw | 0,637 | php, **flaw** | 1,00 |

Le second document n'est trouvé par `php flaw` que parce qu'il contient aussi
`defect`, lien sortant de `flaw` (0,45). Sans ce mot, il n'y a aucun candidat.

### Base réelle (copie `/tmp/opencode/base-audit.redb`, 2,1 Go)

| Requête | Niveau | Candidats | Coupure | Docs PHP dans le top 10 | Rangs des docs PHP |
|---|---|---:|---:|---:|---|
| `php flaw` | Normal | 509 | 19 % | **0/5** | 4/5 aux rangs 28–31 sur 50, score 2,5 |
| `php vulnerability` | Normal | 663 | 19 % | **5/5** | 1–5, score 10,3–16,5 |
| `php flaw` | Diversifié | — | — | **5/5** | 1, 2, 4, 5, 6 |
| `php vulnerability` | Diversifié | — | — | 5/5 | 1–5 (inchangé) |

Les documents PHP trouvés par `php flaw` en Normal ont `mots = ["php"]` et une
couverture de 0,25 : l'élargissement n'a pas contribué. En Diversifié, ils ont
`mots = ["php", "vulnerability"]` et une couverture de 1,0.

Dans le harnais contrôlé (documents courts, sans `defect`), `php flaw` donne :
Normal positif 0,229 / négatif 0,229 (égalité, seulement `php` trouvé) ;
Diversifié positif 0,620 (`php` + `vulnerability`) / négatif 0,115.
`authentication flaw` suit la même bascule : 0,318 / 0,318 → 0,797 / 0,159. Les documents « flaw »
génériques passent des rangs 2–9 à 7–9 (score ~1,9), parce que l'exposant de couverture
passe de 2 à 3 quand assez de candidats ont une couverture complète — effet de bord
à surveiller (voir Risques).

Sonde des liens appris (`learned_probe`, lecture seule) :

```
eng flaw:          [("vulnerability", 0.46446884, 27, Topical)]
eng vulnerability: [("flaw", 0.46446884, 27, Topical)]
eng php:           []
fra faille:        [("exploitation", 0.35174045, 18, Topical),
                    ("correctif", 0.30072036, 16, Topical)]
fra vulnérabilité: []
```

Politique actuelle de `add_learned` : un lien appris seul n'est accepté que si le
dictionnaire ne dit rien du mot (`silent`) **et** s'il est `Syntactic`, avec cosinus
≥ 0,45 et 8 passages partagés. Le lien `flaw ↔ vulnerability` est `Topical` et le
dictionnaire de `flaw` n'est pas vide : il est donc ignoré.

### Où un lien peut être perdu ou enterré

1. **Lien sortant absent** : `flaw` n'a pas `vulnerability` dans ses liens. Chercher
   `flaw` ne peut pas trouver un document qui ne contient que `vulnerability` + une
   ancre (`php`). C'est le cas utilisateur.
2. **Lien appris refusé** : le lien existe (cosinus 0,464, 27 passages) mais la
   politique « dictionnaire silencieux + syntactique » le rejette (voir ci-dessus).
3. **Pondération de sens + troncature** : `software bug` a un lien `glitch` de 0,63 ;
   après `in_context`, le sens logiciel non soutenu tombe au plancher 0,15 (poids ~0,09),
   et `related_words` tronque à `MAX_RELATED = 24` : `glitch` disparaît de la requête.
   Le document « software glitch » n'est alors pas candidat, et `insect` (0,85) gagne.
4. **Plafond `RELATED_KEPT = 8`** : le moteur ne garde que 8 liens **présents dans
   l'index**, après tri par force. Expérience contrôlée : 8 documents contenant
   `flawless, tumult, uproar, quarrel, imperfection, windflaw, imperfect, unflawed`
   occupent les 8 places ; le document `defect` (12e lien, 0,45) n'est **pas** candidat
   en Normal. En Diversifié (`RELATED_KEPT = 16`), il apparaît au rang 10.
5. **Part du lien lié** : `RELATED_SHARE = 0.5` et couverture d'un lien lié = 1,0.
   Un document qui ne matche que par un lien plafonne donc à ~0,5 du texte d'un match
   direct. Quand le premier résultat a en plus un bonus de titre (×1,6) et de
   qualification (×1,5), le ratio peut descendre sous la coupure relative
   (`0,05 + 0,05·log10(candidats)`, plafonnée à 0,30). Ici les documents PHP sont à
   53 % du meilleur, donc au-dessus de la coupure de 19 % : le problème est le top-k,
   pas la coupure.
6. **Fusion des doublons** : si le document lié est un quasi-doublon d'un résultat
   direct, il est replié dans `related` (seuil 0,4–0,95 selon les sources) et n'apparaît
   pas comme résultat séparé. Non mesuré ici (similarité 60 % dans le cas contrôlé,
   donc non replié).

## Modèle à trois niveaux

### Constantes

| Constante | Strict | Normal (défaut) | Diversifié |
|---|---:|---:|---:|
| `MIN_RELATED` (dictionnaire) | 0,55 | 0,30 | 0,20 |
| `MIN_RELATED_EFFECTIF` (après contexte) | 0,50 | 0 | 0 |
| `MAX_RELATED` (côté requête) | 12 | 24 | 48 |
| `RELATED_SHARE` (moteur) | 0,35 | 0,50 | 0,60 |
| `RELATED_KEPT` (liens présents) | 4 | 8 | 16 |
| Couverture d'un lien lié | 0,50 | 1,00 | 1,00 |
| Second pas | non | seulement pour le choix de sens (`SECOND_HOP = 0.25`) | ajouté, validé |
| Signatures de définitions | non | non | ajoutées, validées |
| Mots plus généraux (`broader`) | non | non (soutien de contexte seulement) | non (bruit mesuré) |
| Liens appris seuls | non | dictionnaire silencieux + syntactique | syntactique et thématique |
| Textes liés (graphe) | non | non | phase 2, non mesuré |
| Coupure relative | inchangée | inchangée | inchangée |
| PageRank / autorité / emballement / doublons | inchangés | inchangés | inchangés |

### Sémantique exacte

Effet attendu (avant mesure) :

| Niveau | Rappel | Précision | Latence |
|---|---|---|---|
| Strict | ↓ | ↑ sur le bruit de liens faibles | ≈ Normal |
| Normal | référence | référence | référence |
| Diversifié | ↑ | ↓ possible (liens plus larges ; textes liés : phase 2, non mesuré) | +50 à +150 % |

**Normal** doit rester bit-à-bit identique : mêmes constantes, mêmes fonctions,
même ordre. `Query::variety` vaut `Normal` par défaut ; `build_query`, `build_query_with`,
`build_query_in` et `add_learned` gardent leur signature et leur comportement.

**Strict** :
- ne garder que les liens du dictionnaire ≥ 0,55 **et** dont le poids après contexte
  est ≥ 0,50 (écarte les liens non soutenus) ;
- 12 liens au plus côté requête, 4 liens présents retenus par le moteur ;
- part 0,35, couverture d'un lien lié 0,5 (deux liens liés ne valent qu'un mot direct) ;
- pas de second pas **ajouté** : le second pas reste utilisé pour le choix du sens, comme
  en Normal (voir la note d'implémentation plus bas), mais ne crée aucun lien de requête ;
  pas de signature, pas de `broader`, pas de lien appris seul ;
  les liens du dictionnaire confirmés par le corpus restent acceptés ;
- coupure inchangée : la baisse de score suffit souvent à écarter les liens faibles.

**Diversifié** :
- dictionnaire ≥ 0,20 (le fichier `.rel` a un plancher de 0,25), 48 liens côté requête,
  16 liens présents retenus, part 0,60 ;
- **signatures de définitions** : pour chaque lecture de contenu, compter les mots de
  signature des sens ; garder ceux présents dans ≥ 2 sens, résolus en lemme/catégorie
  de contenu par le lexique, et **validés par un lien retour** `candidat → mot source`
  ≥ 0,50. Poids `min(0,32 ; 0,10 + 0,06·n)`. Plafond 12 **après** validation.
  Exemple : `vulnerability` apparaît dans 3 sens de `flaw` et `vulnerability → flaw`
  vaut 0,73 → ajouté à 0,28. `php` n'ajoute pas `level`, `high`, `language` : ces mots
  ne reviennent pas vers `php`.
- **second pas** : pour les 8 meilleurs liens `(lang, w, p)`, prendre 8 voisins de `w`
  de force ≥ 0,45, exiger un lien retour `voisin → mot source` ≥ 0,50, poids
  `min(0,20 ; 0,12·p·q)`, 12 ajouts après validation.
- **liens appris** : accepter même quand le dictionnaire n'est pas silencieux :
  - `Syntactic` : cosinus ≥ 0,55, ≥ 12 passages partagés ;
  - `Topical` : cosinus ≥ 0,45, ≥ 15 passages partagés ;
  - poids `min(0,30 ; 0,60·cosinus)`, contraires écartés, réciprocité supposée acquise
    par `Engine::learned`.
- **textes liés** (phase 2, non implémenté) : après le classement graphe et avant la
  coupure, prendre les 10 meilleurs résultats, et pour chaque voisin `Similar` de
  similarité ≥ 0,45 absent des résultats, insérer son premier passage avec
  `score = score_parent × similarité × 0,35`, au plus 5 par parent et 10 au total.
  Ne pas appliquer aux requêtes de code, ni changer PageRank/autorité. Chaque résultat
  inséré doit être marqué « texte lié » dans l'API et l'interface. Non mesuré dans ce
  rapport : à évaluer séparément (risque de boilerplate et de faux positifs).

### API et interface

- `nknown-api::SearchRequest` : champ `#[serde(default)] pub variety: Variety`
  (`Variety::{Strict, Normal, Diverse}`, `Default = Normal`, `rename_all = "snake_case"`).
  Un ancien client qui n'envoie pas le champ obtient Normal (testé).
- `nknown-engine::Query` : champ `pub variety: Variety` ; `Default` = Normal. L'enum est
  dupliqué côté API (nknown-api ne dépend pas du cœur) et converti dans `nknown-service`.
- CLI : `nknown search <base> <lex> <requête> [nombre] [--variety=strict|normal|diverse]`
  (ou `--variety <niveau>`). `--explain` affiche le niveau retenu et les mots liés ;
  l'origine fine d'un lien (dictionnaire, appris, signature, second pas) n'est pas encore
  exposée.
- Application Qt / web : curseur à 3 positions implémenté dans les deux interfaces, valeur
  mémorisée et envoyée dans le corps POST `variety` (jamais dans l'URL), défaut Normal. Le
  pont Qt expose la propriété `variety` (`"normal"` par défaut) et l'API web prend le niveau
  en argument.

### Libellés français recommandés

| Position | Libellé | Infobulle |
|---|---|---|
| 1 | **Strict** | « Liens de sens très proches uniquement ; moins de résultats, plus précis. » |
| 2 | **Normal** (défaut) | « Élargissement habituel. » |
| 3 | **Diversifié** | « Liens plus larges et définitions validées ; plus de résultats, plus de bruit possible. » |

Curseur : « Variété des résultats ». Recommandation : Strict et Normal ne changent que
l'élargissement ; les textes liés (phase 2) ne sont pas implémentés dans cette première
version. Ne pas faire
varier PageRank, l'autorité, l'emballement ni les seuils de doublons avec le curseur :
ces signaux sont structurels et doivent rester stables.

## Évaluation

### Hypothèses et critères

- **H1 (reproduction)** : le Normal courant donne `php flaw` top 10 = 0/5 documents PHP
  et `php vulnerability` top 10 = 5/5. Critère : les deux mesures sur la copie de la
  base utilisateur.
- **H2 (Diversifié)** : `php flaw` top 10 ≥ 4/5, positifs dev ≥ Normal, faux positifs
  durs ≤ Normal + 1, distrayants = 0. Critère de réussite : les quatre à la fois.
- **H3 (Strict)** : part des liens liés ≤ Normal et faux positifs durs ≤ Normal.
  Critère : au moins un des deux, sans dégrader les distrayants.
- **H4 (tenu à l'écart)** : pas de régression de positifs ni de faux positifs durs par
  rapport à Normal. Un gain n'est pas exigé (jeu petit).

### Jeux de données

`eval/expansion.tsv` (développement, 32 lignes : 19 positifs, 13 négatifs durs) et
`eval/expansion-heldout.tsv` (16 lignes : 10 positifs, 6 négatifs durs). Format :
`lang query positif négatif distrayant catégorie note`.

Chaque ligne construit trois documents contrôlés dans une base mémoire :

- le positif contient le mot correct, mais pas le mot tapé (sauf ancre commune) ;
- le négatif contient un lien voisin faux (contraire, homonyme, terme trop général) ;
- le distrayant ne contient ni mot de la requête ni lien attendu.

Métriques : le positif bat le négatif d'au moins 5 % (une égalité compte comme échec) ;
faux positif dur = le négatif bat le positif de plus de 5 % ; distrayant retourné =
le document sans lien apparaît dans les résultats. Latence = construction de requête +
élargissement + recherche, 10 résultats.

### Résultats

Machine : AMD Ryzen 7 8845HS, 16 threads, 46 Gio. Analyse de requête CPU uniquement
(`analyze_light` : lexique `.lex`, tagueur `.tag`, relations `.rel`) ; aucune couche
neuronale, aucune quantification Q4_K/Q8_0, recherche mono-thread (calcul flottant f32). Le binaire est construit
depuis la source courante (pas `/usr/bin/nknown`, pas `target/debug` périmé).

Développement (32 lignes) :

| Niveau | Positifs | Faux positifs durs | Distrayants | Latence médiane | Max |
|---|---:|---:|---:|---:|---:|
| Strict | 13/19 (68 %) | 3/13 (23 %) | 0/32 | 0,34 ms | 4,2 ms |
| Normal | 14/19 (74 %) | 3/13 (23 %) | 0/32 | 0,28 ms | 4,2 ms |
| Diversifié | **16/19 (84 %)** | 3/13 (23 %) | 0/32 | 2,37 ms | 13,9 ms |

Tenu à l'écart (16 lignes). Le jeu a été remplacé après revue : 10 des 16 lignes
partageaient une requête ou un document avec le développement. Les 16 lignes actuelles
n'ont aucune requête ni aucun document en commun avec `eval/expansion.tsv`.

| Niveau | Positifs | Faux positifs durs | Distrayants | Latence médiane | Max |
|---|---:|---:|---:|---:|---:|
| Strict | 8/10 | 0/6 | 0/16 | 0,12 ms | 2,18 ms |
| Normal | 9/10 | 0/6 | 0/16 | 0,19 ms | 2,70 ms |
| Diversifié | 9/10 | 0/6 | 0/16 | 1,66 ms | 8,57 ms |

Une seule ligne positive échoue (non identifiée dans cette exécution sans
`NKNOWN_VERBOSE`) ; aucune ne produit de faux positif dur. Le jeu reste un test de
fumée : 10 positifs ne permettent pas de conclure à une absence de régression fine.

Base réelle (release, 2,1 Go, 509/663 candidats) :

| Requête | Niveau | Latence | Docs PHP top 10 |
|---|---|---:|---:|
| `php flaw` | Normal | 3,4 ms | 0/5 |
| `php flaw` | Diversifié | 5,6 ms | 5/5 |
| `php vulnerability` | Normal | 3,4 ms | 5/5 |
| `php vulnerability` | Diversifié | 4,4 ms | 5/5 |
| `php faille` | Normal / Diversifié | 17,9 / 17,1 ms | 1 résultat, aucun lien |

### Après intégration en production

Commandes (mêmes modèles, même machine, release, mono-thread) :

```sh
cargo test --release -p nknown-service --test expansion_baseline expansion_baseline_dev -- --nocapture
cargo test --release -p nknown-service --test expansion_levels expansion_levels_dev -- --nocapture
cargo test --release -p nknown-service --test expansion_levels expansion_levels_heldout -- --ignored --nocapture
NKNOWN_BASE=/tmp/opencode/base-audit.redb \
  cargo test --release -p nknown-service --test expansion_levels expansion_levels_real_corpus -- --ignored --nocapture
```

Développement (32 lignes, élargissement réellement exécuté par `build_query_in_variety` et
`add_learned`, sans transformation dans le test) :

| Niveau | Positifs | Faux positifs durs | Distrayants | Latence médiane | Max |
|---|---:|---:|---:|---:|---:|
| Strict | 13/19 | 3/13 | 0/32 | 0,32 ms | 4,7 ms |
| Normal | 14/19 | 3/13 | 0/32 | 0,31 ms | 4,5 ms |
| Diversifié | 16/19 | **2/13** | 0/32 | 2,48 ms | 15,9 ms |

Tenu à l'écart (16 lignes) :

| Niveau | Positifs | Faux positifs durs | Distrayants |
|---|---:|---:|---:|
| Strict | 8/10 | 0/6 | 0/16 |
| Normal | 9/10 | 0/6 | 0/16 |
| Diversifié | 9/10 | 0/6 | 0/16 |

Base réelle (`/tmp/opencode/base-audit.redb`, 2,1 Go) :

| Requête | Niveau | Latence | Docs PHP dans le top 10 |
|---|---|---:|---:|
| `php flaw` | Strict | 3,9 ms | 0/5 |
| `php flaw` | Normal | 3,3 ms | 0/5 |
| `php flaw` | Diversifié | 5,2 ms | **5/5** |
| `php vulnerability` | Strict / Normal / Diversifié | 3,2 / 3,3 / 4,0 ms | 5/5 |
| `php faille` | Strict / Normal / Diversifié | 14,9 / 16,7 / 14,4 ms | 1 résultat, aucun lien |

Écarts avec le prototype et décisions d'implémentation :

- **Second pas en Strict** : le prototype mesuré appliquait son filtre Strict *après* une
  requête Normal, donc le second pas servait encore au choix du sens (« bug » logiciel et
  non « bog »). La production garde ce contexte de désambiguïsation mais n'ajoute **aucun
  lien** de second pas en Strict ; c'est ce qui reproduit 13/19 et 3/13. En Normal et en
  Diversifié, le contexte reste identique à l'existant ; Diversifié ajoute en plus les
  voisins validés.
- **Plancher dictionnaire Diversifié à 0,20** : le prototype partait des liens Normal
  (plancher 0,30) ; la production applique bien 0,20. Effet mesuré en développement :
  2/13 faux positifs durs au lieu de 3/13. Sur le jeu tenu à l'écart remplacé, les trois
  niveaux sont à 0/6 : ce jeu ne permet pas de mesurer cet effet. Échantillons petits :
  à confirmer.
- **Liens appris Diversifié** : union de la règle historique (dictionnaire silencieux +
  syntactique ≥ 0,45 / 8) et de la règle large (syntactique ≥ 0,55 / 12, thématique
  ≥ 0,45 / 15), poids maximal des deux, contraires écartés.
- **Ordre** : signatures et second pas sont ajoutés dans `add_learned_in` après les liens
  appris, comme dans le prototype ; les plafonds (12 signatures, 12 voisins) s'appliquent
  après validation.
- **Latence** : `php flaw` Diversifié reste sous la cible de 10 ms (5,4 ms), proche du
  prototype (5,6 ms). `php faille` reste à ~14–17 ms, comme avant : le coût vient des
  candidats de la base, pas de l'élargissement.

### Échecs et limites

- **`php flaw` en Normal** : égalité 0,229 dans le cas contrôlé ; aucun document PHP
  dans le top 10 réel. Corrigé par Diversifié.
- **`authentication flaw`** : même égalité 0,318, corrigée par Diversifié (0,797).
- **`faille` / `vulnérabilité` (français)** : aucun lien dans les deux sens. Les liens
  appris de `faille` sont `exploitation` (0,352) et `correctif` (0,301), pas
  `vulnérabilité`. Échec attendu de tous les niveaux ; problème de données.
- **`zero-day`** : aucune fiche de relations ; aucun élargissement possible.
- **`heron` → `bird`** : `broader` n'est pas utilisé pour la recherche ; aucun candidat.
  Diversifié ne l'ajoute pas (le bruit mesuré sur `php` justifiait de le laisser de côté).
- **`software bug` → `glitch`** : lien 0,63 perdu par le plancher de sens (0,15) et la
  troncature à 24 ; `insect` (0,85) gagne. Non corrigé par les niveaux.
- **`river bank`**, **`livre sterling`**, **`chat en ligne`**, **`мышь компьютер`** :
  ambiguïtés de sens que le contexte ne départage pas assez ; Normal et Diversifié
  peuvent préférer le mauvais lien. Strict ne les corrige pas.
- **Strict n'est pas une garantie de précision** : `souris ordinateur` garde `mulot`
  (0,615) mais jette `pointeur` (0,37 < 0,55), donc le mauvais document gagne. Un seuil
  ne remplace pas la désambiguïsation de sens.
- **Effet de bord de couverture** : Diversifié crée plus de candidats à couverture
  complète, ce qui fait passer l'exposant de couverture de 2 à 3 et abaisse les
  correspondances partielles (documents `flaw` génériques de 2–4,5 à ~1,9). À décider :
  calculer l'exposant sur les correspondances directes seulement, ou l'accepter.
- **Petits échantillons** : 19 positifs en développement, 10 en tenu à l'écart. Le gain
  14→16 n'est pas significatif (McNemar exact, 2 gains / 0 perte, p ≈ 0,5). La preuve
  principale est la mesure réelle (0/5 → 5/5 sur une famille de requêtes), pas une
  supériorité générale.
- **Coût** : Diversifié fait un `Relations::record` par candidat de signature et par
  voisin de second pas. Sur la base réelle, +62 % de latence (3,4 → 5,6 ms) ; sur le
  petit jeu, 2,37 ms contre 0,28 ms. Prévoir un cache par lemme et un plafond après
  validation avant de l'activer par défaut.

## Handoff initial (appliqué)

### Fichiers et fonctions

1. `crates/nknown-engine/src/search.rs`
   - ajouter `pub enum Variety` + `related_share()`, `related_kept()`,
     `related_coverage()` ;
   - `Query.variety` (défaut Normal) ;
   - `search_observed` : utiliser `q.variety.related_share()` / `related_kept()` ;
     `Candidate.coverage_units` (1 par mot direct, `related_coverage()` par lien lié) ;
   - phase 2 : injection des textes liés après le score graphe, avant coupure.
2. `crates/nknown-engine/src/lib.rs` : réexporter `Variety`.
3. `crates/nknown-api/src/lib.rs` : `Variety` + `SearchRequest.variety` avec
   `#[serde(default)]` ; mettre à jour les littéraux `SearchRequest` dans
   `nknown-service` (tests), `nknown-server/src/plasma.rs`,
   `nknown-desktop/src/client.rs`, `nknown-web/src/api.rs`.
4. `crates/nknown-service/src/query.rs`
   - `struct Relax { min_related, max_related, min_effective, second_hop,
     signatures, learned_topical, learned_syntactic }` + `relax(Variety)` ;
   - `related_words(..., relax)` : seuil dictionnaire, seuil effectif, troncature ;
   - `choose_senses(...)` : second pas seulement Normal (choix de sens) / Diversifié
     (ajout validé) ;
   - `add_learned_in(..., variety)` : Normal inchangé ; Diversifié accepte
     `Topical` (≥0,45/15) et `Syntactic` (≥0,55/12) même si le dictionnaire n'est pas
     silencieux ; garder l'écartement des contraires ;
   - `build_query_in_variety`, `build_query_with_variety` ; les anciennes fonctions
     appellent Normal ;
   - signatures : fonction dédiée, comptage par sens, validation par lien retour,
     tri déterministe (poids puis mot), plafond après validation.
5. `crates/nknown-service/src/lib.rs` : convertir `api::Variety` → `engine::Variety`,
   passer le niveau à `build_query_in_variety` et `add_learned_in`.
6. `crates/nknown-cli/src/main.rs` : `--variety` ; `--explain` affiche le niveau et
   l'origine des liens.
7. Interfaces : designer pour le curseur ; engineer pour le champ POST et la
   compatibilité (défaut Normal).

### Tests à ajouter

- `variety_normal_is_legacy` : mêmes scores/queries que le harnais Normal actuel
  (`php flaw` 0,229, `php vulnerability` 0,722, 14/19).
- `strict_coverage_is_half` : un lien lié seul donne une couverture de 0,5 ; deux liens
  liés ne valent qu'un mot direct.
- `diverse_signature_needs_reverse_link` : `vulnerability` ajouté à `flaw` (3 sens +
  retour 0,73) ; `level` non ajouté à `php`.
- `diverse_topical_learned_only_in_diverse` : lien `Topical` 0,46/27 accepté en
  Diversifié, refusé en Normal.
- `related_kept_crowding` : 8 liens présents avant `defect` ; Normal l'exclut,
  Diversifié l'inclut (test dans `crates/nknown-service/tests/expansion_levels.rs`).
- `serde_search_request_defaults_to_normal` : ancien JSON sans `variety`.
- `expansion_levels` : harnais sous `crates/nknown-service/tests/` ; seuils : Normal ≥ 14/19, Diversifié ≥ 16/19,
  faux positifs durs ≤ 4/13, distrayants = 0 ; tenu à l'écart : pas de régression.
- `related_texts` (phase 2) : similarité ≥ 0,45, plafond 10, marquage API, pas de
  texte lié pour une requête de code.

## Commandes exactes

```sh
# Source courante (pas le binaire installé)
cargo build -p nknown-cli

# Baseline Normal, développement
cargo test --release -p nknown-service --test expansion_baseline expansion_baseline_dev -- --nocapture

# Baseline Normal, tenu à l'écart (une fois)
cargo test --release -p nknown-service --test expansion_baseline expansion_baseline_heldout -- --ignored --nocapture

# Sonde des liens appris (lecture seule)
NKNOWN_BASE=/tmp/opencode/base-audit.redb \
  cargo test --release -p nknown-service --test expansion_baseline learned_probe -- --ignored --nocapture

# Harnais de production : Strict / Normal / Diversifié
NKNOWN_MODELS=~/.local/share/nknown/models \
  cargo test --release -p nknown-service --test expansion_levels expansion_levels_dev -- --nocapture
NKNOWN_MODELS=~/.local/share/nknown/models \
  cargo test --release -p nknown-service --test expansion_levels expansion_levels_heldout -- --ignored --nocapture
NKNOWN_BASE=/tmp/opencode/base-audit.redb \
  cargo test --release -p nknown-service --test expansion_levels expansion_levels_real_corpus -- --ignored --nocapture

# CLI de référence (source courante)
./target/debug/nknown search /tmp/opencode/base-audit.redb \
  ~/.local/share/nknown/models "php flaw" 50 --explain
./target/debug/nknown search /tmp/opencode/base-audit.redb \
  ~/.local/share/nknown/models "php flaw" 50 --variety=diverse --explain
./target/debug/nknown search /tmp/opencode/base-audit.redb \
  ~/.local/share/nknown/models "php vulnerability" 50 --explain
```

La copie de base `/tmp/opencode/base-audit.redb` est une copie de la base utilisateur :
ne jamais écrire dans `~/.local/share/nknown/base.redb` pendant les essais, et ne jamais
utiliser les données Flatpak (`~/.var/app/...`).
