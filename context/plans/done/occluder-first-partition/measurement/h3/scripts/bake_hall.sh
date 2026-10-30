#!/bin/bash
# H3: rebake stress-warren-hallway-inspection for cap26 then cap19 with 9g's frozen
# compilers and 9g's hallway flags (--sh-probe-spacing 4.0 --no-cache), into
# target/occluder-h3/<config>/ (9g's raw records stay untouched).
W=/Users/dhiester/Projects/Personal/postretro; BIN=$W/target/occluder-9g/bin; M=stress-warren-hallway-inspection
cd $W
for C in cap26 cap19; do
  O=$W/target/occluder-h3/$C/$M; mkdir -p $O
  { echo "binary: $BIN/prl-build-$C $(shasum $BIN/prl-build-$C | cut -c1-12)"; echo "start: $(date '+%F %T')"; df -h . | tail -1; } > $O/bake.time
  s=$(date +%s)
  RUST_LOG=info caffeinate -dimsu $BIN/prl-build-$C content/dev/maps/$M.map -o $O/$M.prl --baked-root $W/target/occluder-9g/baked --no-tui -v --sh-probe-spacing 4.0 --no-cache > $O/bake.log 2>&1
  echo "exit: $?" >> $O/bake.time; echo "wall_seconds: $(( $(date +%s) - s ))" >> $O/bake.time; df -h . | tail -1 >> $O/bake.time
  free=$(df -g . | tail -1 | awk '{print $4}'); [ "$free" -lt 15 ] && { echo "LOW DISK $free GB, stop" >> $O/bake.time; break; }
done
echo BAKES_DONE > $W/target/occluder-h3/bakes.status
