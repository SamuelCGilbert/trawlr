#!/bin/sh
# Runs as PID 1 inside the test VM; see run.sh.
export PATH=/bin HOME=/root
mount -t proc proc /proc
mount -t sysfs sys /sys
mount -t devtmpfs dev /dev 2>/dev/null
mount -t tmpfs tmpfs /run
mount -t tmpfs tmpfs /tmp
# As systemd does at boot: mounts propagate into namespaces created later.
/ul/mount --make-rshared /
for m in /modules/*.ko; do [ -e "$m" ] && insmod "$m" 2>/dev/null; done

FAILS=0
ok() { echo "  ok   $1"; }
bad() { echo "  FAIL $1"; FAILS=$((FAILS + 1)); }
# expect <description> <expected> <command...>: compare the command's output.
expect() {
    desc=$1; want=$2; shift 2
    got=$("$@" 2>&1)
    if [ "$got" = "$want" ]; then ok "$desc"; else bad "$desc"; echo "         wanted: $want"; echo "         got:    $got"; fi
}
settle() { sleep 0.4; }
wait_ready() {
    i=0
    while [ $i -lt 300 ]; do
        trawlr --status 2>/dev/null | grep -q ', ready' && return 0
        sleep 0.1; i=$((i + 1))
    done
    echo "  daemon never became ready:"; cat /run/log; return 1
}
stop_daemon() { kill -TERM "$DPID"; wait "$DPID"; }
as_user() { uid=$1; shift; /ul/setpriv --reuid "$uid" --regid "$uid" --clear-groups "$@"; }

mkdir -p /mnt/ext4 /mnt/btrfs /mnt/vfat /mnt/usb /etc/trawlrd /var/lib/trawlrd /run/trawlrd
mount -t ext4 /dev/vda /mnt/ext4 || bad "mount ext4"
HAVE_BTRFS=1
mount -t btrfs /dev/vdb /mnt/btrfs 2>/dev/null || { HAVE_BTRFS=0; echo "  (btrfs unavailable in this kernel, skipping)"; }
mount -t vfat /dev/vdc /mnt/vfat || bad "mount vfat"
echo "kernel $(uname -r), rootfs is $(grep ' / ' /proc/mounts | cut -d' ' -f3)"

echo "=== A. system service: whole machine, default settings"
printf 'allowed_users = ["sam"]\nallowed_groups = ["wheel", "no-such-group"]\n' > /etc/trawlrd/config.toml
mkdir -p /mnt/ext4/data/docs /mnt/ext4/private
echo x > /mnt/ext4/data/docs/Report.PDF
echo s > /mnt/ext4/private/secret.txt; chown -R 1001:1001 /mnt/ext4/private; chmod 700 /mnt/ext4/private
echo v > /mnt/vfat/ONVFAT.TXT
[ $HAVE_BTRFS = 1 ] && { mkdir -p /mnt/btrfs/sub; echo b > /mnt/btrfs/sub/onbtrfs.txt; }
trawlrd > /run/log 2>&1 &
DPID=$!
wait_ready
trawlr --status
trawlr --status | grep -q '/mnt/ext4 (ext4): watched filesystem' && ok "ext4 is watched with one filesystem-wide mark" || bad "ext4 not watched filesystem-wide"
[ $HAVE_BTRFS = 1 ] && { trawlr --status | grep -q '/mnt/btrfs (btrfs): watched' && ok "btrfs is watched ($(trawlr --status | grep btrfs | sed 's/.*watched //'))" || bad "btrfs not watched"; }
trawlr --status | grep -q '/mnt/vfat (vfat): watched' && ok "vfat is watched ($(trawlr --status | grep vfat | sed 's/.*watched //'))" || bad "vfat not watched"
expect "initial scan found the ext4 file" "/mnt/ext4/data/docs/Report.PDF" trawlr report
expect "pseudo-filesystems are not entered" "1" sh -c 'trawlr --count "/proc/" >/dev/null; echo $?'
expect "the /proc mount point itself is listed" "/proc" trawlr wfn:proc folder:

echo x > /mnt/ext4/data/live-created.txt; settle
expect "ext4: new file appears" "/mnt/ext4/data/live-created.txt" trawlr live-created
mkdir -p /mnt/ext4/tree/a/b; echo x > /mnt/ext4/tree/a/b/deep.bin; settle
expect "ext4: nested folders created at once" "/mnt/ext4/tree/a/b/deep.bin" trawlr deep.bin
mv /mnt/ext4/tree /mnt/ext4/data/moved-tree; settle
expect "ext4: moving a folder moves its contents" "/mnt/ext4/data/moved-tree/a/b/deep.bin" trawlr deep.bin
dd if=/dev/zero of=/mnt/ext4/data/live-created.txt bs=1000 count=7 2>/dev/null; settle
expect "ext4: size follows a write" "7000" sh -c 'trawlr --json live-created | sed "s/.*\"s\":\([0-9]*\).*/\1/"'
rm -rf /mnt/ext4/data/moved-tree; settle
expect "ext4: deleting a tree removes it" "1" sh -c 'trawlr --count deep.bin >/dev/null; echo $?'
echo x > /mnt/vfat/NEWFAT.TXT; settle
expect "vfat: new file appears" "/mnt/vfat/NEWFAT.TXT" trawlr newfat
if [ $HAVE_BTRFS = 1 ]; then
    echo x > /mnt/btrfs/sub/live-btrfs.txt; mkdir /mnt/btrfs/newdir; echo y > /mnt/btrfs/newdir/inner.txt; settle
    expect "btrfs: new file appears" "/mnt/btrfs/sub/live-btrfs.txt" trawlr live-btrfs
    expect "btrfs: new folder with a file" "/mnt/btrfs/newdir/inner.txt" trawlr inner.txt
    mv /mnt/btrfs/newdir /mnt/btrfs/sub/renamed; settle
    expect "btrfs: folder move" "/mnt/btrfs/sub/renamed/inner.txt" trawlr inner.txt
