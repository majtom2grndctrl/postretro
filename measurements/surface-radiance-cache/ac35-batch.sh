#!/bin/bash
# AC 35 light-count sweep: N=16, 2N=32, 4N=64 static lights, same geometry. Cold, one at a time.
# after binary first, then the before binary. Aborts on first failure.
cd "$(dirname "$0")" || exit 1
G="--size 8192 --falloff 768 --pillars 6"
python3 gen_light_sweep.py 4 4 sweep/light-sweep-16.map $G
python3 gen_light_sweep.py 8 4 sweep/light-sweep-32.map $G
python3 gen_light_sweep.py 8 8 sweep/light-sweep-64.map $G
D=measurements/surface-radiance-cache/sweep
for b in after before; do
  for n in 16 32 64; do
    ./run-bake-ts.sh $b $D/light-sweep-$n.map sweep-$n-$b || { echo "ABORT sweep-$n-$b"; exit 1; }
  done
done
echo BATCH-DONE
