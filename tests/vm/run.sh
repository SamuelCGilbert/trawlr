#!/bin/bash
# Exercise trawlrd as root, the way the system service runs, inside a
# throwaway QEMU virtual machine: filesystem-wide fanotify marks on ext4,
# btrfs and vfat, mounting and unmounting a drive while it runs, the reduced
# capability set and read-only view the systemd unit gives it, per-user access
# control, and the randomized stress test.
#
# Nothing on the host is touched and no root access is needed. Requires
# qemu-system-x86_64 with KVM, busybox-static, cpio, fakeroot, e2fsprogs,
# btrfs-progs, dosfstools and (on Debian/Ubuntu) apt-get to fetch a copy of
# the running kernel, whose image in /boot is not readable by normal users.
#
#   tests/vm/run.sh            build it all and run
#   WORK=/some/dir tests/vm/run.sh
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)
WORK=${WORK:-${XDG_CACHE_HOME:-$HOME/.cache}/trawlrd-vm}
KVER=$(uname -r)
mkdir -p "$WORK"

if [ ! -r "$WORK/vmlinuz" ]; then
    if [ -r "/boot/vmlinuz-$KVER" ]; then
        cp "/boot/vmlinuz-$KVER" "$WORK/vmlinuz"
    else
        (cd "$WORK" && apt-get download "linux-image-$KVER" >/dev/null && dpkg-deb -x linux-image-"$KVER"_*.deb kpkg)
        cp "$WORK/kpkg/boot/vmlinuz-$KVER" "$WORK/vmlinuz"
    fi
fi

ST=$WORK/root
rm -rf "$ST"
mkdir -p "$ST"/{bin,proc,sys,dev,run,tmp,mnt,etc,var/lib,root,modules,repo/tests,repo/target/release}

# Copy a dynamically linked program and every library it needs.
copy_bin() {
    local src=$1 dst=${2:-$ST/bin/$(basename "$1")}
    cp -L "$src" "$dst"
    copy_libs "$src"
}

# Copy the shared libraries a binary links against, keeping their paths.
copy_libs() {
    { ldd "$1" 2>/dev/null || true; } | { grep -o '/[^ ]*' || true; } | while read -r lib; do
        mkdir -p "$ST$(dirname "$lib")"
        [ -e "$ST$lib" ] || cp -L "$lib" "$ST$lib" 2>/dev/null || true
    done
}

cp -L "$(command -v busybox)" "$ST/bin/busybox"
for a in sh mount umount mkdir ls cat rm mv touch cp sleep sort find head tail wc sync ln dd seq tee tr sed grep insmod diff chmod chown id date ps kill pkill pgrep rmdir echo true false test env uname dmesg cut poweroff printf basename dirname stat; do
    ln -s busybox "$ST/bin/$a"
done
# The real util-linux tools go in their own directory: busybox's shell runs
# its built-in applets of the same name in preference to anything in PATH.
mkdir -p "$ST/ul"
for b in mount umount unshare setpriv; do copy_bin "$(command -v $b)" "$ST/ul/$b"; done
copy_bin "$REPO/target/release/trawlrd" "$ST/repo/target/release/trawlrd"
copy_bin "$REPO/target/release/trawlr" "$ST/repo/target/release/trawlr"
ln -s /repo/target/release/trawlrd "$ST/bin/trawlrd"
ln -s /repo/target/release/trawlr "$ST/bin/trawlr"
cp "$REPO/tests/stress.py" "$ST/repo/tests/"

# Python for the stress test.
PY=$(readlink -f "$(command -v python3)")
PYLIB=$(python3 -c 'import sysconfig; print(sysconfig.get_path("stdlib"))')
copy_bin "$PY" "$ST/bin/python3"
mkdir -p "$ST$PYLIB"
(cd "$PYLIB" && tar cf - --exclude=test --exclude=tests --exclude=__pycache__ --exclude=site-packages --exclude=dist-packages --exclude='idlelib' --exclude='tkinter' --exclude='ensurepip' .) | (cd "$ST$PYLIB" && tar xf -)
for so in "$ST$PYLIB"/lib-dynload/*.so; do copy_libs "$so"; done

# btrfs is a module on most distribution kernels.
n=0
for mod in btrfs nls_cp437 nls_iso8859_1; do
    { modprobe --show-depends "$mod" 2>/dev/null || true; } | awk '/^insmod/{print $2}' | while read -r m; do
        n=$((n + 1)); out=$(printf '%s/modules/%s-%02d-%s' "$ST" "$mod" "$n" "$(basename "${m%.zst}")")
        case $m in *.zst) zstd -dqf "$m" -o "$out" ;; *.xz) xz -dc "$m" > "$out" ;; *) cp "$m" "$out" ;; esac
    done
done

printf 'root:x:0:0:root:/root:/bin/sh\nsam:x:1000:1000::/tmp:/bin/sh\nother:x:1001:1001::/tmp:/bin/sh\n' > "$ST/etc/passwd"
printf 'root:x:0:\nsam:x:1000:\nother:x:1001:\n' > "$ST/etc/group"
cp "$HERE/guest.sh" "$ST/init"
chmod +x "$ST/init"

fakeroot sh -c "cd '$ST' && find . | cpio -o -H newc --quiet | gzip -1 > '$WORK/initramfs.gz'"

# Disk images: three filesystems to index, and a "USB stick" to plug in later.
cd "$WORK"
rm -rf usbsrc *.img
truncate -s 400M ext4.img && mkfs.ext4 -q -F ext4.img
truncate -s 400M btrfs.img && mkfs.btrfs -q -f btrfs.img >/dev/null
truncate -s 64M vfat.img && mkfs.vfat vfat.img >/dev/null
mkdir -p usbsrc/photos && echo hi > usbsrc/usbfile.txt && echo jpg > usbsrc/photos/holiday.jpg
truncate -s 64M usb.img && mkfs.ext4 -q -F -d usbsrc usb.img

drives=()
for img in ext4 btrfs vfat usb; do drives+=(-drive "file=$WORK/$img.img,if=virtio,format=raw"); done
timeout "${VM_TIMEOUT:-900}" qemu-system-x86_64 -enable-kvm -cpu host -m 3072 -smp 4 -nographic -no-reboot \
    -kernel "$WORK/vmlinuz" -initrd "$WORK/initramfs.gz" -append "console=ttyS0 quiet panic=-1 loglevel=3" \
    "${drives[@]}" 2>&1 | tr -d '\r' | tee "$WORK/console.log" | grep -v '^\[ *[0-9.]*\]' || true
grep -q '^VM-RESULT: PASS' "$WORK/console.log"
