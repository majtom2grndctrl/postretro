#!/bin/bash
# Task 9l re-measure (9k's copy-records.sh, re-rooted): copies the small records of each config
# (cap26 cap19) and the C18 sweep (c18) from target/occluder-9l/
# into this folder. shreach.out keeps its summary only; bake logs become bake-extract.txt.
# Raw .sweep/.bakegap records and PRLs stay in target/.
# usage: copy-records.sh <config>... c18
set -u
T=/Users/dhiester/Projects/Personal/postretro/target/occluder-9l
N=$(cd "$(dirname "$0")" && pwd)
for C in "$@"; do
  if [ $C = c18 ]; then mkdir -p $N/c18; cp $T/c18/*.out $N/c18/; continue; fi
  for D in $T/$C/*/; do
  M=$(basename $D); O=$N/$C/$M; mkdir -p $O
  for f in sweep.out bakegap.out structure.out diff-sweep.out diff-bakegap.out mandatory.out rooftop-sweep.out region-arena.out region-stairs.out bake.time; do
    [ -f $D/$f ] && cp $D/$f $O/
  done
  for u in uncapped-street uncapped-rooftop; do  # summary plus the 40 costliest poses
    [ -f $D/$u.txt ] && { grep '^summary' $D/$u.txt; grep '^pose' $D/$u.txt | sed 's/considered=//' | sort -k3,3nr | head -40 | sed 's/^pose \([^ ]*\) \([0-9]*\)/pose \1 considered=\2/'; } > $O/$u.top.txt
  done
  [ -f $D/shreach.out ] && sed '/^## Per-cell rows/,$d' $D/shreach.out > $O/shreach.out
  [ -f $D/bake.log ] && grep -E 'Occluder classification|Shadow prisms|Door recognition|Runtime cells|seals generated|Exterior flood|Watertightness|leak|unsealed|Total faces|cutting face|CellResidencySet|BSP leaves' $D/bake.log | grep -v 'open edge near' | cut -c1-400 > $O/bake-extract.txt
  done
done
