#!/bin/sh
# Rerun everything. Run from this directory.
D=$(cd "$(dirname "$0")/../../../../../../content/dev/maps" && pwd)
(cd gran && cargo build --release) || exit 1
./gran/target/release/gran stats 15 $D/occlusion-test.prl > stats_occlusion.txt
./gran/target/release/gran stats 0 $D/campaign-test.prl > stats_campaign-test.txt
./gran/target/release/gran stats 0 $D/stress-warren-hallway-inspection.prl > stats_stress-warren-hallway-inspection.txt
python3 match_slivers.py stats_occlusion.txt > slivers_matched.txt
./gran/target/release/gran decomp $D/occlusion-test.prl decomp.csv > decomp_out.txt
python3 summarize.py > summary.txt
