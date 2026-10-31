#!/bin/bash
# Optional extension of the AC 35 sweep to 8N and 16N (128, 256 lights).
cd "$(dirname "$0")" || exit 1
G="--size 8192 --falloff 768 --pillars 6"
python3 gen_light_sweep.py 16 8 sweep/light-sweep-128.map $G
python3 gen_light_sweep.py 16 16 sweep/light-sweep-256.map $G
D=measurements/surface-radiance-cache/sweep
for n in 128 256; do for b in after before; do
  ./run-bake-ts.sh $b $D/light-sweep-$n.map sweep-$n-$b || { echo "ABORT sweep-$n-$b"; exit 1; }
done; done
echo BATCH-DONE
