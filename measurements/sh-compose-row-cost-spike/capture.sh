#!/bin/zsh
# Dump composed SH atlases for one arm across every capture scene.
# usage: capture.sh <arm>   (env SPIKE_BIN). Output: capture/out/<arm>/<scene>/
# For any arm but baseline, each scene is compared with capture/out/baseline
# at once (compare.json) and its .bin dumps are deleted: one dump set is ~3.6 GB.
set -eu
here=${0:A:h}; root=${here:h:h}
arm=$1
fail=0
for scene in $here/capture/*.scene.json; do
  name=${scene:t:r:r}
  dir=$here/capture/out/$arm/$name
  mkdir -p $dir
  (cd $root && POSTRETRO_SH_STREAMING=sync-proof POSTRETRO_SPIKE_ARMS=${${arm%@*}//+/,} \
     POSTRETRO_SPIKE_ATLAS_DUMP=$dir RUST_LOG=info \
     $SPIKE_BIN/postretro --capture $scene > $dir/capture.log 2>&1) || { echo "capture failed: $arm $name" >&2; exit 1; }
  mv $here/capture/out/$name.png $here/capture/out/$name.report.json $dir/ 2>/dev/null || true
  if [[ $arm != baseline ]]; then
    if python3 $here/compare_atlas.py $here/capture/out/baseline/$name $dir > $dir/compare.json; then
      echo "$name identical"
    else
      echo "$name DIFFERS"; fail=1
    fi
    rm -f $dir/*.bin
  fi
done
echo "captured $arm (differs=$fail)"
