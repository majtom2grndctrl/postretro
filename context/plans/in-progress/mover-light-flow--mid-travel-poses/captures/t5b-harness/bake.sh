#!/bin/zsh
# Task 5b: bake one experiment's PRLs with the working tree's release prl-build
# (built with the DIAGNOSTIC switches), cold `--release`, then strip id 57's
# depth block for the `-nodepth` diagnostic column.
#   bake.sh <name> [ENV=VALUE ...]     e.g. bake.sh blend POSTRETRO_DIAG_MID_ENCLOSED=blend
set -e
name=$1; shift
SP=/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/b8b805c8-5289-4dc3-bb1d-ebd668f82d89/scratchpad
REPO=/Users/dhiester/Projects/Personal/postretro
OUT=$SP/t5/ws/content/dev/maps
cd $REPO
for map in mover-setpiece enemy-movers; do
  env "$@" RUST_LOG=info nice $SP/t5b/bin/prl-build content/dev/maps/$map.map -o $OUT/$name-$map.prl --release --no-tui -v -j 6 > $SP/t5b/logs/bake-$name-$map.log 2>&1
  echo "baked $name-$map"
done
