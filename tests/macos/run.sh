#!/bin/bash
# Index the whole Mac as root (with a scratch database and socket, so an
# installed copy is not disturbed), then run the live checks, check that the
# startup disk's data volume is not indexed twice, and that a restart, clean
# or not, catches up with changes made while trawlrd was stopped.
#   tests/macos/run.sh            (needs sudo; used by CI)
set -u
BIN=${BIN:-$(cd "$(dirname "$0")/../.." && pwd)/target/release}
run=$(mktemp -d /tmp/trawlr-run.XXXXXX)
chmod 755 "$run"
printf 'allowed_users = ["*"]\n' > "$run/config.toml"
export TRAWLRD_SOCKET=$run/s
T=$BIN/trawlr
fails=0
ok() { echo "ok    $1"; }
bad() { echo "FAIL  $1"; fails=$((fails + 1)); }

start() {
    sudo "$BIN/trawlrd" --config "$run/config.toml" --root / --db "$run/index.db" --socket "$run/s" >>"$run/log" 2>&1 &
    local deadline=$((SECONDS + ${1:-900}))
    until "$T" --status --json 2>/dev/null | grep -q '"state":"ready"'; do
        if [ $SECONDS -ge $deadline ]; then echo "trawlrd did not become ready"; tail -50 "$run/log"; exit 1; fi
        sleep 1
    done
}
stop() { # signal
    sudo pkill -"$1" -f "trawlrd --config $run/config.toml"
    while pgrep -f "trawlrd --config $run/config.toml" >/dev/null; do sleep 0.2; done
}
expect() { # what query count [seconds]
    local got deadline=$((SECONDS + ${4:-10}))
    while :; do
        got=$("$T" --count "$2")
        [ "$got" = "$3" ] && break
        [ $SECONDS -ge $deadline ] && break
        sleep 0.2
    done
    if [ "$got" = "$3" ]; then ok "$1"; else bad "$1 : $2 gave $got, expected $3"; fi
}

t0=$SECONDS
start
echo "first scan took $((SECONDS - t0))s"
"$T" --status
grep -v '^<7>' "$run/log" | tail -5

"$(dirname "$0")/live.sh" "$T" "$HOME/trawlr-live" || fails=$((fails + 1))

expect 'the data volume is not indexed twice' "/System/Volumes/Data/Users/" 0
n=$("$T" --count "wfn:Safari.app")
[ "$n" -ge 1 ] && ok "apps are indexed ($n Safari.app)" || bad "no Safari.app"
n=$("$T" --count "$HOME/")
[ "$n" -ge 1 ] && ok "home folder is indexed ($n entries)" || bad "home folder missing"

tag=zq$RANDOM$RANDOM
stop TERM
grep -q "saved index" "$run/log" && ok "saved on shutdown" || bad "not saved on shutdown"
echo offline > "$HOME/$tag-offline.txt"
mkdir -p "$HOME/$tag-dir/sub" && echo y > "$HOME/$tag-dir/sub/$tag-inside.txt"
start 120
grep -q "loaded saved index" "$run/log" && ok "restart used the saved index" || bad "restart rescanned"
expect 'file made while stopped' "$tag-offline.txt" 1 30
expect 'folder made while stopped' "$tag-inside.txt" 1 30

stop KILL
rm "$HOME/$tag-offline.txt"
mv "$HOME/$tag-dir" "$HOME/$tag-moved"
start 120
expect 'deleted after a crash' "$tag-offline.txt" 0 30
expect 'moved after a crash' "$tag-moved/sub/$tag-inside.txt" 1 30
rm -rf "$HOME/$tag-moved"
expect 'cleaned up' "$tag" 0

stop TERM
echo "--- log (last lines)"; grep -v '^<7>' "$run/log" | tail -15
echo "failures: $fails"
[ $fails = 0 ]
