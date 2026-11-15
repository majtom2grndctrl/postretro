#!/bin/bash
cd /c/Users/danhi/AppData/Local/Temp/claude/80d2a19114937a96/8904fe45-929f-4f76-b328-129186a2a8bd/scratchpad/amend2/tools
P=../venv/Scripts/python.exe
set -e
until grep -q "RUN1 DONE\|exit" ../logs/run1.txt; do sleep 10; done
$P acap.py batch 4 wall_owner head cur,r10,r25,r40,o40 0,0.26,1
$P acap.py batch 4 sweeper_east head cur,r10,r25,r40,o40 0,0.1,1
$P acap.py batch 4 floor strip cur 0.5,0.8,0.86,0.9,0.95,1.0
$P acap.py batch 4 floor head cur,r10,r25,r40,o40 0,0.2,0.4,0.6,0.7,0.75,0.82,0.84,0.88,0.92,0.97
echo RUN3 DONE