fi
echo x > /root/on-rootfs.txt; settle
expect "root filesystem: new file appears" "/root/on-rootfs.txt" trawlr on-rootfs

echo "--- plugging in a drive"
mount -t ext4 /dev/vdd /mnt/usb; sleep 1.5
expect "mounted drive is indexed" "/mnt/usb/usbfile.txt" trawlr usbfile
echo x > /mnt/usb/photos/added-later.jpg; settle
expect "and watched" "/mnt/usb/photos/added-later.jpg" trawlr added-later
umount /mnt/usb; sleep 1.5
expect "unmounted drive disappears" "1" sh -c 'trawlr --count usbfile >/dev/null; echo $?'
mount -t ext4 /dev/vdd /mnt/usb; sleep 1.5
echo x > /mnt/usb/second-time.txt; settle
expect "plugging it in again works too" "/mnt/usb/photos/added-later.jpg
/mnt/usb/second-time.txt" trawlr added-later\|second-time
umount /mnt/usb; sleep 1

echo "--- who may search"
expect "an allowed user can search" "/mnt/ext4/data/docs/Report.PDF" as_user 1000 trawlr report
expect "and sees names in folders closed to them" "/mnt/ext4/private/secret.txt" as_user 1000 trawlr secret.txt
expect "a member of an allowed group can search" "/mnt/ext4/data/docs/Report.PDF" as_user 1002 trawlr report
expect "another user is refused" "2" sh -c '/ul/setpriv --reuid 1001 --regid 1001 --clear-groups trawlr report >/dev/null 2>&1; echo $?'

echo "--- restart"
stop_daemon
grep -q 'saved index' /run/log && ok "index saved on shutdown" || bad "index not saved"
echo x > /mnt/ext4/data/while-down.txt; rm /mnt/ext4/data/docs/Report.PDF; mkdir /mnt/ext4/data/newdir-down; echo x > /mnt/ext4/data/newdir-down/f.txt
trawlrd >> /run/log 2>&1 &
DPID=$!
wait_ready
expect "file created while stopped is found" "/mnt/ext4/data/while-down.txt" trawlr while-down
expect "folder created while stopped is found" "/mnt/ext4/data/newdir-down/f.txt" trawlr wfn:f.txt
expect "file deleted while stopped is gone" "1" sh -c 'trawlr --count report >/dev/null; echo $?'
grep 'loaded\|checked' /run/log | tail -2 | sed 's/^<.>/  log: /'
echo x > /mnt/ext4/data/after-restart.txt; settle
expect "watching resumed after restart" "/mnt/ext4/data/after-restart.txt" trawlr after-restart
stop_daemon

