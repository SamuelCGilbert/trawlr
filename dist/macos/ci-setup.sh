#!/bin/bash
# Installs Qt for macOS (universal, x86_64 and arm64) for CI, into ~/Qt, and
# prints the folder to add to PATH.
set -euo pipefail
python3 -m pip install --quiet --break-system-packages aqtinstall==3.3.0 2>/dev/null || python3 -m pip install --quiet aqtinstall==3.3.0
python3 -m aqt install-qt mac desktop 6.8.3 clang_64 --archives qtbase qtsvg -O "$HOME/Qt" >/dev/null
echo "$HOME/Qt/6.8.3/macos/bin"
