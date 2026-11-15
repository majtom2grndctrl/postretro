#!/bin/zsh
# bake1.sh <col> <map-name> [src.map]  -- cold --release bake into ws maps
F=/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/9b267032-286c-4f5c-a1c1-c8f51bcf7d53/scratchpad/fable; col=$1; map=$2; src=${3:-/Users/dhiester/Projects/Personal/postretro/content/dev/maps/$map.map}
rm -rf $F/cache/$col-$map; mkdir -p $F/cache/$col-$map
cd /Users/dhiester/Projects/Personal/postretro && nice $F/bin/prl-build $src -o $F/maps/$col-$map.prl --release --no-tui -v --cache-dir $F/cache/$col-$map > $F/logs/bake-$col-$map.log 2>&1; echo "baked $col-$map rc $?"
rm -rf $F/cache/$col-$map
