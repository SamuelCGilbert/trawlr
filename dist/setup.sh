#!/bin/sh
# Second half of install.sh: copy files into place and start the service.
# Usage: setup.sh system|user <source dir> <user to allow> <gui 0|1>
set -eu
MODE=$1 SRC=$2 WHO=$3 GUI=$4

if [ "$MODE" = system ]; then
    BIN=/usr/local/bin
    SHARE=/usr/local/share
    UNITS=/etc/systemd/system
    CONFIGS=/etc
    STATE=/var/lib
    SYSTEMCTL="systemctl"
else
    BIN=$HOME/.local/bin
    SHARE=${XDG_DATA_HOME:-$HOME/.local/share}
    UNITS=${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user
    CONFIGS=${XDG_CONFIG_HOME:-$HOME/.config}
    STATE=${XDG_CACHE_HOME:-$HOME/.cache}
    SYSTEMCTL="systemctl --user"
fi
UNIT=$UNITS/trawlrd.service
CONF=$CONFIGS/trawlrd/config.toml

# Trawlr used to be called "Everything for Linux" (everythingd, es, everything).
# Stop that service, keep its configuration and saved index under the new
# names, and remove its files. Programs are only removed if they are ours.
if [ -e "$UNITS/everythingd.service" ]; then
    echo "==> Replacing Everything for Linux (the old name) with Trawlr"
    $SYSTEMCTL disable --now everythingd >/dev/null 2>&1 || true
    rm -f "$UNITS/everythingd.service"
fi
if [ -d "$CONFIGS/everythingd" ] && [ ! -e "$CONFIGS/trawlrd" ]; then
    mv "$CONFIGS/everythingd" "$CONFIGS/trawlrd"
    [ -f "$CONF" ] && sed -i -e 's/restart everythingd/restart trawlrd/' -e 's/^# everythingd configuration/# trawlrd configuration/' "$CONF"
    echo "    moved $CONFIGS/everythingd to $CONFIGS/trawlrd"
fi
if [ -d "$STATE/everythingd" ] && [ ! -e "$STATE/trawlrd" ]; then
    mv "$STATE/everythingd" "$STATE/trawlrd"
fi
for old in everythingd es everything; do
    if grep -qs everythingd "$BIN/$old"; then rm -f "$BIN/$old"; fi
done
rm -f "$SHARE/applications/everything-for-linux.desktop" "$SHARE/applications/everything-for-linux-new-window.desktop" \
    "$SHARE/applications/everything-for-linux-toggle.desktop" "$SHARE/icons/hicolor/scalable/apps/everything-for-linux.svg"

install -Dm755 "$SRC/target/release/trawlrd" "$BIN/trawlrd"
install -Dm755 "$SRC/target/release/trawlr" "$BIN/trawlr"
if [ "$GUI" = 1 ]; then
    install -Dm755 "$SRC/gui/build/trawlr-gui" "$BIN/trawlr-gui"
    install -Dm644 "$SRC/assets/app-icon.svg" "$SHARE/icons/hicolor/scalable/apps/io.trawlr.Trawlr.svg"
    for f in io.trawlr.Trawlr io.trawlr.Trawlr-new-window io.trawlr.Trawlr-toggle; do
        install -Dm644 "$SRC/dist/$f.desktop" "$SHARE/applications/$f.desktop"
        sed -i -e "s|^Exec=trawlr-gui$|Exec=$BIN/trawlr-gui|" -e "s|^Exec=trawlr-gui |Exec=$BIN/trawlr-gui |" "$SHARE/applications/$f.desktop"
    done
    command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q "$SHARE/applications" 2>/dev/null || true
    command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -qtf "$SHARE/icons/hicolor" 2>/dev/null || true
fi

if [ "$MODE" = system ]; then
    install -Dm644 "$SRC/dist/trawlrd.service" "$UNIT"
else
    install -Dm644 "$SRC/dist/trawlrd-user.service" "$UNIT"
    sed -i "s|^ExecStart=.*|ExecStart=$BIN/trawlrd|" "$UNIT"
fi

if [ ! -e "$CONF" ]; then
    mkdir -p "$(dirname "$CONF")"
    cp "$SRC/dist/config.toml" "$CONF"
    if [ "$MODE" = system ]; then
        [ "$WHO" = root ] || sed -i "s|^allowed_users = \[\]|allowed_users = [\"$WHO\"]|" "$CONF"
        # Backup snapshots repeat every file name on the system, often many
        # times over. Leave the ones we can recognise out of the index.
        skip=
        for mp in $(findmnt -rn -o TARGET -t ext2,ext3,ext4,btrfs,xfs,f2fs,zfs,bcachefs 2>/dev/null); do
            base=${mp%/}
            [ -d "$base/timeshift/snapshots" ] && skip="$skip, \"$base/timeshift\""
            [ -d "$base/.snapshots" ] && skip="$skip, \"$base/.snapshots\""
        done
        if [ -n "$skip" ]; then
            sed -i "s|^#exclude = .*|exclude = [${skip#, }]|" "$CONF"
            echo "    not indexing backup snapshots: ${skip#, }  (edit $CONF to change)"
        fi
    else
        sed -i 's|^allowed_users = \[\]|#allowed_users = []|' "$CONF"
    fi
    echo "    wrote $CONF"
else
    echo "    keeping existing $CONF"
    if [ "$MODE" = system ] && [ "$WHO" != root ] && ! grep -q "\"$WHO\"\|\"\\*\"" "$CONF"; then
        echo "    note: $WHO is not in allowed_users there and will not be able to search"
    fi
fi

if [ -n "${TRAWLR_NO_START:-}" ]; then
    echo "==> Installed (service not started)"
    exit 0
fi

$SYSTEMCTL daemon-reload
$SYSTEMCTL enable trawlrd >/dev/null 2>&1 || true
$SYSTEMCTL restart trawlrd

# Wait for it to answer, then show what it is doing.
i=0
while [ $i -lt 50 ]; do
    if "$BIN/trawlr" --status >/dev/null 2>&1; then
        echo "==> trawlrd is running"
        "$BIN/trawlr" --status | sed 's/^/    /'
        echo
        echo "The first scan of the disk takes from a few seconds to a few minutes;"
        echo "after that the index is kept current as files change."
        [ "$GUI" = 1 ] && echo "Open \"Trawlr\" from the application menu, or run: trawlr"
        echo "Command line: trawlr --help (trawlr on its own opens the window)"
        exit 0
    fi
    sleep 0.2
    i=$((i + 1))
done
echo "trawlrd did not come up. Its log:" >&2
if [ "$MODE" = system ]; then journalctl -u trawlrd -n 30 --no-pager >&2; else journalctl --user -u trawlrd -n 30 --no-pager >&2; fi
exit 1
