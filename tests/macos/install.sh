#!/bin/bash
# Install the built package as a user would, check that the launchd daemon
# indexes and answers, render the window, then uninstall and check that
# nothing is left behind. Changes the machine: for CI runners and test Macs.
#   tests/macos/install.sh dist/out/trawlr-*.pkg [screenshot.png]
set -u
pkg=$1
shot=${2:-}
fails=0
ok() { echo "ok    $1"; }
bad() { echo "FAIL  $1"; fails=$((fails + 1)); }
APP=/Applications/Trawlr.app

sudo installer -pkg "$pkg" -target / || { echo "installer failed"; exit 1; }
[ -x "$APP/Contents/MacOS/trawlrd" ] && ok "app installed" || bad "app missing"
[ "$(readlink /usr/local/bin/trawlr)" = "$APP/Contents/MacOS/trawlr" ] && ok "trawlr on PATH" || bad "no /usr/local/bin/trawlr"
sudo launchctl print system/io.trawlr.trawlrd >/dev/null 2>&1 && ok "daemon registered with launchd" || bad "daemon not registered"

deadline=$((SECONDS + 900))
until /usr/local/bin/trawlr --status --json 2>/dev/null | grep -q '"state":"ready"'; do
    if [ $SECONDS -ge $deadline ]; then bad "daemon never became ready"; sudo tail -30 /Library/Logs/Trawlr/trawlrd.log; break; fi
    sleep 2
done
/usr/local/bin/trawlr --status
n=$(/usr/local/bin/trawlr --count "wfn:Safari.app")
[ "${n:-0}" -ge 1 ] && ok "an administrator can search ($n Safari.app)" || bad "search as $(id -un) failed: $n"
n=$(/usr/local/bin/trawlr --count "exe: wfn:Calculator.app")
[ "${n:-0}" -ge 1 ] && ok "apps count as programs ($n)" || bad "exe: finds no Calculator.app"
perm=$(sudo stat -f '%Su %Lp' "/Library/Application Support/Trawlr")
[ "$perm" = "root 700" ] && ok "index folder is root-only" || bad "index folder is $perm"
sudo test -f "/Library/Application Support/Trawlr/config.toml" && ok "default configuration written" || bad "no config.toml"
sudo grep -q "listening on /var/run/trawlrd/trawlrd.sock" /Library/Logs/Trawlr/trawlrd.log && ok "logging to /Library/Logs/Trawlr" || bad "log missing"
/usr/local/bin/trawlr --version >/dev/null && ok "trawlr runs through the link"

if [ -n "$shot" ]; then
    "$APP/Contents/MacOS/trawlr-gui" --screenshot "$shot" Safari.app && ok "window rendered to $shot" || bad "window did not render"
fi

# Upgrading over an installed copy keeps it working.
sudo installer -pkg "$pkg" -target / >/dev/null || bad "reinstall failed"
sleep 3
deadline=$((SECONDS + 120))
until /usr/local/bin/trawlr --status --json 2>/dev/null | grep -q '"state":"ready"'; do
    if [ $SECONDS -ge $deadline ]; then bad "not ready after reinstall"; break; fi
    sleep 2
done
sudo grep -q "loaded saved index" /Library/Logs/Trawlr/trawlrd.log && ok "reinstall kept the index" || bad "reinstall rebuilt the index"

sudo "$APP/Contents/Resources/uninstall.sh"
for p in "$APP" "/Library/Application Support/Trawlr" /Library/Logs/Trawlr /Library/LaunchDaemons/io.trawlr.trawlrd.plist /usr/local/bin/trawlr; do
    [ -e "$p" ] || [ -L "$p" ] && bad "left behind: $p"
done
sudo launchctl print system/io.trawlr.trawlrd >/dev/null 2>&1 && bad "daemon still registered"
pkgutil --pkgs | grep -q io.trawlr && bad "package receipt left"
pgrep -x trawlrd >/dev/null && bad "trawlrd still running"
echo "failures: $fails"
[ $fails = 0 ]
