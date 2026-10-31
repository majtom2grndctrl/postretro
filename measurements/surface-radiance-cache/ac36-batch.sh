#!/bin/bash
# AC 36 batch: cold Build Summary, before then after, per map. Aborts on first failure.
cd "$(dirname "$0")" || exit 1
M=content/dev/maps   # run-bake.sh runs from the repo root
run() { ./run-bake.sh "$@" || { echo "ABORT on $3"; exit 1; }; }
for m in campaign-test stress-warren-mini stress-warren-hallway-inspection-mini; do
  run before $M/$m.map $m-before
  run after  $M/$m.map $m-after
done
# second campaign-test pair
run before $M/campaign-test.map campaign-test-before-2
run after  $M/campaign-test.map campaign-test-after-2
echo BATCH-DONE
