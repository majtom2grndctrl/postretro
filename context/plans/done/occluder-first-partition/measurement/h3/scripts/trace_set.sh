#!/bin/bash
S=/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/25609316-dd73-4754-993f-da7529caa8f7/scratchpad/h3
for p in occlusion-test.fc_P1 occlusion-test.fc_V1 stress-street.rd_promenade stress-street.rd_elevated stress-street.roof_B3.s1.e_180_-15 stress-street.st_g5.3.0_180 stress-warren-showcase.ar_g8.3.0_180 stress-warren-hallway-inspection.hw_g13.16.2_0 campaign-test.ct_g1.9.0_270; do
  for side in before cap19 cap26; do
    t=$p.$side.r1
    [ -f $S/trace/$t.frames.txt ] && grep -q completion $S/trace/$t.frames.txt && continue
    if [ ! -f $S/capruns/$t.json ]; then echo "no json $t"; continue; fi
    $S/trace_cap.sh $t > /dev/null 2>&1
    echo "$t: $(head -1 $S/trace/$t.frames.txt) | $(tail -1 $S/trace/$t.frames.txt)"
  done
done
echo TRACE_SET_DONE
