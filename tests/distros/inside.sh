#!/bin/sh
# Runs inside one container for tests/distros/run.sh.
set -eu
step() { echo "STEP $*"; }

step installing build dependencies
command -v tar >/dev/null || zypper -nq install tar gzip
mkdir -p /work && cd /work
tar -C /src --exclude=./target --exclude=./gui/build --exclude=./gui/tests/build --exclude=./.git -cf - . | tar -xf -
sh dist/build-deps.sh package
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
. /etc/os-release
echo "RESULT $PRETTY_NAME"
echo "RESULT $(rustc --version | cut -d' ' -f1-2), Qt $(for q in qmake6 qmake-qt6 qmake; do command -v $q >/dev/null && $q -query QT_VERSION && break; done)"

step ./install.sh
TRAWLR_NO_START=1 ./install.sh
test -x /usr/local/bin/trawlr-gui
/usr/local/bin/trawlr --version
test -f /usr/local/share/applications/io.trawlr.Trawlr.desktop

step cargo test
cargo test --release -q

step GUI tests
QMAKE=$(for q in qmake6 qmake-qt6 qmake; do command -v $q >/dev/null && echo $q && break; done)
mkdir -p gui/tests/build && cd gui/tests/build
"$QMAKE" ../tests.pro >/dev/null && make -s -j"$(nproc)" 2>&1 | grep -E 'error|warning' || true
QT_QPA_PLATFORM=offscreen ./tst_gui
cd /work

step stress test
python3 tests/stress.py --rounds 10 --dir /scratch
step packaging
# Start from a clean slate: the install.sh step above already put files,
# the trawlr group and a configuration in place.
rm -rf /usr/local/bin/trawlr /usr/local/bin/trawlrd /usr/local/bin/trawlr-gui /etc/trawlrd /etc/systemd/system/trawlrd.service
groupdel trawlr
case " $ID ${ID_LIKE:-} " in
    *" debian "*|*" ubuntu "*)
        pkg=$(sh dist/package.sh deb)
        apt-get install -yq "$pkg"
        remove() { apt-get purge -yq trawlr; } ;;
    *" fedora "*)
        pkg=$(sh dist/package.sh rpm)
        dnf install -yq "$pkg"
        remove() { dnf remove -yq trawlr; } ;;
    *" suse "*|*" opensuse "*)
        pkg=$(sh dist/package.sh rpm)
        zypper -nq install --allow-unsigned-rpm "$pkg"
        remove() { zypper -nq remove trawlr; } ;;
    *" arch "*)
        # makepkg refuses to run as root, and builds from a release tarball.
        id builder >/dev/null 2>&1 || useradd -m builder
        b=/home/builder/pkg; rm -rf "$b"; mkdir -p "$b"
        v=$(sed -n 's/^pkgver=//p' dist/arch/PKGBUILD)
        tar --exclude=./target --exclude=./gui/build --exclude=./gui/tests/build --exclude=./dist/out \
            --transform "s,^\.,trawlr-$v," -czf "$b/trawlr-$v.tar.gz" .
        cp dist/arch/PKGBUILD dist/arch/trawlr.install "$b/"
        chown -R builder "$b"
        su builder -c "cd $b && TMPDIR=/tmp makepkg -f --nocheck" >/dev/null
        pkg=$(ls "$b"/trawlr-*.pkg.tar.zst)
        pacman -U --noconfirm "$pkg"
        remove() { pacman -R --noconfirm trawlr; } ;;
esac
echo "RESULT package $(basename "$pkg")"
test -x /usr/bin/trawlr-gui
/usr/bin/trawlr --version
grep -q '^ExecStart=/usr/bin/trawlrd$' /usr/lib/systemd/system/trawlrd.service
getent group trawlr
grep -q '^allowed_groups' /etc/trawlrd/config.toml
test -f /usr/share/applications/io.trawlr.Trawlr.desktop
# A changed configuration survives an upgrade (reinstall).
echo '# kept' >> /etc/trawlrd/config.toml
case " $ID ${ID_LIKE:-} " in
    *" debian "*|*" ubuntu "*) apt-get install -yq --reinstall "$pkg" ;;
    *" fedora "*) dnf reinstall -yq "$pkg" ;;
    *" suse "*|*" opensuse "*) zypper -nq install -f --allow-unsigned-rpm "$pkg" ;;
    *" arch "*) pacman -U --noconfirm "$pkg" ;;
esac
grep -q '^# kept' /etc/trawlrd/config.toml
remove
test ! -e /usr/bin/trawlr
echo "RESULT all steps passed"
