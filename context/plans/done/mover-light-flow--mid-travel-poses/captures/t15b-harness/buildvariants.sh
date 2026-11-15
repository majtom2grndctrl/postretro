#!/bin/zsh
# Build prl-build (release) and the compiler test binary per outside-distance variant.
set -u
R=/Users/dhiester/Projects/Personal/postretro
T=/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/991d9ce0-b6e7-4e9e-8a81-c2d9f7c07b7d/scratchpad/t15b
F=$R/crates/level-compiler/src/mover_light_gate/geometry_knots.rs
cd $R
for spec in "o44:4.4 / 39.37" "o20:2.0 / 39.37" "o10:1.0 / 39.37" "o005:0.005"; do
  name=${spec%%:*}; value=${spec#*:}
  sed -i '' "s|^pub(crate) const OUTSIDE_KNOT_DISTANCE_M: f64 = .*;|pub(crate) const OUTSIDE_KNOT_DISTANCE_M: f64 = $value;|" $F
  grep "^pub(crate) const OUTSIDE_KNOT_DISTANCE_M" $F
  mkdir -p $T/bin/$name
  nice cargo build --release -p postretro-level-compiler --bin prl-build > $T/logs/build-$name.log 2>&1 || { echo "build $name failed"; exit 1; }
  cp target/release/prl-build $T/bin/$name/
  nice cargo test -p postretro-level-compiler --bin prl-build --no-run > $T/logs/testbuild-$name.log 2>&1 || { echo "test build $name failed"; exit 1; }
  exe=$(grep -o 'target/debug/deps/prl_build-[0-9a-f]*' $T/logs/testbuild-$name.log | tail -1)
  cp $exe $T/bin/$name/prl_build_tests
  echo "built $name"
done
sed -i '' "s|^pub(crate) const OUTSIDE_KNOT_DISTANCE_M: f64 = .*;|pub(crate) const OUTSIDE_KNOT_DISTANCE_M: f64 = 4.4 / 39.37;|" $F
echo restored
