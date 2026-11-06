# Charte d'interface NKnown

Valable pour l'app native (Qt6/QML) et l'interface web (Leptos). Les deux partagent les mêmes
jetons (`design/tokens.toml`), les mêmes formes et le même mouvement. Toute nouvelle vue se
justifie au regard de ces règles ; en cas de doute, on retire plutôt qu'on ajoute.

## 1. Principes

1. **Le graphe est l'interface.** Le résultat d'une recherche n'est pas une liste : c'est la
   carte des chemins qui y ont mené. La liste n'est qu'une seconde lecture de la carte.
2. **Montrer, pas écrire.** Une grandeur se lit par une taille, une épaisseur, une intensité,
   jamais par un nombre seul. Pas de libellé quand une icône et une infobulle suffisent. Le
   texte affiché est du contenu (titres, extraits), presque jamais de l'interface.
3. **Chaque signe a un sens.** Couleur, forme, taille, épaisseur, mouvement : chacun code une
   seule chose, toujours la même, dans toutes les vues. Rien de décoratif qui puisse être pris
   pour une information.
4. **Calme par défaut, vivant au contact.** À l'arrêt, l'écran est sobre. Le survol, la
   sélection et la recherche en cours l'animent ; l'animation s'éteint quand l'action finit.
5. **Continuité.** On ne change pas d'écran, on transforme celui qui est là : chaque élément
   va de son ancienne place à la nouvelle. Jamais de coupure sèche, jamais de chargement blanc.
6. **Rapide d'abord.** Une animation ne retarde jamais une information : le contenu est
   utilisable dès qu'il est là, l'animation l'accompagne.

## 2. Couleur

Sombre par défaut (encre profonde, pas de noir pur), thème clair dérivé. Une seule teinte par
**nature** de nœud ; l'accent de l'interface est celui de la requête.

