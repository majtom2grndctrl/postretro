#!/bin/zsh
# Interleaved arm batch for sh-compose-row-cost-spike. Display held awake for
# the whole batch; 15 s idle gap before every launch; each run is one Metal
# System Trace after RUN_TRACE_AFTER timing windows, exported, reduced by
# gpu_time.py and deleted at once (traces are ~1 GB each).
# usage: batch.sh <batch-key> <rounds> <arms (space-separated, quoted)> <map.prl> [engine args...]
#   arm names are POSTRETRO_SPIKE_ARMS values; "baseline" means none.
# env: PAIRED=1 runs each arm as the B half of a per-frame A/B pair against
#      the baseline inside one launch (POSTRETRO_SPIKE_ARMS_B);
#      SPIKE_BIN (probe binary dir holding postretro + scripts-build),
#      RUN_FOREGROUND, TRACE_DIR
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
    if [[ ${PAIRED:-0} == 1 ]]; then
      # Paired: A and B alternate per frame. "a/b" names both halves;
      # a bare arm pairs against the baseline.
      if [[ $arm == */* ]]; then
        arm_a=${${arm%%/*}//+/,}; export POSTRETRO_SPIKE_ARMS_B=${${arm#*/}//+/,}
      else
        arm_a=baseline; export POSTRETRO_SPIKE_ARMS_B=${arm//+/,}
      fi
    else
      unset POSTRETRO_SPIKE_ARMS_B; arm_a=${arm//+/,}
    fi
    if POSTRETRO_SPIKE_ARMS=$arm_a RUN_TRACE=1 RUN_TRACE_AFTER=${RUN_TRACE_AFTER:-3} \
        RUN_TRACE_DIR=$TRACE_DIR python3 $here/run.py $label $SPIKE_BIN/postretro $map "$@" > /dev/null; then
      :
    else
      invalid=$((invalid + 1))
      echo "invalid run $label" >&2
    fi
    trace=$TRACE_DIR/$label.trace
    xml=$TRACE_DIR/$label-gpu.xml
    if [[ -d $trace ]]; then
      xcrun xctrace export --input $trace \
        --xpath '/trace-toc/run[@number="1"]/data/table[@schema="metal-gpu-intervals"]' \
        --output $xml > /dev/null
      python3 $here/gpu_time.py $xml $runs/$label-gpu.json > /dev/null
      gzip -c $xml > $here/raw/$label-gpu.xml.gz
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
