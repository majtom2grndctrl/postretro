#!/bin/bash
# Build the A/B PRLs with --release (ship mode: no cache) into content/dev/maps/ so
# `postretro-tool run` finds them as maps/<name>.prl. *.prl is gitignored.
# Needs target/release/prl-build (cargo build --release -p postretro-level-compiler).
# Run from anywhere; one bake at a time. Takes ~30 s each for campaign-test, ~1 min the room.
set -eu
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$ROOT"
for m in measurements/surface-radiance-cache/ab/maps/*.map; do
  n="$(basename "$m" .map)"
  caffeinate -dims target/release/prl-build "$m" -o "content/dev/maps/$n.prl" --release --no-tui
done
ls -la content/dev/maps/ab-*.prl
