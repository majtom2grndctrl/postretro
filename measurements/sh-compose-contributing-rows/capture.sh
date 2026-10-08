#!/bin/zsh
# Atlas byte check for sh-compose-contributing-rows. For each spike capture
# view and time, dumps both composed atlases under old (whole-domain) and new
# (entry-carrying) membership, gated and force-full-resident, then compares:
#   new vs old, gated  — the exactness claim (zero-entry rows keep their bytes)
#   new vs old, full   — control
#   new gated vs new full — informational: lag outside the gate may differ
# Dumps (~0.3 GB each) are deleted after comparison.
# usage: capture.sh   (env SPIKE_BIN: dir holding the probe postretro)
set -eu
here=${0:A:h}; root=${here:h:h}
spike=$root/measurements/sh-compose-row-cost-spike
out=$here/capture/out
mkdir -p $out
fail=0
for view in animroom arena kinematicmid; do
  for t in t050 t100; do
    for mode in gated full; do
      for arm in old new; do
        name=$view-$mode-$t
        dir=$out/$arm/$name
        mkdir -p $dir
        python3 - $spike/capture/$name.scene.json $dir <<'PY'
import json, sys
scene = json.load(open(sys.argv[1])); d = sys.argv[2]
scene["output"] = f"{d}/capture.png"; scene["measurement"]["report"] = f"{d}/report.json"
json.dump(scene, open(f"{d}/scene.json", "w"), indent=2)
PY
        if [[ $arm == old ]]; then export POSTRETRO_SPIKE_OLD_MEMBERSHIP=1; else unset POSTRETRO_SPIKE_OLD_MEMBERSHIP; fi
        (cd $root && unset POSTRETRO_SPIKE_ARMS_B && POSTRETRO_SH_STREAMING=sync-proof POSTRETRO_SPIKE_ARMS=baseline \
           POSTRETRO_SPIKE_ATLAS_DUMP=$dir RUST_LOG=info \
           $SPIKE_BIN/postretro --capture $dir/scene.json > $dir/capture.log 2>&1) \
          || { echo "capture failed: $arm $name" >&2; exit 1; }
        grep -q "compose membership: $([[ $arm == old ]] && echo whole-domain || echo entry-carrying)" $dir/capture.log \
          || { echo "wrong membership arm: $arm $name" >&2; exit 1; }
      done
      cmpdir=$out/compare/$view-$mode-$t; mkdir -p $cmpdir
      if python3 $spike/compare_atlas.py $out/old/$view-$mode-$t $out/new/$view-$mode-$t > $cmpdir/new-vs-old.json; then
        echo "$view-$mode-$t new vs old identical"
      else
        echo "$view-$mode-$t new vs old DIFFERS"; fail=1
      fi
    done
    python3 $spike/compare_atlas.py $out/new/$view-gated-$t $out/new/$view-full-$t > $out/compare/$view-gated-$t/new-gated-vs-full.json \
      && echo "$view-$t new gated vs full identical" || echo "$view-$t new gated vs full differ (informational)"
    rm -f $out/{old,new}/$view-*-$t/*.bin
  done
done
echo "atlas check done (new-vs-old differs=$fail)"
exit $fail
