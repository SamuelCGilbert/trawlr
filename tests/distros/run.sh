#!/bin/bash
# Build, install and test Trawlr from source in clean containers of other
# Linux distributions, the way someone would after cloning the repository:
# their packaged compiler and Qt, then ./install.sh, then the test suites.
#
#   tests/distros/run.sh                      every distribution below
#   tests/distros/run.sh debian:12 fedora:43  only these
#
# Needs docker (Docker Desktop works: DOCKER_CONTEXT=desktop-linux). Logs go
# to $WORK/<distro>.log.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)
WORK=${WORK:-${XDG_CACHE_HOME:-$HOME/.cache}/trawlr-distros}
mkdir -p "$WORK"

DISTROS=("$@")
[ ${#DISTROS[@]} -gt 0 ] || DISTROS=(ubuntu:22.04 ubuntu:24.04 debian:12 debian:13 fedora:latest archlinux:latest opensuse/tumbleweed:latest)

fail=0
for d in "${DISTROS[@]}"; do
    log=$WORK/${d//[:\/]/_}.log
    start=$(date +%s)
    # --privileged: the daemon's fanotify calls are blocked by the default
    # seccomp profile. The container still only sees its own files.
    # The tests' scratch files go on a real (ext4) volume: fanotify does not
    # report changes made inside the container's overlay filesystem.
    if docker run --rm --privileged -v "$REPO:/src:ro" --mount type=volume,dst=/scratch -e TMPDIR=/scratch \
        "$d" sh /src/tests/distros/inside.sh >"$log" 2>&1; then
        printf '%-28s pass  (%ss)  %s\n' "$d" $(( $(date +%s) - start )) "$(grep '^RESULT' "$log" | cut -d' ' -f2- | tr '\n' ' ')"
    else
        printf '%-28s FAIL  (%ss)  see %s\n%s\n' "$d" $(( $(date +%s) - start )) "$log" "$(grep -E '^(STEP|RESULT)' "$log" | tail -3 | sed 's/^/    /')"
        fail=1
    fi
done
exit $fail
