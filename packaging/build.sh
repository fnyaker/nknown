#!/usr/bin/env bash
# Construit les paquets de NKnown dans dist/ à partir d'un seul binaire.
#
#   packaging/build.sh [format...]     formats : tar zip deb rpm arch appimage flatpak
#
# Sans argument : tous les formats dont l'outil est présent. Le binaire est target/dist/nknown
# (construit au besoin) ou $NKNOWN_BINARY. Pour des binaires portables (glibc ≥ 2.28), la CI
# de release le compile dans Rocky Linux 8 : .github/workflows/release.yml.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
pkg=$root/packaging
dist=$root/dist
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -n1)
name=nknown-linux-x86_64
stage=$dist/stage/$name
binary=${NKNOWN_BINARY:-$root/target/dist/nknown}
maintainer="fnyaker <fnyaker@users.noreply.github.com>"
summary="Base de connaissances personnelle : recherche et carte des liens"
homepage=https://github.com/fnyaker/nknown

say() { printf '\033[1;36m▸\033[0m %s\n' "$*"; }
skip() { printf '\033[1;33m–\033[0m %s : %s\n' "$1" "$2"; }

if [ ! -x "$binary" ]; then
  say "compilation du binaire (profil dist)"
  (cd "$root" && cargo build --profile dist -p nknown-cli --no-default-features --features neural,plasma)
fi

# ------------------------------------------------------------------ arborescence commune
say "NKnown $version : préparation de $stage"
rm -rf "$dist/stage" && mkdir -p "$stage/bin" "$stage/share/systemd/user" "$stage/share/doc"
install -m755 "$binary" "$stage/bin/nknown"
install -m644 "$pkg/systemd/nknownd.service" "$stage/share/systemd/user/nknownd.service"
install -m755 "$pkg/install.sh" "$stage/install.sh"
install -m644 "$root/README.md" "$stage/share/doc/README.md"
install -m644 "$pkg/models/NOTICE.md" "$stage/share/doc/MODELS.md"
install -Dm644 "$pkg/share/nknown.svg" "$stage/share/icons/hicolor/scalable/apps/nknown.svg"
install -Dm644 "$pkg/plasma/nknown-krunner.desktop" "$stage/share/krunner/dbusplugins/nknown.desktop"
install -Dm644 "$pkg/plasma/org.nknown.krunner.service" "$stage/share/dbus-1/services/org.nknown.krunner.service"
cat > "$stage/README.txt" <<EOF
NKnown $version (Linux x86_64)

Installer :            ./install.sh             (~/.local, démon activé)
Pour tous :            ./install.sh --system
Sans installer :       ./bin/nknown daemon      puis  ./bin/nknown --help
Désinstaller :         ./install.sh --uninstall

Les modèles (291 Mo) sont téléchargés au premier lancement du démon.
EOF

# Racine d'un paquet système (/usr) : binaire, lien nknownd, service utilisateur, doc, icône.
system_root() {
  local r=$1
  install -Dm755 "$stage/bin/nknown" "$r/usr/bin/nknown"
  ln -sf nknown "$r/usr/bin/nknownd"
  install -Dm644 "$stage/share/systemd/user/nknownd.service" "$r/usr/lib/systemd/user/nknownd.service"
  install -Dm644 "$stage/share/doc/README.md" "$r/usr/share/doc/nknown/README.md"
  install -Dm644 "$stage/share/doc/MODELS.md" "$r/usr/share/doc/nknown/MODELS.md"
  install -Dm644 "$pkg/share/nknown.svg" "$r/usr/share/icons/hicolor/scalable/apps/nknown.svg"
  install -Dm644 "$pkg/plasma/nknown-krunner.desktop" "$r/usr/share/krunner/dbusplugins/nknown.desktop"
  install -Dm644 "$pkg/plasma/org.nknown.krunner.service" "$r/usr/share/dbus-1/services/org.nknown.krunner.service"
}

build_tar() {
  tar -C "$dist/stage" --owner=0 --group=0 -czf "$dist/$name.tar.gz" "$name"
  say "dist/$name.tar.gz"
}

build_zip() {
  command -v zip >/dev/null || { skip zip "zip absent"; return; }
  rm -f "$dist/$name.zip"
  (cd "$dist/stage" && zip -qr -X "$dist/$name.zip" "$name")
  say "dist/$name.zip"
}

build_deb() {
  command -v dpkg-deb >/dev/null || { skip deb "dpkg-deb absent"; return; }
  local r=$dist/stage/deb
  rm -rf "$r" && system_root "$r" && mkdir -p "$r/DEBIAN"
  cat > "$r/DEBIAN/control" <<EOF
Package: nknown
Version: $version
Architecture: amd64
Maintainer: $maintainer
Depends: libc6 (>= 2.28), libgcc-s1 | libgcc1
Section: utils
Priority: optional
Homepage: $homepage
Installed-Size: $(du -sk "$r/usr" | cut -f1)
Description: $summary
 Indexe vos dossiers, flux et pages, analyse les textes (français, anglais,
 russe, chinois) et relie mots, passages et documents dans un graphe pondéré.
 Le démon (service systemd utilisateur nknownd) est activé pour chaque
 utilisateur ; il télécharge ses modèles au premier lancement.
EOF
  cat > "$r/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e
if [ "$1" = configure ] && command -v systemctl >/dev/null; then
  systemctl --global enable nknownd.service >/dev/null 2>&1 || true
fi
EOF
  cat > "$r/DEBIAN/prerm" <<'EOF'
#!/bin/sh
set -e
if [ "$1" = remove ] && command -v systemctl >/dev/null; then
  systemctl --global disable nknownd.service >/dev/null 2>&1 || true
fi
EOF
  chmod 755 "$r/DEBIAN/postinst" "$r/DEBIAN/prerm"
  dpkg-deb --root-owner-group -Zxz --build "$r" "$dist/nknown_${version}_amd64.deb" >/dev/null
  say "dist/nknown_${version}_amd64.deb"
}

