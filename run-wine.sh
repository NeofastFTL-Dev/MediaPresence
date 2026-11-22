#!/usr/bin/env bash
# Launch NeoPresence from its published build.
#
# Usage:
#   ./run-wine.sh [args...]
#
# Environment:
#   WINEPREFIX   prefix to run in (default: ~/.wine-apps)
#   EXE          executable to launch, relative to this repo (default: the
#                published build produced by ./build-wine.sh)
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
EXE="${EXE:-pc/dist/wine-x64/NeoPresence.exe}"
export WINEPREFIX="${WINEPREFIX:-$HOME/.wine-apps}"

command -v wine >/dev/null || { echo "error: wine is not on PATH" >&2; exit 1; }
if [ ! -f "$HERE/$EXE" ]; then
  echo "error: $EXE does not exist yet - build it with ./build-wine.sh" >&2
  exit 1
fi

# NeoPresence reaches Discord through the Windows named-pipe bridge; without it
# the app still runs but stays "Discord niet verbonden".
if [ ! -f "$WINEPREFIX/drive_c/windows/discord-pipe-bridge.exe" ]; then
  echo "note: discord-pipe-bridge.exe is not installed in this prefix;" >&2
  echo "      NeoPresence will start but stay 'Discord niet verbonden'." >&2
fi

exec wine "$HERE/$EXE" "$@"
