#!/usr/bin/env bash
# NKnown : installeur universel (toute distribution Linux x86_64, glibc ≥ 2.31).
#
# En une ligne, dernière version :
#   curl -fsSL https://raw.githubusercontent.com/fnyaker/nknown/main/packaging/install.sh | bash
# Avec des options :
#   curl -fsSL …/install.sh | bash -s -- --system
#
# Depuis l'archive portable (tar.gz ou zip) : ./install.sh, qui installe le binaire voisin.
#
# Options :
#   --system       pour tous les utilisateurs (/usr/local, demande sudo) ; défaut : ~/.local
#   --no-service   ne pas activer le démon (service systemd utilisateur)
#   --version V    une version précise (ex. v0.1.0) ; défaut : la dernière
#   --uninstall    désinstaller (les données et la configuration restent)
#
# Rien n'est écrit dans /etc. Les données vont dans ~/.local/share/nknown, la configuration dans
# ~/.config/nknown ; les modèles (291 Mo) sont téléchargés par le démon à son premier lancement.
set -euo pipefail

REPO=${NKNOWN_REPO:-fnyaker/nknown}
ASSET=nknown-linux-x86_64.tar.gz
# Remplace l'adresse des releases (miroir, tests) : <url>/<asset> et <url>/SHA256SUMS.
RELEASE_URL=${NKNOWN_RELEASE_URL:-}

system=0 service=1 uninstall=0 version=latest
while [ $# -gt 0 ]; do
  case $1 in
    --system) system=1 ;;
    --no-service) service=0 ;;
    --uninstall) uninstall=1 ;;
    --version) version=${2:?--version demande une valeur}; shift ;;
    -h|--help) sed -n '2,22p' "${BASH_SOURCE[0]:-/dev/null}" 2>/dev/null | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "option inconnue : $1" >&2; exit 2 ;;
  esac
  shift
done

say() { printf '\033[1;36m▸\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m!\033[0m %s\n' "$*" >&2; }
die() { printf '\033[1;31m✗\033[0m %s\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = Linux ] || die "NKnown s'installe sur Linux uniquement."
case $(uname -m) in
  x86_64|amd64) ;;
  *) die "architecture $(uname -m) non prise en charge pour l'instant (x86_64 seulement)." ;;
esac

if [ $system = 1 ]; then
  prefix=/usr/local
  unit_dir=/usr/local/lib/systemd/user
  if [ "$(id -u)" = 0 ]; then sudo=; else
    command -v sudo >/dev/null || die "--system demande sudo (ou les droits root)."
    sudo=sudo
  fi
else
  prefix=$HOME/.local
  unit_dir=${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user
  sudo=
fi
bin_dir=$prefix/bin
has_systemd() { command -v systemctl >/dev/null && systemctl --user show-environment >/dev/null 2>&1; }

# ------------------------------------------------------------------ désinstallation
if [ $uninstall = 1 ]; then
  if has_systemd; then
    systemctl --user disable --now nknownd.service 2>/dev/null || true
    [ $system = 1 ] && $sudo systemctl --global disable nknownd.service 2>/dev/null || true
  fi
  $sudo rm -f "$bin_dir/nknown" "$bin_dir/nknownd" "$unit_dir/nknownd.service"
  has_systemd && systemctl --user daemon-reload || true
  say "NKnown désinstallé."
  say "Données et configuration conservées : ~/.local/share/nknown, ~/.config/nknown"
  exit 0
fi

# ------------------------------------------------------------------ binaire à installer
here=$(cd "$(dirname "${BASH_SOURCE[0]:-.}")" 2>/dev/null && pwd || echo .)
if [ -x "$here/bin/nknown" ]; then
  src=$here   # archive portable : le binaire est à côté du script
else
  fetch() {
    if command -v curl >/dev/null; then curl -fL --retry 3 --progress-bar -o "$2" "$1"
    elif command -v wget >/dev/null; then wget -q --show-progress -O "$2" "$1"
    else die "curl ou wget est nécessaire."; fi
  }
  if [ -n "$RELEASE_URL" ]; then base=$RELEASE_URL
  elif [ "$version" = latest ]; then base=https://github.com/$REPO/releases/latest/download
  else base=https://github.com/$REPO/releases/download/$version
  fi
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT
  say "Téléchargement de NKnown ($version)…"
  fetch "$base/$ASSET" "$tmp/$ASSET"
  fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS"
  expected=$(grep " \*\{0,1\}$ASSET\$" "$tmp/SHA256SUMS" | cut -d' ' -f1)
  [ -n "$expected" ] || die "SHA256SUMS ne mentionne pas $ASSET."
  actual=$(sha256sum "$tmp/$ASSET" | cut -d' ' -f1)
  [ "$expected" = "$actual" ] || die "archive corrompue (SHA-256 inattendu) : installation annulée."
  tar -xzf "$tmp/$ASSET" -C "$tmp"
  src=$tmp/nknown-linux-x86_64
  [ -x "$src/bin/nknown" ] || die "archive inattendue : bin/nknown absent."
fi

# ------------------------------------------------------------------ installation
say "Installation dans $bin_dir"
$sudo install -Dm755 "$src/bin/nknown" "$bin_dir/nknown"
$sudo ln -sf nknown "$bin_dir/nknownd"
$sudo mkdir -p "$unit_dir"
sed "s|^ExecStart=.*|ExecStart=$bin_dir/nknownd|" "$src/share/systemd/user/nknownd.service" \
  | $sudo tee "$unit_dir/nknownd.service" >/dev/null
$sudo chmod 644 "$unit_dir/nknownd.service"

if [ $service = 1 ]; then
  if has_systemd; then
    [ $system = 1 ] && $sudo systemctl --global enable nknownd.service >/dev/null 2>&1 || true
    systemctl --user daemon-reload
    systemctl --user enable --now nknownd.service >/dev/null 2>&1 \
      && say "Démon démarré (systemctl --user status nknownd) ; il télécharge les modèles (291 Mo)." \
      || warn "le démon n'a pas pu démarrer : journalctl --user -u nknownd"
  else
    warn "systemd utilisateur absent : lancez le démon vous-même (nknownd &)."
  fi
fi

case ":$PATH:" in
  *":$bin_dir:"*) ;;
  *) warn "$bin_dir n'est pas dans le PATH : ajoutez-le (export PATH=\"$bin_dir:\$PATH\")." ;;
esac
say "$("$bin_dir/nknown" --version 2>/dev/null || echo NKnown) installé. Désinstaller : install.sh --uninstall"
