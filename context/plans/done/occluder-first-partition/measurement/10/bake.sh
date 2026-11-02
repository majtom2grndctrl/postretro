#!/bin/bash
# usage: bake.sh <binary dir> <out root> <map>...
cd /c/Users/danhi/Projects/Personal/postretro/.claude/worktrees/mac-agent-review-panel-ee30a1
W=$(pwd -W); BIN=$1; G=$2; shift 2
flags_for() { case $1 in stress-warren-hallway-inspection) echo "--sh-probe-spacing 4.0 --no-cache";;
  stress-warren-showcase) echo "--sh-probe-spacing 10.0 --lightmap-density 0.25 --no-cache";;
  stress-street) echo "--sh-probe-spacing 4.0 --lightmap-density 0.25";; *) echo "";; esac; }
bake() { M=$1; O=$G/head/$M; mkdir -p $O; F=$(flags_for $M); CF=""
  case "$F" in *--no-cache*) ;; *) rm -rf $O/cache; CF="--cache-dir $O/cache";; esac; s=$(date +%s)
  { echo "flags: $F $CF"; echo "binary: $(sha1sum $BIN/prl-build.exe|cut -c1-12)"; echo "start: $(date '+%F %T')"; } > $O/bake.time
  RUST_LOG=info $BIN/prl-build.exe content/dev/maps/$M.map -o $O/$M.prl --baked-root $G/baked --no-tui -v $F $CF > $O/bake.log 2>&1
  { echo "exit: $?"; echo "wall_seconds: $(( $(date +%s)-s ))"; } >> $O/bake.time; rm -rf $O/cache
  grep -E "Occluder classification|Door recognition|Shadow prisms:|Runtime cells:|seals generated portal|Exterior flood-fill|boundary leak|unsealed or have a leak" $O/bake.log > $O/bake-extract.txt; cat $O/bake.time; }
for m in "$@"; do bake $m; done
