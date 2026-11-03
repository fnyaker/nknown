#!/usr/bin/env bash
# Construit l'interface web dans crates/nknown-web/dist (servie par nknown-server : NKNOWN_WEB).
# Pas de trunk : il injecte un script en ligne, que la politique de sécurité (CSP) du serveur
# refuse. Ici : cargo (wasm32) → wasm-bindgen → wasm-opt, plus les fichiers statiques.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
out=${1:-$here/dist}
profile=${PROFILE:-release}
(cd "$root" && cargo build -p nknown-web --lib --target wasm32-unknown-unknown --profile "$profile")
rm -rf "$out" && mkdir -p "$out"
wasm-bindgen --target web --no-typescript --out-dir "$out" --out-name nknown \
  "$root/target/wasm32-unknown-unknown/$profile/nknown_web.wasm"
if command -v wasm-opt >/dev/null; then
  wasm-opt -Oz --enable-bulk-memory --enable-nontrapping-float-to-int \
    -o "$out/nknown_bg.wasm" "$out/nknown_bg.wasm"
fi
cp "$here"/static/* "$out/"
cp "$root/packaging/share/nknown.svg" "$out/icon.svg"
echo "interface web : $out ($(du -sh "$out" | cut -f1))"
