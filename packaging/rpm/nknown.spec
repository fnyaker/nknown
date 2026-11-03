# Paquet RPM à partir des fichiers déjà construits (packaging/build.sh rpm définit les macros).
Name:           nknown
Version:        %{version}
Release:        1
Summary:        %{summary}
License:        Proprietary
URL:            %{homepage}
ExclusiveArch:  x86_64
Requires:       glibc >= 2.28
AutoReqProv:    no

%description
Indexe vos dossiers, flux et pages, analyse les textes (français, anglais, russe, chinois) et
relie mots, passages et documents dans un graphe pondéré. Le démon (service systemd utilisateur
nknownd) est activé pour chaque utilisateur ; il télécharge ses modèles au premier lancement.

%install
cp -a %{stageroot}/. %{buildroot}/

%post
systemctl --global enable nknownd.service >/dev/null 2>&1 || :

%preun
if [ "$1" -eq 0 ]; then
  systemctl --global disable nknownd.service >/dev/null 2>&1 || :
fi

%files
/usr/bin/nknown
/usr/bin/nknownd
/usr/lib/systemd/user/nknownd.service
/usr/share/applications/nknown.desktop
/usr/share/krunner/dbusplugins/nknown.desktop
/usr/share/dbus-1/services/org.nknown.krunner.service
/usr/share/icons/hicolor/scalable/apps/nknown.svg
%doc /usr/share/doc/nknown/README.md
%doc /usr/share/doc/nknown/MODELS.md
