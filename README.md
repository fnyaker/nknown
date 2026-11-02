# NKnown

Base de connaissances en Rust. Elle collecte des sources (RSS, fichiers, sites, Wikipedia),
analyse les textes dans toutes les langues et relie mots, sens, passages et documents
dans un graphe pondéré interrogeable (`actu paris`, ...).

## Crates

| Crate | Rôle |
|---|---|
| `nknown-core` | Types et traits partagés : `Source`, `RawItem`, `Analyzer`, `Lang`, `NatureProfile`, dates |
| `nknown-sources` | Implémentations de sources (fichiers locaux ; RSS, Wikipedia à venir) |
| `nknown-nlp` | Pipeline NLP en 3 couches : lexique (Wiktextract → `fst`), tagueur, neuronal optionnel (candle) |
| `nknown-engine` | Moteur : graphe, index, scoring, au-dessus d'un `Storage` abstrait |
| `nknown-cli` | Binaire `nknown` |

## Utilisation

```sh
# Lexique français depuis le dump Wiktextract (https://kaikki.org/dictionary/French/)
nknown lexicon build kaikki.org-dictionary-French.jsonl fr fr.lex
nknown lexicon lookup fr.lex livres "l'" aujourd'hui

nknown ingest <dossier> [fr.lex]
```

Les données du Wiktionary sont sous licence CC BY-SA : un lexique compilé en hérite.
