# NKnown

Base de connaissances en Rust. Elle collecte des sources (RSS, fichiers, sites, Wikipedia),
analyse les textes dans toutes les langues et relie mots, sens, passages et documents
dans un graphe pondéré interrogeable (`actu paris`, ...).

## Crates

| Crate | Rôle |
|---|---|
| `nknown-core` | Types et traits partagés : `Source`, `RawItem`, `Analyzer`, `Lang`, `NatureProfile`, dates |
| `nknown-sources` | Implémentations de sources (fichiers locaux ; RSS, Wikipedia à venir) |
| `nknown-nlp` | Pipeline NLP : lexique (Wiktextract + Lexique → `fst`), tagueur perceptron, rattachement des adjectifs ; neuronal (candle) à venir |
| `nknown-engine` | Moteur : graphe, index, scoring, au-dessus d'un `Storage` abstrait |
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