build_rpm() {
  command -v rpmbuild >/dev/null || { skip rpm "rpmbuild absent"; return; }
  local top=$dist/stage/rpm
  rm -rf "$top" && mkdir -p "$top"/{BUILD,RPMS,SOURCES,SPECS,SRPMS,ROOT}
  system_root "$top/ROOT"
  rpmbuild -bb --quiet \
    --define "_topdir $top" --define "stageroot $top/ROOT" \
    --define "version $version" --define "summary $summary" --define "homepage $homepage" \
    "$pkg/rpm/nknown.spec"
  cp "$top"/RPMS/x86_64/nknown-*.rpm "$dist/"
  say "dist/$(basename "$top"/RPMS/x86_64/nknown-*.rpm)"
}

build_arch() {
  command -v makepkg >/dev/null || { skip arch "makepkg absent"; return; }
  [ -f "$dist/$name.tar.gz" ] || build_tar
  local d=$dist/stage/arch
  rm -rf "$d" && mkdir -p "$d"
  cp "$pkg/arch/PKGBUILD" "$pkg/arch/nknown.install" "$d/"
  cp "$dist/$name.tar.gz" "$d/"
  sed -i "s/^pkgver=.*/pkgver=$version/" "$d/PKGBUILD"
  # L'app (Qt6) : construite contre la Qt d'Arch ($NKNOWN_APP ou target/dist/nknown-app) ;
  # sans elle, le paquet ne contient que la ligne de commande et le démon.
  local app=${NKNOWN_APP:-$root/target/dist/nknown-app}
  if [ -x "$app" ]; then
    install -m755 "$app" "$d/nknown-app"
    cp "$pkg/share/nknown.desktop" "$d/nknown.desktop"
  else
    sed -i '/# app$/d' "$d/PKGBUILD"
    skip "arch (app)" "nknown-app absent : paquet sans interface graphique"
  fi
  (cd "$d" && PKGDEST="$dist" makepkg --force --skipchecksums --nodeps >/dev/null)
  say "dist/$(cd "$dist" && ls nknown-bin-"$version"-*.pkg.tar.zst)"
}

build_appimage() {
  local tool=${APPIMAGETOOL:-$(command -v appimagetool || true)}
  [ -n "$tool" ] || { skip appimage "appimagetool absent (APPIMAGETOOL=…)"; return; }
  local d=$dist/stage/NKnown.AppDir
  rm -rf "$d" && system_root "$d"
  install -m755 "$pkg/appimage/AppRun" "$d/AppRun"
  cp "$pkg/appimage/nknown.desktop" "$d/nknown.desktop"
  cp "$pkg/share/nknown.svg" "$d/nknown.svg"
  ln -sf nknown.svg "$d/.DirIcon"
  ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$tool" --no-appstream "$d" \
    "$dist/NKnown-x86_64.AppImage" >/dev/null 2>&1
  say "dist/NKnown-x86_64.AppImage"
}

build_flatpak() {
  command -v flatpak-builder >/dev/null || { skip flatpak "flatpak-builder absent"; return; }
  local d=$dist/stage/flatpak
  rm -rf "$d" && mkdir -p "$d"
  cp "$pkg/flatpak/io.github.fnyaker.NKnown.yml" "$pkg/flatpak/io.github.fnyaker.NKnown.metainfo.xml" "$d/"
  cp "$pkg/appimage/nknown.desktop" "$d/nknown.desktop"
  cp -r "$stage" "$d/payload"
  (cd "$d" && flatpak-builder --user --install-deps-from=flathub --force-clean \
      --repo=repo build io.github.fnyaker.NKnown.yml >/dev/null \
    && flatpak build-bundle repo "$dist/NKnown-x86_64.flatpak" io.github.fnyaker.NKnown)
  say "dist/NKnown-x86_64.flatpak"
}

formats=("$@")
[ ${#formats[@]} -gt 0 ] || formats=(tar zip deb rpm arch appimage flatpak)
for f in "${formats[@]}"; do
  case $f in
    tar|zip|deb|rpm|arch|appimage|flatpak) "build_$f" ;;
    *) echo "format inconnu : $f" >&2; exit 2 ;;
  esac
done

(cd "$dist" && sha256sum -- *.tar.gz *.zip *.deb *.rpm *.pkg.tar.zst *.AppImage *.flatpak 2>/dev/null \
  > SHA256SUMS || true)
say "dist/SHA256SUMS"
