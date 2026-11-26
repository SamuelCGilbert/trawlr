#!/bin/sh
# Runs inside the demo container for tests/demo/run.sh: a made-up user's
# home folder, the system daemon indexing the whole container, the window
# on a virtual screen typed into by xdotool. Writes to /out.
set -eu
OUT=/out
mkdir -p $OUT
SCALE=${SCALE:-2}      # the still pictures, for sharp text on high-DPI screens
W=1060 H=660           # window size in logical pixels

step() { echo "==> $*"; }

step making /home/alex
useradd -M -d /home/alex -s /bin/bash alex
usermod -aG trawlr alex
python3 /src/tests/demo/tree.py /home/alex
chown -R alex:alex /home/alex

# The container's own system files are on overlayfs, which the stock
# configuration leaves out; take them in so the index looks like a machine.
cat >>/etc/trawlrd/config.toml <<'CONF'
extra_fs_types = ["overlay"]
exclude = ["/src", "/out", "/proc", "/sys", "/dev"]
CONF

step starting trawlrd
trawlrd >/tmp/trawlrd.log 2>&1 &
for _ in $(seq 300); do
    trawlr --status 2>/dev/null | grep -q ready && break
    sleep 0.2
done
trawlr --status

Xvfb :1 -screen 0 2400x1600x24 -nolisten tcp >/dev/null 2>&1 &
export DISPLAY=:1
for _ in $(seq 50); do xdpyinfo >/dev/null 2>&1 && break; sleep 0.1; done

# Run the window as alex, with fresh settings each time.
gui() {
    theme=$1 scale=$2; shift 2
    rm -rf /home/alex/.config/trawlr
    mkdir -p /home/alex/.config/trawlr /tmp/rt-alex
    cat >/home/alex/.config/trawlr/trawlr.conf <<CONF
[General]
trayIcon=false

[view]
theme=$theme
sort=date
descending=true
CONF
    chown -R alex:alex /home/alex/.config /tmp/rt-alex
    chmod 700 /tmp/rt-alex
    runuser -u alex -- env LANG=C.UTF-8 HOME=/home/alex XDG_RUNTIME_DIR=/tmp/rt-alex XDG_CURRENT_DESKTOP=KDE KDE_SESSION_VERSION=6 QT_QPA_PLATFORMTHEME=kde \
        QT_SCALE_FACTOR="$scale" trawlr-gui "$@" >/tmp/gui.log 2>&1 &
    WID=$(xdotool search --sync --onlyvisible --name "Trawlr\$" | head -n1)
    xdotool windowsize "$WID" $((W * scale)) $((H * scale)) windowmove "$WID" 0 0
    sleep 2
}
quit() { pkill -x trawlr-gui || true; while pgrep -x trawlr-gui >/dev/null; do sleep 0.1; done; }

for theme in dark light; do
    step "still picture ($theme)"
    gui "$theme" "$SCALE" invoice
    import -window "$WID" "$OUT/screenshot-$theme.png"
    quit
done

step recording
# At 2x and scaled down: at 1x the icon theme picks its small monochrome icons.
gui dark 2
ffmpeg -loglevel error -y -f x11grab -draw_mouse 0 -framerate 25 -video_size $((W * 2))x$((H * 2)) -i :1+0,0 -t 15 /tmp/demo.mkv &
REC=$!
sleep 1.2
xdotool type --delay 150 "invoice"
sleep 1.8
# A new invoice arrives while the search is open.
runuser -u alex -- sh -c 'f="/home/alex/Downloads/invoice-2026-1252 Bluebird Studio.pdf"; head -c 81234 /dev/zero >"$f"'
sleep 2.5
xdotool type --delay 150 " 2025"
sleep 2.5
xdotool key ctrl+a BackSpace
xdotool type --delay 70 "ext:flac"
wait $REC
quit
ffmpeg -loglevel error -y -i /tmp/demo.mkv -vf "fps=12,scale=${W}:-1:flags=lanczos,split[a][b];[a]palettegen=max_colors=128:stats_mode=diff[p];[b][p]paletteuse=dither=none:diff_mode=rectangle" "$OUT/demo.gif"
ls -l "$OUT"
