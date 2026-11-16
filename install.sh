#!/bin/sh
# Build Trawlr and install it.
#
#   ./install.sh            system service (asks for your sudo password once)
#   ./install.sh --user     no root: per-user service that indexes only $HOME
#   ./install.sh --no-gui   service and the `trawlr` command only
set -eu
cd "$(dirname "$0")"

MODE=system
GUI=1
for arg in "$@"; do
    case $arg in
        --user) MODE=user ;;
        --no-gui) GUI=0 ;;
        -h|--help) sed -n '2,7p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "install.sh: unknown option $arg" >&2; exit 2 ;;
    esac
done

QMAKE=
for q in qmake6 qmake-qt6 qmake; do
    if command -v "$q" >/dev/null 2>&1 && "$q" -query QT_VERSION 2>/dev/null | grep -q '^6\.'; then QMAKE=$q; break; fi
done

missing=
command -v cargo >/dev/null 2>&1 || missing="$missing cargo"
command -v cc >/dev/null 2>&1 || missing="$missing a-C-compiler"
if [ $GUI = 1 ]; then
    [ -n "$QMAKE" ] || missing="$missing qt6-base-dev"
    command -v make >/dev/null 2>&1 || missing="$missing make"
    command -v c++ >/dev/null 2>&1 || missing="$missing a-C++-compiler"
fi
if [ -n "$missing" ]; then
    echo "Missing build tools:$missing" >&2
    echo "  Debian/Ubuntu:  sudo apt install cargo build-essential qt6-base-dev" >&2
    echo "  Fedora:         sudo dnf install cargo gcc-c++ make qt6-qtbase-devel" >&2
    echo "  Arch:           sudo pacman -S rust base-devel qt6-base" >&2
    echo "(or run with --no-gui to skip the graphical client)" >&2
    exit 1
fi

# Long-term releases (Debian 12, Ubuntu 24.04 and older) package a Rust that
# is too old for the libraries trawlr uses.
need=$(sed -n 's/^rust-version = "\(.*\)"/\1/p' Cargo.toml)
have=$(cargo --version 2>/dev/null | cut -d' ' -f2)
if [ "$(printf '%s\n%s\n' "$need" "$have" | sort -V | head -n1)" != "$need" ]; then
    echo "Rust $need or newer is needed; this system has $have." >&2
    echo "  Install a current one from https://rustup.rs (no root needed), then run this again:" >&2
    echo "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh" >&2
    exit 1
fi

echo "==> Building trawlrd and trawlr"
cargo build --release
if [ $GUI = 1 ]; then
    echo "==> Building the graphical client"
    mkdir -p gui/build
    (cd gui/build && "$QMAKE" ../trawlr.pro >/dev/null && make -j"$(nproc 2>/dev/null || echo 2)" | grep -v '^\(g++\|/usr\|moc\|rcc\|make\)' || true)
    [ -x gui/build/trawlr-gui ] || { echo "GUI build failed" >&2; exit 1; }
fi

if [ $MODE = user ]; then
    exec sh dist/setup.sh user "$PWD" "$(id -un)" $GUI
fi
echo "==> Installing (needs root)"
if [ "$(id -u)" = 0 ]; then
    exec sh dist/setup.sh system "$PWD" "${SUDO_USER:-root}" $GUI
fi
exec ${SUDO:-sudo} sh dist/setup.sh system "$PWD" "$(id -un)" $GUI
