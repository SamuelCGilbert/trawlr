#!/bin/sh
# Removes Trawlr completely: the daemon, the app, the index and the settings.
#   sudo "/Applications/Trawlr.app/Contents/Resources/uninstall.sh"
if [ "$(id -u)" != 0 ]; then
    echo "Run it with sudo:  sudo \"$0\"" >&2
    exit 1
fi
APP="/Applications/Trawlr.app"
[ -x "$APP/Contents/MacOS/trawlrd" ] && "$APP/Contents/MacOS/trawlrd" --uninstall-service
pkill -x trawlr-gui 2>/dev/null
rm -rf "/Library/Application Support/Trawlr" /Library/Logs/Trawlr /var/run/trawlrd
[ "$(readlink /usr/local/bin/trawlr)" = "$APP/Contents/MacOS/trawlr" ] && rm -f /usr/local/bin/trawlr
for home in /Users/*; do
    rm -f "$home/Library/LaunchAgents/io.trawlr.Trawlr.plist"
done
rm -rf "$APP"
pkgutil --forget io.trawlr.Trawlr >/dev/null 2>&1
echo "Trawlr is removed."
