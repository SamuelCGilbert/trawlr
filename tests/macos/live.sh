#!/bin/bash
# Live-update checks for trawlrd on macOS: files and folders created,
# renamed, moved, hard-linked and deleted in a scratch folder must show up in
# (and leave) the index within seconds. Needs a running trawlrd that indexes
# the folder $2 is in (TRAWLRD_SOCKET set if it is not the system one).
#   tests/macos/live.sh [path/to/trawlr] [scratch folder, default ~/trawlr-live]
T=${1:-target/release/trawlr}
root=${2:-$HOME/trawlr-live}
leaf=$(basename "$root")
rm -rf "$root"
fails=0
q() { "$T" --count "$1"; }
expect() { # what query count
    local got deadline=$((SECONDS + 5))
    while :; do
        got=$(q "$2")
        [ "$got" = "$3" ] && break
        [ $SECONDS -ge $deadline ] && break
        sleep 0.1
    done
    if [ "$got" = "$3" ]; then echo "ok    $1"; else echo "FAIL  $1 : $2 gave $got, expected $3"; fails=$((fails + 1)); fi
}
tag=zq$RANDOM$RANDOM
mkdir -p "$root/a/b"
echo hello > "$root/a/b/$tag-one.txt"
expect 'new file in new folders' "$tag-one.txt" 1
expect 'its path' "$leaf/a/b/$tag-one.txt" 1
mv "$root/a/b/$tag-one.txt" "$root/a/b/$tag-two.txt"
expect 'rename: old name gone' "$tag-one.txt" 0
expect 'rename: new name there' "$tag-two.txt" 1
for i in $(seq 1 50); do echo x > "$root/a/b/$tag-many-$i.dat"; done
expect 'fifty files' "$tag-many-" 50
mv "$root/a/b" "$root/a/c"
expect 'folder rename moves children' "$leaf/a/c/$tag-many-" 50
expect 'old folder path empty' "$leaf/a/b/$tag-many-" 0
mkdir -p "$root/x/y"
mv "$root/a/c" "$root/x/y/c"
expect 'folder move moves children' "$leaf/x/y/c/$tag-many-" 50
expect 'nothing left at old place' "$leaf/a/c/" 0
head -c 5000 /dev/zero >> "$root/x/y/c/$tag-two.txt"
sleep 0.8
size=$("$T" --json "$tag-two.txt" | sed -n 's/.*"s":\([0-9]*\).*/\1/p')
if [ "${size:-0}" -gt 5000 ]; then echo "ok    size follows writes ($size)"; else echo "FAIL  size is $size"; fails=$((fails + 1)); fi
ln "$root/x/y/c/$tag-two.txt" "$root/x/$tag-link.txt"
expect 'hard link appears' "$tag-link.txt" 1
expect 'original still there' "$tag-two.txt" 1
rm "$root/x/$tag-link.txt"
expect 'hard link removed' "$tag-link.txt" 0
expect 'original kept' "$tag-two.txt" 1
ln -s "$root/x/y" "$root/x/$tag-symlink"
expect 'symlink appears' "$tag-symlink" 1
mv "$root/x/y/c/$tag-two.txt" "$root/x/y/c/$tag-TWO.txt"
sleep 0.8
names=$("$T" "$tag-two.txt")
if [ "$(echo "$names" | grep -c .)" = 1 ] && echo "$names" | grep -q "$tag-TWO.txt"; then echo "ok    case-only rename"; else echo "FAIL  case-only rename: $names"; fails=$((fails + 1)); fi
mv "$root/x/y/c" "$root/x/y/C"
expect 'case-only folder rename' "$leaf/x/y/C/$tag-many-" 50
rm "$root/x/y/C/$tag-many-1.dat"; echo again > "$root/x/y/C/$tag-many-1.dat"
expect 'delete then recreate same name' "$tag-many-1.dat" 1
mkdir "$root/fast"; for i in $(seq 1 10); do echo f > "$root/fast/$tag-fast-$i"; done; mkdir "$root/fast2"; mv "$root/fast" "$root/fast2/fast"
expect 'quick create-fill-move' "$leaf/fast2/fast/$tag-fast-" 10
name_u=$(printf 'caf\xc3\xa9-%s.txt' "$tag")
echo u > "$root/$name_u"
expect 'accented name' "$name_u" 1
rm -rf "$root/x"
expect 'tree delete' "$tag-many-" 0
expect 'tree delete (case-renamed)' "$tag-two.txt" 0
rm -rf "$root"
expect 'scratch folder gone' "$tag" 0
echo "failures: $fails"
[ $fails = 0 ]
