#!/bin/bash
# Build the A/B PRLs with --release (ship mode: no cache) into content/dev/maps/ so
# `postretro-tool run` finds them as maps/<name>.prl. *.prl is gitignored.
# Needs target/release/prl-build (cargo build --release -p postretro-level-compiler).
# Run from anywhere; one bake at a time. Takes ~30 s each for campaign-test, ~1 min the room.
set -eu
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$ROOT"
MAPS=measurements/surface-radiance-cache/ab/maps
# The campaign variants are campaign-test.map with one worldspawn line added after
# `"wad" ""`; they are generated here (and gitignored) rather than committed.
for f in 0 0.5 1; do
  awk -v f="$f" '{print} !d && $0=="\"wad\" \"\"" {print "\"_bounce_fill\" \"" f "\""; d=1}' \
    content/dev/maps/campaign-test.map > "$MAPS/ab-campaign-fill-$f.map"
  grep -q "\"_bounce_fill\" \"$f\"" "$MAPS/ab-campaign-fill-$f.map" \
    || { echo "build.sh: could not insert _bounce_fill into campaign-test.map" >&2; exit 1; }
done
for m in "$MAPS"/*.map; do
  n="$(basename "$m" .map)"
  caffeinate -dims target/release/prl-build "$m" -o "content/dev/maps/$n.prl" --release --no-tui
done
ls -la content/dev/maps/ab-*.prl
