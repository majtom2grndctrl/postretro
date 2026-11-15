#!/bin/bash
A=/c/Users/danhi/AppData/Local/Temp/claude/80d2a19114937a96/8904fe45-929f-4f76-b328-129186a2a8bd/scratchpad/amend2
B=$(dirname "$0")/dbake.sh
L=$A/darkb/logs/bakes.txt
: > $L
$B h2 head enemy-movers >> $L
$B dark head enemy-movers 1 >> $L
$B h2 head mover-setpiece >> $L
$B dark head mover-setpiece 1 >> $L
$B dtruth truth enemy-movers 1 >> $L
$B dtruth truth mover-setpiece 1 >> $L
echo BAKES DONE >> $L
