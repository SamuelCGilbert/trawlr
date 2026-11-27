#!/bin/sh
# Turn a release's Linux packages into signed package sources for apt,
# dnf/zypper and pacman, so a system that adds trawlr.io once gets every
# update through its usual upgrade:
#
#   dist/repo/build.sh PACKAGE_DIR OUT_DIR
#
# PACKAGE_DIR holds the release's .deb, .rpm and .pkg.tar.zst files. The
# signing key must be in gpg's keyring (GNUPGHOME); its fingerprint is in
# dist/repo/fingerprint (or SIGNING_KEY, for tests). BASE_URL is where OUT_DIR
# is served, https://trawlr.io by default. Writes OUT_DIR/{apt,rpm,arch} and the public key as
# OUT_DIR/trawlr.asc (armoured) and OUT_DIR/trawlr.gpg (binary, for apt).
#
# Needs: apt-ftparchive (apt-utils), createrepo_c, rpmsign (rpm), repo-add
# (pacman-package-manager and makepkg on Ubuntu, or pacman), gpg.
set -eu
IN=$(cd "$1" && pwd)
mkdir -p "$2"
OUT=$(cd "$2" && pwd)
HERE=$(cd "$(dirname "$0")" && pwd)
KEY=${SIGNING_KEY:-$(cat "$HERE/fingerprint")}
BASE=${BASE_URL:-https://trawlr.io}
gpg --batch --list-secret-keys "$KEY" >/dev/null

gpg --batch --yes --armor --export "$KEY" > "$OUT/trawlr.asc"
gpg --batch --yes --export "$KEY" > "$OUT/trawlr.gpg"

# --- apt: one suite per release the packages are built for.
# The file names say which (trawlr_0.1.0-1.ubuntu24.04_amd64.deb).
codename() {
    case $1 in
    ubuntu22.04) echo jammy ;; ubuntu24.04) echo noble ;;
    debian12) echo bookworm ;; debian13) echo trixie ;;
    *) return 1 ;;
    esac
}
apt=$OUT/apt
suites=
for f in "$IN"/*.deb; do
    [ -e "$f" ] || continue
    dist=$(basename "$f" | sed -n 's/^trawlr_[^-]*-[0-9]*[.~]\([a-z0-9.]*\)_[a-z0-9]*\.deb$/\1/p')
    c=$(codename "$dist") || { echo "no apt suite for $f" >&2; exit 1; }
    mkdir -p "$apt/pool/$c"
    cp "$f" "$apt/pool/$c/"
    case " $suites " in *" $c "*) ;; *) suites="$suites $c" ;; esac
done
for c in $suites; do
    for arch in amd64 arm64; do
        d=$apt/dists/$c/main/binary-$arch
        mkdir -p "$d"
        (cd "$apt" && apt-ftparchive --arch "$arch" packages "pool/$c") > "$d/Packages"
        gzip -9nkf "$d/Packages"
    done
    (cd "$apt/dists/$c" && apt-ftparchive \
        -o APT::FTPArchive::Release::Origin=Trawlr \
        -o APT::FTPArchive::Release::Label=Trawlr \
        -o APT::FTPArchive::Release::Suite="$c" \
        -o APT::FTPArchive::Release::Codename="$c" \
        -o APT::FTPArchive::Release::Architectures="amd64 arm64" \
        -o APT::FTPArchive::Release::Components=main \
        -o APT::FTPArchive::Release::Description="Trawlr, from trawlr.io" \
        release .) > "$apt/Release.tmp"
    mv "$apt/Release.tmp" "$apt/dists/$c/Release"
    gpg --batch --yes -u "$KEY" --digest-algo SHA512 --clearsign -o "$apt/dists/$c/InRelease" "$apt/dists/$c/Release"
    gpg --batch --yes -u "$KEY" --digest-algo SHA512 -abs -o "$apt/dists/$c/Release.gpg" "$apt/dists/$c/Release"
done

# --- rpm: one repository each for Fedora and openSUSE, packages signed too.
rpmrepo() { # name, the distribution's tag in the file names
    d=$OUT/rpm/$1
    mkdir -p "$d"
    found=
    for f in "$IN"/*.$2.*.rpm; do
        [ -e "$f" ] || continue
        cp "$f" "$d/"
        found=1
    done
    [ -n "$found" ] || return 0
    rpmsign --define "__gpg $(command -v gpg)" --define "_gpg_name $KEY" --define "_gpg_digest_algo sha256" --addsign "$d"/*.rpm >/dev/null
    createrepo_c --quiet --general-compress-type=gz "$d"
    gpg --batch --yes -u "$KEY" --armor --detach-sign -o "$d/repodata/repomd.xml.asc" "$d/repodata/repomd.xml"
    cp "$OUT/trawlr.asc" "$d/repodata/repomd.xml.key"
    cat > "$d/trawlr.repo" <<REPO
[trawlr]
name=Trawlr
baseurl=$BASE/rpm/$1
enabled=1
gpgcheck=1
repo_gpgcheck=1
gpgkey=$BASE/trawlr.asc
REPO
}
rpmrepo fedora 'fc[0-9]*'
rpmrepo opensuse tumbleweed

# --- pacman: [trawlr], packages and database signed.
for f in "$IN"/*.pkg.tar.zst; do
    [ -e "$f" ] || continue
    arch=$(basename "$f" .pkg.tar.zst | sed 's/.*-//')
    d=$OUT/arch/$arch
    mkdir -p "$d"
    cp "$f" "$d/"
    gpg --batch --yes -u "$KEY" --detach-sign -o "$d/$(basename "$f").sig" "$d/$(basename "$f")"
    repo-add --quiet --sign --key "$KEY" "$d/trawlr.db.tar.gz" "$d/$(basename "$f")"
done
# repo-add links trawlr.db to trawlr.db.tar.gz; a static site wants copies.
find "$OUT/arch" -type l 2>/dev/null | while read -r l; do
    cp --remove-destination "$(readlink -f "$l")" "$l"
done

find "$OUT" -type f | sed "s|^$OUT/||" | sort
