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
cargo run -p nknown-cli -- ingest <dossier>
```
