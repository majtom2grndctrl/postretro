#!/bin/zsh
# usage: screen_only.sh <name> <wgsl>... pairs: screens each candidate mover shader
# without committing, then restores the committed shader.
set -euo pipefail
WT=/Users/dhiester/Projects/Personal/postretro-spike-mover-cost
K=/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/01b2a632-f0d9-48e7-9edc-a890a7736e2f/scratchpad
S=crates/renderer/src/shaders/kinematic_brush.wgsl
cd $WT
while (( $# >= 2 )); do
    name=$1; src=$2; shift 2
    cp $src $S
    rm -rf $K/spike-regs/$name
    if CARGO_INCREMENTAL=0 tools/metal_register_screen/screen_mover.sh $K/spike-regs/$name > $K/spike-regs/$name.txt 2>&1; then
        echo "$name $(grep fragment: $K/spike-regs/$name.txt)"
    else
        echo "$name FAILED (see $K/spike-regs/$name/capture.log)"
    fi
done
git show HEAD:$S > $S
git status --short $S
