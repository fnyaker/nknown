# NKnown

Base de connaissances en Rust. Elle collecte des sources (RSS, fichiers, sites, Wikipedia),
analyse les textes dans toutes les langues et relie mots, sens, passages et documents
dans un graphe pondéré interrogeable (`actu paris`, ...).

## Crates

| Crate | Rôle |
|---|---|
| `nknown-core` | Types et traits partagés : `Source`, `RawItem`, `Analyzer`, `Lang`, `NatureProfile`, dates |
| `nknown-sources` | Sources : fichiers locaux, flux RSS/Atom/JSON Feed, Wikipédia (titres, recherche, catégorie) |
| `nknown-nlp` | Pipeline NLP : lexique (Wiktextract + Lexique → `fst`), tagueur perceptron, rattachement des adjectifs ; neuronal (candle) à venir |
| `nknown-engine` | Moteur : graphe (lemmes, passages, documents, qualifications), index BM25, intentions (`actu`, `info`…), stockage redb |
| `nknown-cli` | Binaire `nknown` |

## Utilisation

```sh
# Lexique français : Wiktextract (couverture) + Lexique 3.83 (fréquences)
#   https://kaikki.org/dictionary/French/kaikki.org-dictionary-French.jsonl
#   http://www.lexique.org/databases/Lexique383/Lexique383.tsv
nknown lexicon build fr.lex \
    wiktextract kaikki.org-dictionary-French.jsonl fr \
    lexique Lexique383.tsv
nknown lexicon lookup fr.lex été "l'" aujourd'hui

# Tagueur (couche 2) entraîné sur UD French-GSD (CC BY-SA 4.0)
#   https://github.com/UniversalDependencies/UD_French-GSD
nknown tagger train fr.lex fr_gsd-ud-train.conllu fr.tag
nknown tagger eval fr.lex fr_gsd-ud-test.conllu fr.tag
nknown analyze fr.lex "Les rues calmes de Paris." fr.tag

nknown ingest <dossier> [fr.lex]
```

Le Wiktionary (CC BY-SA 3.0/4.0) et Lexique 3.83 (CC BY-SA 4.0) sont sous licence à partage
à l'identique : un lexique compilé en hérite.

## Base de connaissances

```sh
nknown index base.redb data/fr.lex @examples/sources.txt data/fr.tag   # ou data/parser-fr (couche 3)
nknown index base.redb data/fr.lex rss:https://www.lemonde.fr/rss/une.xml data/fr.tag
nknown index base.redb data/fr.lex "wiki:fr:Paris|Lyon" data/fr.tag
nknown index base.redb data/fr.lex <dossier> data/fr.tag
nknown search base.redb data/fr.lex "actu paris"
nknown search base.redb data/fr.lex "paris sportifs"         # bonus si « sportifs » qualifie « paris »
```

Sources : `<dossier>`, `rss:<url>`, `wiki:<édition>:<Titre>|<Titre>`, `wiki-search:<édition>:<requête>`,
`wiki-cat:<édition>:<Catégorie>`, `@<fichier>` (une par ligne). Les requêtes HTTP sont espacées
(1/s pour Wikipédia) et reprises sur 429/503 ; `NKNOWN_USER_AGENT` permet d'y ajouter un contact.
La collecte est incrémentale : un contenu inchangé n'est ni retéléchargé (Wikipédia : dates de
révision par lots de 50) ni réanalysé ; une version modifiée remplace l'ancienne.
`nknown watch base.redb data/fr.lex @examples/sources.txt 30m data/fr.tag` relance la collecte
toutes les 30 minutes.

Intentions reconnues dans la requête : `actu` (événements récents), `info` (référence),
`avis`, `tuto`, `perso`. Un mot ambigu (« paris ») est cherché sous toutes ses lectures,
pondérées par leur usage dans la base.

## Mesures (UD French-GSD, test)

| | Couche 1 seule | + tagueur (couche 2) | + analyseur neuronal (couche 3) |
|---|---|---|---|
| Catégories (UPOS) | 81,0 % (dev) | 97,6 % | **98,3 %** |
| Adjectifs `amod` : rappel / précision | 67,4 % / 86,8 % (dev) | 83,7 % / 87,3 % | **94,6 % / 94,4 %** |
| Dépendances UAS / LAS | — | — | **95,1 % / 92,4 %** |

Couche 3 : XLM-RoBERTa base affiné 3 passes sur French-GSD, ~35 min sur Radeon 780M (Vulkan).
Résultats identiques avec le moteur CPU (candle) et le moteur GPU (burn/Vulkan).

```sh
# entraînement sur GPU (ou sans « vulkan » : sur CPU, ~2× plus lent)
nknown parser train vulkan xlm-roberta-base/ fr_gsd-ud-train.conllu fr_gsd-ud-dev.conllu parser-fr/ 3
nknown eval fr.lex fr_gsd-ud-test.conllu vulkan:parser-fr/
nknown analyze fr.lex "Les rues calmes de Paris." parser-fr/
```
