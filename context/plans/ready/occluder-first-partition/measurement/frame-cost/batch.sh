#!/bin/sh
D=/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/8f487b87-e062-49e0-9ae8-817344ae0304/scratchpad/frame-cost
cd $D
for m in base wall nopillars; do
 while read tag pose; do
  $D/run1.sh ${tag}_$m $D/proj/content/dev/maps/occ-$m.prl "$pose" 26 x noshot >/dev/null 2>&1 || { echo "ABORT $tag $m" >> $D/batch.status; exit 1; }
  n=$(grep -c "\[CpuTiming\]" $D/logs/${tag}_$m.log)
  echo "$tag $m windows=$n" >> $D/batch.status
  [ "$n" -lt 4 ] && { echo "INVALID $tag $m" >> $D/batch.status; exit 1; }
  sleep 3
 done < $D/poses.txt
done
echo ALLDONE >> $D/batch.status
