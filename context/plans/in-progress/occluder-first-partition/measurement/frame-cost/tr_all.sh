#!/bin/sh
D=/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/8f487b87-e062-49e0-9ae8-817344ae0304/scratchpad/frame-cost
M=$D/proj/content/dev/maps
run() { # tag map pose
  $D/trace1.sh $1 $M/occ-$2.prl "$3" || return 1
  xcrun xctrace export --input $D/trace/$1.trace --xpath '/trace-toc/run[@number="1"]/data/table[@schema="metal-gpu-intervals"]' > $D/trace/$1.gpu.xml 2>/dev/null
  python3 $D/gpuparse2.py $D/trace/$1.gpu.xml 40 > $D/trace/$1.txt
  rm -rf $D/trace/$1.trace $D/trace/$1.gpu.xml $TMPDIR/instruments*.ktrace
  df -h / | tail -1 >> $D/trace/status
  echo "$1 done" >> $D/trace/status
}
run C1_base base "-3.2512,1.0,-3.2512,0,0"
run G1_base base "-14.224,1.0,-13.8176,98,15"
run P1_wall wall "-17.78,1.0,-13.8176,90,0"
run P1_nopillars nopillars "-17.78,1.0,-13.8176,90,0"
echo ALL >> $D/trace/status
