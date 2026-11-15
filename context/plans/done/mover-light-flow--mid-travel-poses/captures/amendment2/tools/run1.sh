#!/bin/bash
cd /c/Users/danhi/AppData/Local/Temp/claude/80d2a19114937a96/8904fe45-929f-4f76-b328-129186a2a8bd/scratchpad/amend2/tools
P=../venv/Scripts/python.exe
set -e
$P acap.py batch 4 sweepfix head,truth cur,r10,r25,r40 0:0.5:129
$P acap.py batch 4 sweepfix head o40 0:0.5:129
$P acap.py batch 4 sweepfix head,truth cur,r10,r25,r40,o40 1.0
$P acap.py batch 4 walledge head,truth cur,r10,r25,r40 0.1:0.45:146
$P acap.py batch 4 walledge head o40 0.1:0.45:146
$P acap.py batch 4 walledge head,truth cur,r10,r25,r40,o40 0,1
$P acap.py batch 4 door_ceiling head,truth cur,r10,r25,r40 0.1:0.6:135
$P acap.py batch 4 door_ceiling head o40 0.1:0.6:135
$P acap.py batch 4 door_ceiling shift cur,r40 0.1:0.6:135
$P acap.py batch 4 door_ceiling head,truth cur,r10,r25,r40,o40 0,1
$P acap.py batch 4 floor head cur,r10,r25,r40,o40 0.5,0.8,0.86,0.9,0.95,1.0
echo RUN1 DONE