| Jeton | Sombre | Clair | Usage |
|---|---|---|---|
| `bg.base` | `#0A0D13` | `#F4F6FA` | fond |
| `bg.raised` | `#11151E` | `#FFFFFF` | panneaux, cartes |
| `bg.overlay` | `#171C28` E6 | `#FFFFFF` F0 | inspecteur, menus (flou d'arrière-plan 24 px) |
| `line.subtle` | `#FFFFFF` 0F | `#0A0D13` 14 | séparateurs, bords |
| `text.primary` | `#E8ECF4` | `#121722` | titres |
| `text.secondary` | `#8D96A8` | `#5A6375` | extraits, métadonnées |
| `accent` / `node.query` | `#7CF3FF` | `#0091A8` | requête, focus, progression |
| `node.term` | `#6AA8FF` | `#2D6BE0` | mot de la requête |
| `node.qualifier` | `#B58CFF` | `#7A4DE0` | adjectif qualifiant un mot |
| `node.prose` | `#FFC46B` | `#C98200` | résultat : texte |
| `node.code` | `#5CF0A6` | `#0E9F63` | résultat : code |
| `node.source` | `#8D96A8` | `#7A8394` | source (fichier, flux, wiki) |
| `danger` | `#FF6B81` | `#D63352` | erreurs uniquement |

Règles : jamais plus de 5 teintes de nœuds à l'écran ; la saturation est réservée aux
éléments actifs ; un élément inactif ou hors sélection voit son opacité multipliée par 0,25,
il ne change pas de couleur. Le contact est exempté de cette réduction. La lueur (`glow`) ne
sert qu'au focus et à la progression.

## 3. Typographie

| Rôle | Police | Taille / graisse |
|---|---|---|
| Champ de recherche | Inter (repli : police système) | 28 px / 300 au repos, 18 px / 400 en haut |
| Titre de résultat | Inter | 15 px / 600 |
| Extrait | Inter | 13 px / 400, interligne 1,45 |
| Métadonnée | Inter | 11 px / 500, chiffres tabulaires |
| Code | JetBrains Mono (repli : monospace système) | 12,5 px |

Mots de la requête surlignés dans les extraits : couleur `node.term`, pas de gras, pas de fond.

## 4. Grammaire du graphe

### Nœuds

| Nature | Forme | Taille code… | Détail |
|---|---|---|---|
| Requête | anneau creux | fixe (le centre) | pulse pendant la recherche |
| Mot de la requête | disque | poids du mot dans les résultats (sa contribution moyenne aux scores, rareté comprise) : le mot qui a mené aux résultats est le plus gros | |
| Qualifiant (adjectif) | losange | fixe, petit | collé à son nom |
| Résultat texte | carré arrondi | score (aire ∝ score) | |
| Résultat code | carré arrondi + glyphe `</>` | score | |
| Source | petit cercle creux + icône (dossier, flux, globe) | nombre de résultats issus de la source | |
| Document apparenté | carré arrondi en contour seul | similarité | « la même information ailleurs » |

Rayon : de 6 px (minimum lisible) à 28 px ; l'échelle est en racine (aire ∝ valeur).

### Liens

| Lien | Trait | Épaisseur / intensité |
|---|---|---|
| Requête → mot | plein, fin | 1,2 px, 70 % (structure) |
| Mot → résultat | plein, dégradé couleur du mot → couleur du résultat | **0,5 + 3,5 × poids px, opacité 30 + 70 × poids %** |
| Qualifiant → mot | arc court `node.qualifier` | **0,5 + 3,5 × poids px, opacité 30 + 70 × poids %** |
| Résultat → source | pointillé fin `node.source` | 1 px, 30 % (structure) |
| Résultat ↔ apparenté | tirets | **0,5 + 3,5 × poids px, opacité 30 + 70 × poids %** |

Le poids est dans `[0, 1]`. L'épaisseur ne change **jamais** au survol ni à la sélection : seules
l'opacité et le détourage changent. Les liens mot → résultat sous 0,12 ne sont pas tracés.

Au contact, l'opacité augmente de 0,30 (plafond 100 %) et le lien exact est détouré par 3 px
de `bg.base` ; son épaisseur reste celle de son poids. Un nœud survolé allume tous ses liens et
ses voisins directs. La sélection multiplie l'opacité des éléments hors sélection par 0,25 ; un
élément au contact est exempté de cette réduction.

Le sens de lecture est requête → résultats : pendant la recherche et au survol, des particules
glissent le long des liens dans ce sens, d'autant plus vite que le lien pèse lourd. Hors
interaction, les liens sont immobiles.

### Disposition

Radiale et stable : requête au centre, mots sur un premier anneau, résultats sur un second, à
une distance qui décroît avec le score (le meilleur est le plus proche), sources en périphérie.
Un ressort léger évite les chevauchements. Même requête, même carte : la disposition est
déterministe.

## 5. Mouvement

| Jeton | Durée | Courbe | Usage |
|---|---|---|---|
| `motion.quick` | 120 ms | `OutCubic` | survol, pression |
| `motion.standard` | 240 ms | `OutCubic` | panneaux, sélection |
| `motion.emphasis` | 420 ms | `OutExpo` | changement de vue (le champ qui monte) |
| `motion.spring` | ressort amorti (raideur 180, amortissement 22) | | nœuds du graphe, caméra |

- Entrées décalées de 30 ms entre éléments d'une même famille, plafonné à 300 ms au total.
- Sorties deux fois plus rapides que les entrées.
- `prefers-reduced-motion` / animations désactivées dans Plasma : fondus de 120 ms seulement.

## 6. Vues et transitions

**Accueil.** Seul le champ de recherche, centré, avec le logo au-dessus ; sous son trait, le
curseur de variété, discret. En bas, un point d'état discret (vert : à jour ; animé : collecte
en cours ; rouge : moteur injoignable).

**Recherche lancée.** Le champ glisse vers le haut (`motion.emphasis`) et rétrécit ; la zone
libérée accueille la carte (≈ 62 % de la largeur) et la liste des résultats (≈ 38 %). La
progression se voit à deux endroits, sans jamais de barre de chargement :
1. le trait sous le champ se remplit par étapes (analyse, lectures, candidats, scores) ;
2. la carte se construit dans le même ordre, au fil des patchs du flux (`graph_patch`) : les
   mots apparaissent dès l'analyse, puis les résultats provisoires éclosent et leurs liens se
   tendent, puis la carte finale ajuste les tailles, ajoute sources et apparentés, et fait
   s'effacer (sortie 2× plus rapide) les candidats écartés. Chaque patch s'anime dès réception,
   avec les entrées décalées de `motion` ; la recherche locale est souvent plus rapide que
   l'animation : on ne la ralentit pas, on enchaîne.

**Sélection.** Cliquer un résultat le met en valeur dans la carte : la caméra s'y rend
(`motion.spring`), le nœud et les chemins qui y mènent s'allument, le reste voit son opacité
multipliée par 0,25 (le contact est exempté).
Cliquer un nœud ouvre l'**inspecteur** par-dessus la liste : ce qu'il est (forme et couleur
reprises en en-tête), ses liens triés par poids (barres), ses actions (ouvrir, chercher ce
mot). `Échap` referme, puis désélectionne, puis revient à l'accueil.

