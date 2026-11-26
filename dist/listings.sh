#!/bin/sh
# Fill in the package listings for a published release, in dist/out/listings:
# the Homebrew cask, the winget manifests, and the AUR PKGBUILD with .SRCINFO.
#   dist/listings.sh 0.1.0
# .SRCINFO is made with makepkg, or in an Arch container if there is none.
set -eu
cd "$(dirname "$0")/.."
v=${1:?usage: dist/listings.sh VERSION}
repo=https://github.com/SamuelCGilbert/trawlr
out=dist/out/listings
rm -rf "$out"
mkdir -p "$out/homebrew" "$out/winget/$v" "$out/aur"

sums=$(curl -fsSL "$repo/releases/download/v$v/SHA256SUMS")
sum() { echo "$sums" | awk -v f="$1" '$2 == f || $2 == "*" f { print $1 }' | grep .; }
mac=$(sum "trawlr-$v-macos.pkg")
win=$(sum "trawlr-$v-windows-x64-setup.exe" | tr a-f A-F)
src=$(curl -fsSL "$repo/archive/refs/tags/v$v.tar.gz" | sha256sum | cut -d' ' -f1)

sed "s/@VERSION@/$v/g; s/@SHA256@/$mac/g" dist/homebrew/trawlr.rb >"$out/homebrew/trawlr.rb"
for f in dist/winget/*.yaml; do
    sed "s/@VERSION@/$v/g; s/@SHA256@/$win/g" "$f" >"$out/winget/$v/$(basename "$f")"
done
sed "s/^pkgver=.*/pkgver=$v/; s/^pkgrel=.*/pkgrel=1/; s/^sha256sums=.*/sha256sums=('$src')/" dist/arch/PKGBUILD >"$out/aur/PKGBUILD"
cp dist/arch/trawlr.install "$out/aur/"
if command -v makepkg >/dev/null; then
    (cd "$out/aur" && makepkg --printsrcinfo >.SRCINFO)
else
    tar -C "$out/aur" -cf - PKGBUILD trawlr.install | docker run --rm -i archlinux:latest sh -c \
        'useradd -m b && cd /home/b && tar -xf - && chown -R b . && su b -c "makepkg --printsrcinfo"' >"$out/aur/.SRCINFO"
fi
find "$out" -type f | sort
