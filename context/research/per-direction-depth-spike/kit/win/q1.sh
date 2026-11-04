#!/usr/bin/env bash
# Q1 run. Usage: q1.sh <scratch dir> <step>. Run from the worktree root.
set -u
SP=$1
K=context/research/per-direction-depth-spike/kit/win/closet.py
OUT=$SP/q1
mkdir -p "$SP/tiles" "$OUT"
case $2 in
  tiles)
    for r in 256 1024; do
      for k in static nodoor; do
        PDD_OUT="$SP/tiles/$k-r$r" PDD_RAYS=$r python3 $K bake $k </dev/null 2>&1 | grep -E '\[pdd\]|rror' | sed "s/^/$k r$r: /"
      done
    done
    ;;
  off)
    python3 $K cap off-static static "$OUT" </dev/null
    python3 $K cap off-nodoor nodoor "$OUT" </dev/null
    ;;
  run)
    for t in 4 6 8; do
      python3 $K cap static-t$t-r256 static "$OUT" --env "POSTRETRO_PDD_TILES=$SP/tiles/static-r256.t$t.bin" </dev/null
      python3 $K cap nodoor-t$t-r256 nodoor "$OUT" --env "POSTRETRO_PDD_TILES=$SP/tiles/nodoor-r256.t$t.bin" </dev/null
    done
    ;;
esac
