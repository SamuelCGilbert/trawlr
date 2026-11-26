#!/bin/sh
# Turn a finished build (cargo build --release, then the GUI in gui/build)
# into a .deb or .rpm for the distribution this runs on, in dist/out/.
#   dist/package.sh deb|rpm
#   dist/package.sh stage DIR   only lay the files out under DIR (Arch's PKGBUILD)
# The package installs to /usr, starts the service, and keeps
# /etc/trawlrd/config.toml across upgrades.
set -eu
cd "$(dirname "$0")/.."
KIND=$1
PKGVER=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)
LICENSE_NAME=$(sed -n 's/^license = "\(.*\)"/\1/p' Cargo.toml | head -n1)
MAINTAINER=$(sed -n 's/^authors = \["\(.*\)"\]/\1/p' Cargo.toml | head -n1)
OUT=$PWD/dist/out
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
ROOT=$WORK/root
[ "$KIND" = stage ] && ROOT=$2

for f in target/release/trawlrd target/release/trawlr gui/build/trawlr-gui; do
    [ -x "$f" ] || { echo "package.sh: $f is missing; build first" >&2; exit 1; }
done

# The files, laid out as installed.
install -Dm755 target/release/trawlrd "$ROOT/usr/bin/trawlrd"
install -Dm755 target/release/trawlr "$ROOT/usr/bin/trawlr"
install -Dm755 gui/build/trawlr-gui "$ROOT/usr/bin/trawlr-gui"
install -Dm644 dist/trawlrd.service "$ROOT/usr/lib/systemd/system/trawlrd.service"
sed -i 's|^ExecStart=.*|ExecStart=/usr/bin/trawlrd|' "$ROOT/usr/lib/systemd/system/trawlrd.service"
install -Dm644 dist/trawlrd-user.service "$ROOT/usr/lib/systemd/user/trawlrd.service"
sed -i 's|^ExecStart=.*|ExecStart=/usr/bin/trawlrd|' "$ROOT/usr/lib/systemd/user/trawlrd.service"
# Members of this group may search (see allowed_groups in the configuration).
install -d "$ROOT/usr/lib/sysusers.d"
echo 'g trawlr - -' > "$ROOT/usr/lib/sysusers.d/trawlr.conf"
for f in io.trawlr.Trawlr io.trawlr.Trawlr-new-window io.trawlr.Trawlr-toggle; do
    install -Dm644 "dist/$f.desktop" "$ROOT/usr/share/applications/$f.desktop"
done
install -Dm644 assets/app-icon.svg "$ROOT/usr/share/icons/hicolor/scalable/apps/io.trawlr.Trawlr.svg"
install -Dm644 dist/config.toml "$ROOT/etc/trawlrd/config.toml"
install -Dm644 README.md "$ROOT/usr/share/doc/trawlr/README.md"
[ -f LICENSE ] && install -Dm644 LICENSE "$ROOT/usr/share/licenses/trawlr/LICENSE"
[ "$KIND" = stage ] && exit 0
mkdir -p "$OUT"

SUMMARY="Instant file-name search with a live, always-current index"
DESCRIPTION="Trawlr finds any file or folder on the computer by name as you type. A small
system service (trawlrd) indexes every local disk once and then follows each
change through fanotify, so results are always current. It comes with a
window (trawlr-gui) and a command (trawlr)."

# Shared by both formats: what runs after installing or upgrading, and before
# removing. $1 is "install", "upgrade" or "remove".
cat > "$WORK/scripts" <<'SCRIPT'
have_systemd() { [ -d /run/systemd/system ]; }
case $1 in
install|upgrade)
    systemd-sysusers trawlr.conf >/dev/null 2>&1 || getent group trawlr >/dev/null || groupadd -r trawlr || true
    command -v update-desktop-database >/dev/null && update-desktop-database -q /usr/share/applications || true
    command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -qtf /usr/share/icons/hicolor || true
    if have_systemd; then
        systemctl daemon-reload || true
        if [ "$1" = install ]; then
            systemctl enable --now trawlrd >/dev/null 2>&1 || true
        else
            systemctl try-restart trawlrd || true
        fi
    fi ;;
remove)
    have_systemd && systemctl disable --now trawlrd >/dev/null 2>&1 || true ;;
esac
SCRIPT

