#!/bin/sh
# End-to-end check of the package sources (dist/repo/build.sh): builds them
# from a release's packages with a throwaway signing key, serves them on a
# private Docker network, then on each supported distribution follows the
# README's "updates through your package manager" steps, installs Trawlr,
# and finally checks that a newer version arrives with a normal upgrade.
#
#   tests/repo/run.sh [TAG]          (default: the latest release)
#
# Needs docker and gh. Only the URL and the key fingerprint differ from the
# published instructions.
set -eu
cd "$(dirname "$0")/../.."
TAG=${1:-$(gh release view --json tagName -q .tagName)}
ID=trawlr-repo-$$
WORK=$(mktemp -d)
cleanup() {
    docker ps -aq --filter "label=$ID" | xargs -r docker rm -fv >/dev/null
    docker network rm "$ID" >/dev/null 2>&1 || true
    rm -rf "$WORK"
}
trap cleanup EXIT
step() { echo "==> $*"; }

step "packages of $TAG"
gh release download "$TAG" -D "$WORK/pkgs" -p '*.deb' -p '*.rpm' -p '*.pkg.tar.zst'

docker network create "$ID" >/dev/null
start() { # name image
    docker run -d --name "$ID-$1" --hostname "$1" --network "$ID" --network-alias "$1" --label "$ID" "$2" sleep infinity >/dev/null
}
on() { name=$1; shift; docker exec -i "$ID-$name" sh -euc "$@"; }

step "building the sources"
start repo ubuntu:24.04
on repo 'apt-get -qq update && DEBIAN_FRONTEND=noninteractive apt-get -qq install -y --no-install-recommends \
    apt-utils createrepo-c rpm pacman-package-manager makepkg libarchive-tools zstd gnupg python3 >/dev/null'
tar c dist/repo | on repo 'tar x -C /'
docker cp "$WORK/pkgs" "$ID-repo:/pkgs"
on repo 'gpg --batch -q --passphrase "" --quick-gen-key "Test <test@example.invalid>" rsa3072 sign never 2>/dev/null'
FPR=$(on repo 'gpg --with-colons --list-secret-keys | awk -F: "/^fpr/{print \$10; exit}"')
build() { on repo "SIGNING_KEY=$FPR BASE_URL=http://repo:8000 sh /dist/repo/build.sh /pkgs /srv"; }
build
on repo 'cd /srv && nohup python3 -m http.server 8000 >/dev/null 2>&1 &'
BASE=http://repo:8000

# The published steps, word for word apart from $BASE and $FPR.
apt_steps="
sudo install -d -m 755 /etc/apt/keyrings
curl -fsSL $BASE/trawlr.gpg | sudo tee /etc/apt/keyrings/trawlr.gpg >/dev/null
echo \"deb [signed-by=/etc/apt/keyrings/trawlr.gpg] $BASE/apt \$(. /etc/os-release; echo \${UBUNTU_CODENAME:-\$VERSION_CODENAME}) main\" | sudo tee /etc/apt/sources.list.d/trawlr.list
sudo apt update && sudo apt install -y trawlr"
dnf_steps="
sudo curl -fsSLo /etc/yum.repos.d/trawlr.repo $BASE/rpm/fedora/trawlr.repo
sudo dnf install -y trawlr"
zypper_steps="
sudo zypper --non-interactive --gpg-auto-import-keys addrepo $BASE/rpm/opensuse/trawlr.repo
sudo zypper --non-interactive --gpg-auto-import-keys install trawlr"
pacman_steps="
curl -fsSL $BASE/trawlr.asc | sudo pacman-key --add -
sudo pacman-key --lsign-key $FPR
printf '\n[trawlr]\nServer = $BASE/arch/\$arch\n' | sudo tee -a /etc/pacman.conf
sudo pacman -Syu --noconfirm trawlr"

# name image prepare steps
client() {
    step "$1 ($2)"
    start "$1" "$2"
    on "$1" "$3" >/dev/null 2>&1
    on "$1" "sudo() { \"\$@\"; }; $4" >"$WORK/$1.log" 2>&1 || { tail -30 "$WORK/$1.log"; echo "FAILED: $1"; exit 1; }
    on "$1" 'trawlr --version'
}
apt_prep='apt-get -qq update && DEBIAN_FRONTEND=noninteractive apt-get -qq install -y curl ca-certificates'
export DEBIAN_FRONTEND=noninteractive
client jammy ubuntu:22.04 "$apt_prep" "export DEBIAN_FRONTEND=noninteractive; $apt_steps"
client noble ubuntu:24.04 "$apt_prep" "export DEBIAN_FRONTEND=noninteractive; $apt_steps"
client bookworm debian:12 "$apt_prep" "export DEBIAN_FRONTEND=noninteractive; $apt_steps"
client trixie debian:13 "$apt_prep" "export DEBIAN_FRONTEND=noninteractive; $apt_steps"
client fedora fedora:latest true "$dnf_steps"
client opensuse opensuse/tumbleweed:latest true "$zypper_steps"
client arch archlinux:latest 'pacman-key --init; pacman-key --populate archlinux' "$pacman_steps"

step "a newer version reaches apt and pacman"
on repo '
cd /tmp && rm -rf deb pkg && v=$(ls /pkgs/*.ubuntu24.04_amd64.deb)
dpkg-deb -R "$v" deb && sed -i "s/^Version: \([0-9.]*\)-/Version: 9.9.9-/" deb/DEBIAN/control
dpkg-deb -b deb /pkgs/trawlr_9.9.9-1.ubuntu24.04_amd64.deb >/dev/null && rm "$v"
a=$(ls /pkgs/*.pkg.tar.zst) && mkdir pkg && bsdtar -xf "$a" -C pkg
sed -i "s/^pkgver = .*/pkgver = 9.9.9-1/" pkg/.PKGINFO
(cd pkg && bsdtar --zstd -cf /pkgs/trawlr-9.9.9-1-x86_64.pkg.tar.zst .PKGINFO .BUILDINFO .MTREE $(ls -A | grep -v "^\.\(PKGINFO\|BUILDINFO\|MTREE\)\$")) && rm "$a"
rm -rf /srv/*'
build
on noble 'apt-get -qq update && apt-get -qq install -y --only-upgrade trawlr >/dev/null && dpkg-query -W trawlr'
on arch 'pacman -Syu --noconfirm >/dev/null && pacman -Q trawlr'
step "all good"