echo "=== B. confined like the systemd unit: two capabilities, read-only view"
rm -f /var/lib/trawlrd/*
# What ProtectSystem=strict + ProtectHome=read-only amount to: a private
# mount namespace in which every mount is read-only except the state and
# runtime directories. Changes made outside it must still be seen.
cat > /run/confined.sh <<'EOS'
/ul/mount --bind /var/lib/trawlrd /var/lib/trawlrd
for m in /mnt/ext4 /mnt/btrfs /mnt/vfat /; do grep -q " $m " /proc/mounts && /ul/mount -o remount,bind,ro $m; done
exec /ul/setpriv --bounding-set -all,+sys_admin,+dac_read_search --inh-caps -all /repo/target/release/trawlrd
EOS
/ul/unshare --mount --propagation slave /bin/busybox sh /run/confined.sh > /run/log 2>&1 &
DPID=$!
wait_ready
PID=$DPID
expect "process holds only the two capabilities" "CapEff:	0000000000200004" grep CapEff /proc/$PID/status
expect "and its view of the disk is read-only" "ro" sh -c "grep ' /mnt/ext4 ' /proc/$PID/mounts | cut -d' ' -f4 | cut -d, -f1"
trawlr --status | grep -q '/mnt/ext4 (ext4): watched filesystem' && ok "filesystem-wide watch works when confined" || bad "no filesystem-wide watch when confined"
expect "reads folders it has no permission bits for" "/mnt/ext4/private/secret.txt" trawlr secret.txt
echo x > /mnt/ext4/data/outside-ns.txt; settle
expect "sees changes made outside its namespace" "/mnt/ext4/data/outside-ns.txt" trawlr outside-ns
mount -t ext4 /dev/vdd /mnt/usb; sleep 1.5
expect "sees a drive mounted outside its namespace" "/mnt/usb/usbfile.txt" trawlr usbfile
echo x > /mnt/usb/confined-live.txt; settle
expect "and watches it" "/mnt/usb/confined-live.txt" trawlr confined-live
umount /mnt/usb 2>&1 | head -2; sleep 1.5
expect "drive can be unmounted while indexed" "0" sh -c 'grep -c " /mnt/usb " /proc/mounts'
expect "and disappears from the index" "1" sh -c 'trawlr --count usbfile >/dev/null; echo $?'
stop_daemon
grep -q 'saved index' /run/log && ok "confined daemon saved its index" || { bad "confined daemon could not save"; tail -5 /run/log; }
grep -i 'warn\|error\|cannot\|denied' /run/log | head -5

echo "=== C. randomized stress as root (filesystem-wide marks)"
python3 /repo/tests/stress.py --rounds 12 --ops 300 --seed 11 --dir /mnt/ext4 | grep -v '^  ok' || true
[ "$(python3 /repo/tests/stress.py --rounds 12 --ops 300 --seed 12 --dir /mnt/ext4 | tail -1)" = PASS ] && ok "stress on ext4" || bad "stress on ext4"
if [ $HAVE_BTRFS = 1 ]; then
    [ "$(python3 /repo/tests/stress.py --rounds 12 --ops 300 --seed 13 --dir /mnt/btrfs | tee /run/btrfs.out | tail -1)" = PASS ] && ok "stress on btrfs" || { bad "stress on btrfs"; grep -v '^  ok' /run/btrfs.out | head -20; }
fi
[ "$(python3 /repo/tests/stress.py --rounds 8 --ops 200 --seed 14 --dir /mnt/vfat --run-dir /run | tee /run/vfat.out | tail -1)" = PASS ] && ok "stress on vfat" || { bad "stress on vfat"; grep -v '^  ok' /run/vfat.out | head -20; }

echo
if [ $FAILS = 0 ]; then echo "VM-RESULT: PASS"; else echo "VM-RESULT: FAIL ($FAILS)"; fi
sync
poweroff -f
