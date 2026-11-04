#!/usr/bin/env bash
# Prépare une release de modèles : copie les fichiers sous des noms plats (assets GitHub) et
# écrit le manifeste lu par nknown-service (crates/nknown-service/models.toml).
#
#   packaging/models/stage.sh <dossier-des-modèles> <tag> <sortie>
#   gh release create <tag> <sortie>/* --title "Modèles <tag>" --notes-file packaging/models/NOTICE.md
set -euo pipefail
src=${1:?dossier des modèles}; tag=${2:?tag de release, ex. models-v1}; out=${3:?dossier de sortie}
repo=${NKNOWN_REPO:-fnyaker/nknown}
manifest=$(dirname "$0")/../../crates/nknown-service/models.toml
mkdir -p "$out"

# chemin local | groupe (base : toujours ; standard / light : analyseur selon la précision)
#   [| nom d'asset, pour un fichier dont le format a changé : les anciennes versions gardent
#   l'ancien asset, les nouvelles téléchargent celui-ci (taille différente)]
files=(
  "fr.lex|base" "fr.tag|base" "en.lex|base" "en.tag|base"
  "ru.lex|base" "ru.tag|base" "zh.lex|base" "zh.tag|base"
  "fr.rel|base|fr.v3.rel" "en.rel|base|en.v3.rel" "ru.rel|base|ru.v3.rel" "zh.rel|base|zh.v3.rel"
  "parser-multi/parser.json|parser" "parser-multi/tokenizer.nkt|parser"
  "parser-multi/model.q4k-q8.gguf|standard" "parser-multi/model.q4k.gguf|light"
)
# Traduction (OPUS-MT, voir NOTICE.md) : téléchargée par paire, quand une langue est acceptée.
for pair in en-fr de-fr es-fr it-fr nl-fr ru-fr zh-en; do
  for f in config.json model.safetensors source.spm vocab.json; do
    files+=("translate/$pair/$f|translate||$pair")
  done
done
{
  echo "# Généré par packaging/models/stage.sh : ne pas modifier à la main."
  echo "release = \"https://github.com/$repo/releases/download/$tag\""
  for entry in "${files[@]}"; do
    IFS='|' read -r path group asset pair <<< "$entry"
    asset=${asset:-${path//\//--}}
    cp -L "$src/$path" "$out/$asset"
    size=$(stat -c %s "$out/$asset")
    sum=$(sha256sum "$out/$asset" | cut -d' ' -f1)
    printf '\n[[file]]\npath = "%s"\nasset = "%s"\ngroup = "%s"\n' "$path" "$asset" "$group"
    [ -n "$pair" ] && printf 'pair = "%s"\n' "$pair"
    printf 'size = %s\nsha256 = "%s"\n' "$size" "$sum"
  done
} > "$manifest"
cp "$(dirname "$0")/NOTICE.md" "$out/NOTICE.md"
echo "manifeste : $manifest ; assets : $out"
