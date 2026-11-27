#!/bin/sh
# Install what building Trawlr needs, on Debian, Ubuntu, Fedora, openSUSE or
# Arch, from the distribution's own packages. Run as root (CI, containers).
#   dist/build-deps.sh          compilers, Qt and Rust
#   dist/build-deps.sh package  also the tools dist/package.sh uses
set -eu
. /etc/os-release
case " $ID ${ID_LIKE:-} " in
    *" debian "*|*" ubuntu "*)
        export DEBIAN_FRONTEND=noninteractive
        apt-get update -q
        apt-get install -yq --no-install-recommends build-essential qt6-base-dev qt6-base-dev-tools libgl-dev \
            python3 curl ca-certificates cargo dpkg-dev
        # Ubuntu 22.04 ships qmake6 as a package of its own.
        apt-get install -yq --no-install-recommends qmake6 2>/dev/null || true ;;
    *" fedora "*)
        dnf install -yq gcc-c++ make qt6-qtbase-devel cargo python3 findutils curl
        [ "${1:-}" = package ] && dnf install -yq rpm-build ;;
    *" suse "*|*" opensuse "*)
        zypper -nq install gcc-c++ make qt6-base-devel qt6-widgets-devel qt6-network-devel qt6-dbus-devel \
            cargo python3 tar gzip curl
        [ "${1:-}" = package ] && zypper -nq install rpm-build ;;
    *" arch "*)
        pacman -Syu --noconfirm --needed base-devel qt6-base hicolor-icon-theme rust python ;;
    *) echo "build-deps.sh: unknown distribution $ID" >&2; exit 1 ;;
esac

# Long-term releases package a Rust older than trawlr needs: get a current one.
need=$(sed -n 's/^rust-version = "\(.*\)"/\1/p' "$(dirname "$0")/../Cargo.toml")
have=$(cargo --version 2>/dev/null | cut -d' ' -f2)
if [ "$(printf '%s\n%s\n' "$need" "$have" | sort -V | head -n1)" != "$need" ]; then
    echo "Rust $have is older than $need: installing a current one from rustup.rs"
    curl -sSf https://sh.rustup.rs | sh -s -- -yq --profile minimal >/dev/null
    echo "Run: . \$HOME/.cargo/env"
fi
