#!/bin/zsh
# Bake head and two PRLs cold --release with HEAD's prl-build, then derive
# nodepth (id 57 depth block emptied) and strip (id 57 removed) from head.
set -e
M=/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/991d9ce0-b6e7-4e9e-8a81-c2d9f7c07b7d/scratchpad/motion
R=/Users/dhiester/Projects/Personal/postretro
OUT=$M/ws/content/dev/maps
PY=$M/venv/bin/python
cd $R
for map in mover-setpiece enemy-movers; do
  for col in head two; do
    if [[ $col == head ]]; then src=content/dev/maps/$map.map; else src=$OUT/two-$map.map; fi
    rm -rf $M/cache/$col-$map; mkdir -p $M/cache/$col-$map
    /usr/bin/time -p nice $M/bin/prl-build $src -o $OUT/$col-$map.prl --release --no-tui -v \
      --cache-dir $M/cache/$col-$map > $M/logs/bake-$col-$map.log 2>&1
    echo "baked $col-$map"
  done
  $PY -I $M/tools/strip57.py depth $OUT/head-$map.prl $OUT/nodepth-$map.prl
  $PY -I $M/tools/strip57.py all $OUT/head-$map.prl $OUT/strip-$map.prl
done
ls -la $OUT
