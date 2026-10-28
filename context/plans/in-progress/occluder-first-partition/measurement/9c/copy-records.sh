#!/bin/bash
# Copies the small records of each config (and the 9c side runs on before /
# Task 9) from target/occluder-9c/ into this folder. shreach.out keeps its
# summary only (the per-cell CSV is dropped); bake logs become bake-extract.txt.
set -u
T=/Users/dhiester/Projects/Personal/postretro/target/occluder-9c
N=$(cd "$(dirname "$0")" && pwd)
for C in "$@"; do for D in $T/$C/*/; do
  M=$(basename $D); O=$N/$C/$M; mkdir -p $O
  for f in sweep.out bakegap.out structure.out diff-sweep.out diff-bakegap.out mandatory.out probe-reach.out rooftop-sweep.out region-arena.out region-stairs.out bake.time; do
    [ -f $D/$f ] && cp $D/$f $O/
  done
  [ -f $D/shreach.out ] && sed '/^## Per-cell rows/,$d' $D/shreach.out > $O/shreach.out
  [ -f $D/bake.log ] && grep -E 'Occluder classification|Door recognition|Runtime cells|seals generated|Exterior flood|Watertightness|leak|unsealed|Total faces|cutting face|CellResidencySet|BSP leaves' $D/bake.log | grep -v 'open edge near' | cut -c1-400 > $O/bake-extract.txt
done; done
