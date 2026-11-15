#!/bin/zsh
# Per variant: quiet-machine cold --release bakes (timed), then AC 36 tests.
set -u
R=/Users/dhiester/Projects/Personal/postretro
T=/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/991d9ce0-b6e7-4e9e-8a81-c2d9f7c07b7d/scratchpad/t15b
cd $R
for v in o44 o20 o10 o005; do
  for map in mover-setpiece enemy-movers; do
    while pgrep -x 'cargo|rustc|prl-build' > /dev/null; do sleep 20; done
    rm -rf $T/cache/$v-$map; mkdir -p $T/cache/$v-$map
    /usr/bin/time -p nice $T/bin/$v/prl-build content/dev/maps/$map.map -o $T/ws/content/dev/maps/$v-$map.prl \
      --release --no-tui -v --baked-root $T/ws/baked --cache-dir $T/cache/$v-$map > $T/logs/bake-$v-$map.log 2>&1
    echo "$v $map rc=$? $(grep '^real' $T/logs/bake-$v-$map.log) quiet-after=$(pgrep -x 'cargo|rustc' >/dev/null && echo no || echo yes)"
    rm -rf $T/cache/$v-$map
  done
done
for v in o44 o20 o10 o005; do
  (cd $R/crates/level-compiler && nice $T/bin/$v/prl_build_tests read_the_fine_bake_in_motion --ignored --nocapture > $T/logs/test-$v.log 2>&1)
  echo "test $v rc=$?"
done
echo alldone
