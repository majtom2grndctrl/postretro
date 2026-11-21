#!/bin/zsh
# usage: build_variant.sh <variant> <wgsl file> <commit message line>
# Installs the variant shader, commits it on the spike branch, screens the mover
# registers, builds the release capture binary and copies it out.
set -euo pipefail
v=$1; src=$2; msg=$3
WT=/Users/dhiester/Projects/Personal/postretro-spike-mover-cost
K=/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/01b2a632-f0d9-48e7-9edc-a890a7736e2f/scratchpad
cd $WT
[[ $(git branch --show-current) == spike/specular-mover-cost ]] || { echo "wrong branch"; exit 1; }
cp $src crates/renderer/src/shaders/kinematic_brush.wgsl
git add crates/renderer/src/shaders/kinematic_brush.wgsl
git commit -q -m "spike $v: $msg

Mover shader = this variant; everything else = f8cb485b7 + capture hook + kit.
Throwaway measurement build; never merge.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_018b4haiVH1jQJtU6qSY6Gy8"
sha=$(git rev-parse --short HEAD)
echo "$v commit $sha"
rm -rf $K/spike-regs/$v
CARGO_INCREMENTAL=0 tools/metal_register_screen/screen_mover.sh $K/spike-regs/$v | tee $K/spike-regs/$v.txt
CARGO_INCREMENTAL=0 cargo build --release -p postretro --features capture > $K/spike-logs/build-$v.log 2>&1
mkdir -p $K/spike-bin/$v
cp target/release/postretro $K/spike-bin/$v/postretro
echo $sha > $K/spike-bin/$v/COMMIT
echo "$v built $sha"; df -h / | tail -1
free_kb=$(df -k / | tail -1 | awk '{print $4}')
if (( free_kb < 15 * 1024 * 1024 )); then
    echo "under 15 GB free: clearing target/*/incremental"
    rm -rf target/*/incremental
    df -h / | tail -1
fi