case $KIND in
deb)
    . /etc/os-release
    ARCH=$(dpkg --print-architecture)
    REV="1~$ID${VERSION_ID:-}"
    mkdir -p "$ROOT/DEBIAN" "$WORK/debian"
    # dpkg-shlibdeps works out the library packages (their names differ
    # between releases, e.g. libqt6widgets6 and libqt6widgets6t64).
    printf 'Source: trawlr\n\nPackage: trawlr\nArchitecture: any\n' > "$WORK/debian/control"
    deps=$(cd "$WORK" && dpkg-shlibdeps -O "$ROOT/usr/bin/trawlrd" "$ROOT/usr/bin/trawlr" "$ROOT/usr/bin/trawlr-gui" 2>/dev/null \
        | sed -n 's/^shlibs:Depends=//p')
    size=$(du -sk "$ROOT" | cut -f1)
    {
        echo "Package: trawlr"
        echo "Version: $PKGVER-$REV"
        echo "Architecture: $ARCH"
        echo "Maintainer: $MAINTAINER"
        echo "Installed-Size: $size"
        # The platform plugins Qt loads at run time are not linked, so
        # dpkg-shlibdeps cannot see them.
        echo "Depends: $deps, qt6-qpa-plugins, systemd"
        echo "Recommends: qt6-wayland"
        echo "Section: utils"
        echo "Priority: optional"
        echo "Homepage: https://github.com/SamuelCGilbert/trawlr"
        echo "Description: $SUMMARY"
        echo "$DESCRIPTION" | sed 's/^/ /'
    } > "$ROOT/DEBIAN/control"
    echo /etc/trawlrd/config.toml > "$ROOT/DEBIAN/conffiles"
    { echo 'Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/'
      echo 'Upstream-Name: trawlr'
      echo 'Source: https://github.com/SamuelCGilbert/trawlr'
      echo; echo 'Files: *'; echo "Copyright: 2026 $MAINTAINER"; echo "License: GPL-3+"
      echo ' On Debian systems, the full text of the GNU General Public License'
      echo ' version 3 is in /usr/share/common-licenses/GPL-3.'; } > "$ROOT/usr/share/doc/trawlr/copyright"
    # postinst gets "configure <old version>": empty on a first install.
    { echo '#!/bin/sh'; echo 'set -e'; echo 'main() {'; cat "$WORK/scripts"; echo '}'
      echo '[ "$1" = configure ] || exit 0'; echo 'if [ -z "${2:-}" ]; then main install; else main upgrade; fi'; } > "$ROOT/DEBIAN/postinst"
    { echo '#!/bin/sh'; echo 'set -e'; echo 'main() {'; cat "$WORK/scripts"; echo '}'
      echo '[ "$1" = remove ] || exit 0'; echo 'main remove'; } > "$ROOT/DEBIAN/prerm"
    { echo '#!/bin/sh'; echo 'set -e'
      echo '[ "$1" = purge ] && rm -rf /var/lib/trawlrd /etc/trawlrd'
      echo '[ -d /run/systemd/system ] && systemctl daemon-reload || true'; } > "$ROOT/DEBIAN/postrm"
    chmod 755 "$ROOT/DEBIAN/postinst" "$ROOT/DEBIAN/prerm" "$ROOT/DEBIAN/postrm"
    file="$OUT/trawlr_${PKGVER}-${REV}_$ARCH.deb"
    dpkg-deb --root-owner-group -Zxz --build "$ROOT" "$file" >/dev/null
    ;;
rpm)
    . /etc/os-release
    mkdir -p "$WORK/rpm/SPECS" "$WORK/rpm/RPMS"
    # A tag for the release field, which may not contain "-".
    case $ID in
        fedora) DIST=fc$VERSION_ID ;;
        opensuse-tumbleweed) DIST=tumbleweed ;;
        opensuse-leap) DIST=leap$VERSION_ID ;;
        *) DIST=$(echo "$ID${VERSION_ID:-}" | tr -cd 'a-z0-9.') ;;
    esac
    {
        echo "Name: trawlr"
        echo "Version: $PKGVER"
        echo "Release: 1.$DIST"
        echo "Summary: $SUMMARY"
        echo "License: $LICENSE_NAME"
        echo "Packager: $MAINTAINER"
        echo "URL: https://github.com/SamuelCGilbert/trawlr"
        echo "Requires: systemd"
        echo "Recommends: qt6-qtwayland"
        echo "%description"
        echo "$DESCRIPTION"
        echo "%install"
        echo "cp -a $ROOT/. %{buildroot}/"
        echo "%files"
        (cd "$ROOT" && find . -type f -o -type l | sed 's|^\.||' | grep -v '^/etc/trawlrd/config.toml$' | sort)
        echo "%config(noreplace) /etc/trawlrd/config.toml"
        echo "%dir /etc/trawlrd"
        echo "%dir /usr/share/doc/trawlr"
        # $1 counts the copies installed after the step: 1 means a first install.
        echo "%post"
        echo 'main() {'; cat "$WORK/scripts"; echo '}'
        echo 'if [ "$1" -eq 1 ]; then main install; else main upgrade; fi'
        echo "%preun"
        echo 'main() {'; cat "$WORK/scripts"; echo '}'
        echo 'if [ "$1" -eq 0 ]; then main remove; fi'
        echo "%postun"
        echo '[ -d /run/systemd/system ] && systemctl daemon-reload || true'
    } > "$WORK/rpm/SPECS/trawlr.spec"
    rpmbuild -bb --quiet --define "_topdir $WORK/rpm" --define "_build_id_links none" \
        --define "debug_package %{nil}" "$WORK/rpm/SPECS/trawlr.spec"
    cp "$WORK"/rpm/RPMS/*/*.rpm "$OUT/"
    file=$(ls "$WORK"/rpm/RPMS/*/*.rpm)
    file=$OUT/$(basename "$file")
    ;;
*) echo "package.sh: deb or rpm" >&2; exit 2 ;;
esac
echo "$file"