**Interactions de carte.** Survol : voisinage allumé. Glisser : déplace la carte ou un nœud
(qui revient doucement). Molette : zoom centré sur le pointeur. Double-clic : recentrer.

Le **contact** est unique : le pointeur l'emporte sur le focus clavier, qui reprend quand le
pointeur sort. Un lien survolé s'allume seul et nomme ses extrémités ; un nœud survolé allume
ses liens et ses voisins. Une carte de résultat, une ligne de lien ou une occurrence du passage
produit le même contact que le nœud ou le lien correspondant, dans les deux sens. Le survol ne
déplace jamais la caméra ni le contenu.

## 7. Composants

- **Champ de recherche** : sans bordure, trait inférieur seulement ; l'intention reconnue
  (`actu`, `code`…) devient une pastille colorée dans le champ.
- **Curseur de variété** : trois positions (Strict, Normal, Diversifié), une piste de 72 px,
  trois repères de 4 px et un bouton de 12 px à la couleur d'accent ; à droite, le libellé de
  la position courante (11 px, largeur fixe 64 px). Sous le trait du champ, centré, à
  l'accueil comme en recherche ; ni fond ni panneau. Le bouton glisse en `motion.quick` ;
  `Normal` par défaut, le choix est mémorisé. `Tab` l'atteint, les flèches et `Origine`/`Fin`
  le règlent ; dans l'app, le clic ne prend pas le focus, le champ reste prioritaire.
  Infobulle avec l'explication du niveau ; le rôle annoncé est celui d'un curseur, valeur
  lue en toutes lettres.
- **Carte de résultat** : icône de nature, titre, source et date relative, extrait de deux
  lignes ; à gauche, une barre verticale dont la hauteur est le score relatif au meilleur.
- **Carte de passage** (inspecteur d'un résultat) : la ligne source · date, puis le passage
  trouvé (`passage_text`, repli sur l'extrait) sur fond `bg.base`, 13 px, interligne 1,45 ;
  hauteur 96–260 px, défilement vertical avec barre fine ; ouverture sur la première
  correspondance. Mots de la requête en `node.term`, sans gras ni fond ; au contact, les
  occurrences liées sont soulignées d'un trait de 1 px, sur le mot seul (pas la ponctuation).
  La prose normalise les espaces, le code garde ses espaces et retours (`pre-wrap`,
  monospace 12,5 px côté web, 12 px dans l'app car QML attend un pixel entier). Le titre reste
  dans l'en-tête de l'inspecteur. Le survol par occurrence est réservé à la prose ; dans le
  code, le couplage passe par les lignes de lien. Le code s'ouvre en tête : le repérage de la
  première correspondance est réservé à la prose.
- **Pastille** : 22 px de haut, rayon complet, fond à 12 % de sa couleur.
- **Infobulle** : apparaît après 400 ms, une ligne, jamais d'information indispensable.
- Coins : 10 px (cartes), 14 px (panneaux), complet (pastilles, champ).
- Grille de 4 px ; marges de 24 px dans les panneaux.

## 8. Accessibilité

Contraste AA minimum pour tout texte ; une nature ne se distingue jamais par la couleur seule
(la forme la porte aussi) ; tout est atteignable au clavier (`Tab` parcourt les résultats, les
flèches parcourent les voisins d'un nœud dans la carte) ; libellés accessibles sur chaque
icône. Le curseur de variété est un seul arrêt de tabulation, réglable aux flèches et à
`Origine`/`Fin`, annoncé avec sa valeur (« Variété de recherche : Normal »).
Les cartes de résultat et les lignes de lien sont focusables ; le focus et le survol produisent
le même contact, et `Entrée` ouvre. `Échap` conserve son ordre : inspecteur, sélection, accueil.
Quand l'inspecteur est ouvert, la liste des résultats est inerte (`inert`), masquée pour les
lecteurs d'écran (`aria-hidden`) et retirée du parcours `Tab`, même si son fondu reste visible.

## 9. Avant d'ajouter quoi que ce soit

- Que dit cet élément, et à qui ? S'il ne répond à aucune question de l'utilisateur, il part.
- Peut-il être un signe (forme, taille, couleur déjà définie) plutôt qu'un texte ?
- Réutilise-t-il un jeton existant ? Un nouveau jeton se documente ici d'abord.
- D'où vient-il et où va-t-il quand la vue change ?
