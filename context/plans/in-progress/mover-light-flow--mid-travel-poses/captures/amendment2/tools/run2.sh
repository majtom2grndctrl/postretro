#!/bin/bash
cd /c/Users/danhi/AppData/Local/Temp/claude/80d2a19114937a96/8904fe45-929f-4f76-b328-129186a2a8bd/scratchpad/amend2/tools
P=../venv/Scripts/python.exe
export A2_ATTRS=../attrs.json
set -e
$P acap.py batch 2 door_ceiling head cur 0.3,0.5 02 none
$P acap.py batch 2 door_ceiling head,shift cur,o40,r40 0.1:0.94:43 02
$P acap.py batch 2 door_ceiling head cur,o40,r40 0.1:0.94:43 02 wenc,wabove,xabove
$P acap.py batch 2 door_ceiling head,truth cur,r40 0:1:101
echo RUN2 DONE
