# Architecture de distribution

NKnown se distribue sous trois formes, toutes construites sur le même cœur
(`nknown-engine` + `nknown-nlp`) et parlant la même API.

```
                  ┌─────────────────────────── poste de l'utilisateur ───────────────────────────┐
                  │                                                                              │
 KRunner ─D-Bus─▶ │  nknown (app Qt6/QML)  ──REST/Unix──▶  nknownd (démon, systemd --user)       │
 Plasma tray ◀──  │        │  backend                        ├─ nknown-service (cœur embarqué)   │
                  │        │  au choix :                     ├─ surveillance des fichiers        │
                  │        │   • démon local (défaut)        └─ ~/.local/share/nknown/           │
                  │        │   • cœur dans le processus (démon désactivé)                         │
                  │        │   • serveur distant ───────────────────────────────┐                 │
                  └────────┼───────────────────────────────────────────────────┼─────────────────┘
                           │                                                   │ HTTPS + jeton
                           ▼                                                   ▼
                                                     nknown-server (Docker)
                                                       ├─ /api/v1  (REST, même API)
                                                       ├─ /        (interface web, Rust → WASM)
                                                       └─ nknown-service
```

## Crates

| Crate | Rôle | Dépend du cœur |
|---|---|---|
| `nknown-api` | Types de l'API (requêtes, réponses, graphe, évènements de progression). Compile en WASM. | non |
| `nknown-graph` | Disposition du graphe de résultats (forces, anneaux), partagée app / web. Compile en WASM. | non |
| `nknown-service` | Façade du cœur : chargement des modèles, requêtes, construction du graphe, collecte, surveillance de dossiers. | oui |
| `nknown-server` | Serveur HTTP (axum). Binaire `nknown-server` (distant, TCP, jeton, interface web) et `nknownd` (local, socket Unix). | oui |
| `nknown-desktop` | App native Qt6/QML (cxx-qt), intégration Plasma. | oui (mode embarqué) |
| `nknown-web` | Interface web (Leptos, CSR), servie par `nknown-server`. | non |

L'app et l'interface web ne connaissent que `nknown-api` : un même écran fonctionne face au
démon local, au cœur embarqué ou à un serveur distant.

## Emplacements (XDG, jamais `/etc`)

| Quoi | Où |
|---|---|
| Base, modèles, lexiques | `$XDG_DATA_HOME/nknown/` (défaut `~/.local/share/nknown/`) : `base.redb`, `models/` |
| Configuration | `$XDG_CONFIG_HOME/nknown/config.toml` |
| Socket du démon | `$XDG_RUNTIME_DIR/nknown/nknownd.sock` (dossier 0700) |
| Jeton d'un serveur distant | Secret Service (KWallet), jamais dans un fichier en clair |
| Service utilisateur | `~/.config/systemd/user/nknownd.service`, activé par défaut, désactivable depuis l'app |

En conteneur, `NKNOWN_DATA` (défaut `/data`, volume) remplace `$XDG_DATA_HOME/nknown`.

## API REST (`/api/v1`)

Règle : **aucune donnée sensible dans une URL**. Le texte d'une requête, un chemin de fichier,
un jeton ne passent jamais par le chemin ou la chaîne de requête (journaux, historique,
proxys). Ils vont dans le corps (`POST`) ou les en-têtes. Les URL ne portent que des
identifiants opaques.

| Méthode | Chemin | Corps / réponse |
|---|---|---|
| `GET` | `/status` | version, statistiques de la base, collectes en cours |
| `POST` | `/search` | `{query, limit}` → flux NDJSON d'évènements de progression puis le résultat ; `Accept: application/json` pour le résultat seul |
| `GET` | `/documents/{id}` | document et ses passages |
| `POST` | `/documents` | ajout direct d'un texte (servira au serveur MCP) |
| `GET` | `/sources` | sources configurées et leur état |
| `POST` | `/sources` | ajout d'une source (`{spec}` : dossier, `rss:`, `wiki:`…) |
| `DELETE` | `/sources/{id}` | retrait d'une source |
| `POST` | `/session` | interface web : échange le jeton contre un cookie `HttpOnly; Secure; SameSite=Strict` |

Flux d'une recherche (`POST /search`, une ligne JSON par évènement) :

```
{"type":"phase","phase":"analyze"}
{"type":"terms","intent":null,"terms":[…]}
{"type":"graph_patch","nodes":[requête, mots],"edges":[…]}
{"type":"phase","phase":"search"}
{"type":"graph_patch","nodes":[résultats provisoires, sources],"edges":[…]}
{"type":"phase","phase":"graph"}
{"type":"graph_patch","nodes":[modifiés],"edges":[…],"removed_nodes":[écartés]}
{"type":"done", …résultat complet, dont la carte finale…}
```

La carte se construit en direct : chaque `graph_patch` ajoute ou remplace des nœuds et des liens
(même `id` ; même `from`, `to`, `kind`) et en retire ; les appliquer dans l'ordre redonne la
carte de `done`. Les résultats provisoires sont classés au seul score du texte ; la carte finale
ajoute la propagation dans le graphe, l'autorité, les seuils et les documents apparentés.

Authentification :
- serveur distant : `Authorization: Bearer <jeton>` (comparaison en temps constant), ou le
  cookie de session pour l'interface web ; jeton fourni par `NKNOWN_TOKEN` ou généré au premier
  démarrage dans le volume de données ;
- démon local : socket Unix accessible au seul utilisateur, pas de jeton.

## Plus tard : serveur MCP

Gardé de côté, pas encore implémenté. `nknownd` exposera un serveur MCP (stdio et socket)
pour qu'un modèle puisse :
- lancer des requêtes (`search`, avec le graphe) ;
- ajouter des données (`add_document`, `add_source`) ;
- explorer (`get_document`, `neighbors` d'un nœud, `term` : lectures et liens d'un mot).

Ces outils reprendront un à un les points de l'API REST via `nknown-service` : aucune logique
propre au MCP.
