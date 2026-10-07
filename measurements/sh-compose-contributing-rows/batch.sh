#!/bin/zsh
# Interleaved old/new membership batch for sh-compose-contributing-rows (from
# sh-compose-row-cost-spike/batch.sh). Display held awake for
# the whole batch; 15 s idle gap before every launch; each run is one Metal
# System Trace after RUN_TRACE_AFTER timing windows, exported, reduced by
# gpu_time.py and deleted at once (traces are ~1 GB each).
# usage: batch.sh <batch-key> <rounds> "old new" <map.prl> [engine args...]
# env (required): SPIKE_BIN (dir holding the probe postretro + scripts-build),
#      RUN_FOREGROUND (foreground helper, read by run.py), TRACE_DIR.
# env (optional): PAIRED=1 (per-frame A/B inside one launch via
#      POSTRETRO_SPIKE_ARMS_B; every reported delta uses this),
#      RUN_TRACE_AFTER (default 3 timing windows before the trace).
set -eu
here=${0:A:h}; runs=$here/runs
mkdir -p $runs $here/raw
key=$1; rounds=$2; arms=(${=3}); map=$4; shift 4
caffeinate -d -i -u -t 14400 &
caf=$!
# Re-declare user activity every few seconds so the idle lock never arms.
( while true; do caffeinate -u -t 5; done ) &
active=$!
trap "kill $caf $active 2>/dev/null; pkill -P $active caffeinate 2>/dev/null" EXIT
ioreg -c IOAccelerator -r -d1 | grep -E 'inUseVidMemoryBytes|vramFreeBytes|Device Utilization %' \
  > $runs/$key-idle.ioreg.txt
invalid=0
for round in $(seq 1 $rounds); do
  for arm in $arms; do
    sleep 15
    label=$key-${arm//\//_vs_}-r$round
    marker=$(mktemp)
    # Arms: "old" restores whole-domain compose membership through the probe
    # toggle; "new" is the entry-carrying membership under test.
    if [[ $arm == old ]]; then export POSTRETRO_SPIKE_OLD_MEMBERSHIP=1; else unset POSTRETRO_SPIKE_OLD_MEMBERSHIP; fi
    unset POSTRETRO_SPIKE_ARMS_B
    if POSTRETRO_SPIKE_ARMS=baseline RUN_TRACE=1 RUN_TRACE_AFTER=${RUN_TRACE_AFTER:-3} \
        RUN_TRACE_DIR=$TRACE_DIR python3 $here/run.py $label $SPIKE_BIN/postretro $map "$@" > /dev/null; then
      :
    else
      invalid=$((invalid + 1))
      echo "invalid run $label" >&2
    fi
    trace=$TRACE_DIR/$label.trace
    xml=$TRACE_DIR/$label-gpu.xml
    if [[ -d $trace ]]; then
      # A failed export or reduction must not strand a ~1 GB trace.
      { xcrun xctrace export --input $trace \
          --xpath '/trace-toc/run[@number="1"]/data/table[@schema="metal-gpu-intervals"]' \
          --output $xml > /dev/null \
        && python3 $here/gpu_time.py $xml $runs/$label-gpu.json > /dev/null \
        && gzip -c $xml > $here/raw/$label-gpu.xml.gz; } || echo "reduction failed: $label" >&2
      rm -rf $trace $xml
    fi
    find ${TMPDIR:-/tmp} -maxdepth 1 -name 'instruments*.ktrace' -newer $marker -delete 2>/dev/null
    rm -f $marker
    # Stop the batch at the first run that fails foreground/lock/saver checks:
    # a machine-state problem invalidates the rest of the interleave too.
    if [[ -f $runs/$label.run.json ]] && python3 -c "import json,sys; r=json.load(open('$runs/$label.run.json')); sys.exit(0 if (r['foreground_all'] and not r['screensaver_seen'] and not r['locked_seen']) else 1)"; then
      :
    else
      echo "batch $key stopped at $label (machine state)" >&2
      exit 4
    fi
  done
done
echo "batch $key done ($invalid invalid)"
