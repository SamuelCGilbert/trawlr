#!/bin/sh
# Remove Trawlr.
#
#   ./uninstall.sh            remove the system install (asks for sudo)
#   ./uninstall.sh --user     remove a per-user install
#   add --purge to also delete the configuration and the saved index
set -eu
MODE=system
PURGE=0
for arg in "$@"; do
    case $arg in
        --user) MODE=user ;;
        --purge) PURGE=1 ;;
        *) sed -n '2,6p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
    esac
done

if [ $MODE = user ]; then
    systemctl --user disable --now trawlrd 2>/dev/null || true
    share=${XDG_DATA_HOME:-$HOME/.local/share}
    conf=${XDG_CONFIG_HOME:-$HOME/.config}
    rm -f "$HOME/.local/bin/trawlrd" "$HOME/.local/bin/trawlr" "$HOME/.local/bin/trawlr-gui" "$conf/systemd/user/trawlrd.service" \
        "$share/applications/io.trawlr.Trawlr.desktop" "$share/applications/io.trawlr.Trawlr-new-window.desktop" \
        "$share/applications/io.trawlr.Trawlr-toggle.desktop" "$share/icons/hicolor/scalable/apps/io.trawlr.Trawlr.svg"
    systemctl --user daemon-reload 2>/dev/null || true
    [ $PURGE = 1 ] && rm -rf "$conf/trawlrd" "${XDG_CACHE_HOME:-$HOME/.cache}/trawlrd"
    echo "Removed the per-user install."
    exit 0
fi

if [ "$(id -u)" != 0 ]; then
    exec ${SUDO:-sudo} sh "$0" "$@"
fi
systemctl disable --now trawlrd 2>/dev/null || true
rm -f /usr/local/bin/trawlrd /usr/local/bin/trawlr /usr/local/bin/trawlr /usr/local/bin/trawlr-gui /etc/systemd/system/trawlrd.service \
    /usr/local/share/applications/io.trawlr.Trawlr.desktop /usr/local/share/applications/io.trawlr.Trawlr-new-window.desktop \
    /usr/local/share/applications/io.trawlr.Trawlr-toggle.desktop /usr/local/share/icons/hicolor/scalable/apps/io.trawlr.Trawlr.svg
systemctl daemon-reload
if [ $PURGE = 1 ]; then
    rm -rf /etc/trawlrd /var/lib/trawlrd
    echo "Removed Trawlr, its configuration and its index."
else
    echo "Removed Trawlr. Configuration (/etc/trawlrd) and index (/var/lib/trawlrd) were kept; --purge deletes them."
fi
