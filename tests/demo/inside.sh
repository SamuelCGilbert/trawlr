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
exclude = ["/src", "/out", "/proc", "/sys", "/dev", "/tmp", "/run", "/var/lib/trawlrd"]
CONF

# Building the image and the steps above touched system files today; on a
# real machine those would be weeks old, so the newest files are alex's.
mkdir -p /var/lib/trawlrd /run/trawlrd
find / -xdev -newermt '-2 days' -exec touch -h -d '40 days ago' {} + 2>/dev/null || true

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
    # Settings and caches outside the home folder (and out of the index),
    # so the window's own files are not the newest things in it.
    rm -rf /tmp/alex
    mkdir -p /tmp/alex/config/trawlr /tmp/alex/cache /tmp/alex/run
    cat >/tmp/alex/config/trawlr/trawlr.conf <<CONF
[General]
trayIcon=false

[view]
theme=$theme
sort=date
descending=true
CONF
    chown -R alex:alex /tmp/alex
    chmod 700 /tmp/alex/run
    runuser -u alex -- env LANG=C.UTF-8 HOME=/home/alex XDG_RUNTIME_DIR=/tmp/alex/run XDG_CONFIG_HOME=/tmp/alex/config \
        XDG_CACHE_HOME=/tmp/alex/cache XDG_DATA_HOME=/tmp/alex/data MESA_SHADER_CACHE_DISABLE=true XDG_CURRENT_DESKTOP=KDE KDE_SESSION_VERSION=6 QT_QPA_PLATFORMTHEME=kde \
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
ffmpeg -loglevel error -y -f x11grab -draw_mouse 0 -framerate 25 -video_size $((W * 2))x$((H * 2)) -i :1+0,0 -t 30 /tmp/demo.mkv &
REC=$!
# Narrow down step by step, the way people search.
t() { xdotool type --delay 70 "$1"; sleep "$2"; }
sleep 0.8
t "invoice" 1.4
t " documents/" 1.4
t " .pdf" 1.4
t " kestrel" 1.4
# An invoice moved out of Downloads shows up while the search is open.
runuser -u alex -- mv "/home/alex/Downloads/invoice-2026-1251 Kestrel Labs.pdf" /home/alex/Documents/Work/Invoices/2026/
sleep 2.2
xdotool key ctrl+a BackSpace
t ".flac" 1.6
kill -TERM $REC   # ffmpeg finishes the file; sh ignores SIGINT in background jobs
wait $REC || true
quit
ffmpeg -loglevel error -y -i /tmp/demo.mkv -vf "fps=12,scale=${W}:-1:flags=lanczos,split[a][b];[a]palettegen=max_colors=128:stats_mode=diff[p];[b][p]paletteuse=dither=none:diff_mode=rectangle" "$OUT/demo.gif"
ls -l "$OUT"
