# Modèles NKnown

Téléchargés automatiquement au premier lancement de NKnown (`nknown models fetch` à la main).

| Fichiers | Contenu | Sources et licences |
|---|---|---|
| `fr.lex` | lexique français | Wiktionary via Wiktextract (CC BY-SA 3.0/4.0), Lexique 3.83 (CC BY-SA 4.0) |
| `en.lex`, `ru.lex`, `zh.lex` | lexiques anglais, russe, chinois | Wiktionary via Wiktextract (CC BY-SA 3.0/4.0) |
| `*.rel` | liens de sens entre mots (synonymes, mots apparentés, définitions), pour élargir les recherches | Wiktionnaires de chaque langue via Wiktextract (français pour `fr.rel`, russe et chinois avec le Wiktionary anglais pour `ru.rel` et `zh.rel`), Wiktionary anglais pour `en.rel` (CC BY-SA 3.0/4.0) |
| `translate--*` | traduction vers le français (anglais, allemand, espagnol, italien, néerlandais, russe ; chinois par l'anglais), poids en f16, téléchargés seulement si une traduction est acceptée | OPUS-MT, Helsinki-NLP (Tiedemann et Thottingal, 2020) : `opus-mt-*-fr` sous licence Apache 2.0, `opus-mt-zh-en` sous licence CC BY 4.0 |
| `*.tag` | tagueurs (catégories grammaticales) | entraînés sur Universal Dependencies : French-GSD, English-EWT, Russian-GSD et Taiga, Chinese-GSDSimp (CC BY-SA 4.0) |
| `parser-multi--*` | analyseur syntaxique fr/en/ru/zh, quantifié | encodeur XLM-RoBERTa base (MIT, Facebook AI), affiné sur les mêmes corpus UD (CC BY-SA 4.0) |

Les lexiques et modèles dérivés de ressources CC BY-SA sont distribués sous la même licence
(CC BY-SA 4.0), avec attribution aux projets ci-dessus.
