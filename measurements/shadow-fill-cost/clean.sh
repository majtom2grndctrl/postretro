#!/bin/zsh
# Clean before/after batch: display held awake, 15 s idle gap before every
# launch, alternating builds, screen-saver presence recorded per run.
# usage: clean.sh <map-key> <map.prl> [engine args...]
# env: BEFORE_BIN, AFTER_BIN, RUN_FOREGROUND, TRACE_DIR
set -u
here=${0:A:h}; runs=$here/runs
key=$1; map=$2; shift 2
caffeinate -d -i -u -t 3600 &
caf=$!
trap "kill $caf 2>/dev/null" EXIT
ioreg -c IOAccelerator -r -d1 | grep -E 'inUseVidMemoryBytes|vramFreeBytes|Device Utilization %' \
  > $runs/$key-clean-idle.ioreg.txt
for i in 1 2 3; do
  for build in before after; do
    bin=${build:u}_BIN
    sleep 15
    python3 $here/run.py $key-$build-clean$i ${(P)bin} $map "$@" > /dev/null
  done
done
for build in before after; do
  bin=${build:u}_BIN
  sleep 15
  marker=$(mktemp)
  RUN_TRACE=1 RUN_TRACE_AFTER=3 RUN_TRACE_DIR=$TRACE_DIR \
    python3 $here/run.py $key-$build-cleantrace ${(P)bin} $map "$@" > /dev/null
  trace=$TRACE_DIR/$key-$build-cleantrace.trace
  xml=$TRACE_DIR/$key-$build-cleantrace-gpu.xml
  xcrun xctrace export --input $trace \
    --xpath '/trace-toc/run[@number="1"]/data/table[@schema="metal-gpu-intervals"]' \
    --output $xml > /dev/null
  python3 $here/gpu_time.py $xml $runs/$key-$build-cleantrace-gpu.json > /dev/null
  gzip -c $xml > $here/raw/$key-$build-cleantrace-gpu.xml.gz
  rm -rf $trace $xml
  find ${TMPDIR:-/tmp} -maxdepth 1 -name 'instruments*.ktrace' -newer $marker -delete 2>/dev/null
  rm -f $marker
done
echo "batch $key done"
