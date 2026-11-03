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
éléments actifs ; un élément inactif ou hors sélection descend à 25 % d'opacité, il ne change
pas de couleur. La lueur (`glow`) ne sert qu'au focus et à la progression.

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

| Lien | Trait | Épaisseur / intensité code… |
|---|---|---|
| Requête → mot | plein, fin | — (structure) |
| Mot → résultat | plein, dégradé couleur du mot → couleur du résultat | **part du mot dans le score du résultat** (épaisseur 0,5–4 px, opacité 30–100 %) |
| Qualifiant → mot | arc court `node.qualifier` | présent dans le résultat ou non |
| Résultat → source | pointillé fin `node.source` | — |
| Résultat ↔ apparenté | tirets | similarité |

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

**Accueil.** Seul le champ de recherche, centré, avec le logo au-dessus. En bas, un point
d'état discret (vert : à jour ; animé : collecte en cours ; rouge : moteur injoignable).

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
(`motion.spring`), le nœud et les chemins qui y mènent s'allument, le reste passe à 25 %.
Cliquer un nœud ouvre l'**inspecteur** par-dessus la liste : ce qu'il est (forme et couleur
reprises en en-tête), ses liens triés par poids (barres), ses actions (ouvrir, chercher ce
mot). `Échap` referme, puis désélectionne, puis revient à l'accueil.

**Interactions de carte.** Survol : voisinage allumé. Glisser : déplace la carte ou un nœud
(qui revient doucement). Molette : zoom centré sur le pointeur. Double-clic : recentrer.

## 7. Composants

- **Champ de recherche** : sans bordure, trait inférieur seulement ; l'intention reconnue
  (`actu`, `code`…) devient une pastille colorée dans le champ.
- **Carte de résultat** : icône de nature, titre, source et date relative, extrait de deux
  lignes ; à gauche, une barre verticale dont la hauteur est le score relatif au meilleur.
- **Pastille** : 22 px de haut, rayon complet, fond à 12 % de sa couleur.
- **Infobulle** : apparaît après 400 ms, une ligne, jamais d'information indispensable.
- Coins : 10 px (cartes), 14 px (panneaux), complet (pastilles, champ).
- Grille de 4 px ; marges de 24 px dans les panneaux.

## 8. Accessibilité

Contraste AA minimum pour tout texte ; une nature ne se distingue jamais par la couleur seule
(la forme la porte aussi) ; tout est atteignable au clavier (`Tab` parcourt les résultats, les
flèches parcourent les voisins d'un nœud dans la carte) ; libellés accessibles sur chaque
icône.

## 9. Avant d'ajouter quoi que ce soit

- Que dit cet élément, et à qui ? S'il ne répond à aucune question de l'utilisateur, il part.
- Peut-il être un signe (forme, taille, couleur déjà définie) plutôt qu'un texte ?
- Réutilise-t-il un jeton existant ? Un nouveau jeton se documente ici d'abord.
- D'où vient-il et où va-t-il quand la vue change ?
