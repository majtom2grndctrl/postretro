#!/bin/zsh
# Register screen for the glossy overlay pipelines: capture wgpu's MSL for the
# production overlay entries (or, with POSTRETRO_REGISTER_SCREEN_SOURCE set, a
# replacement WGSL source such as an older overlay), compile each named
# fragment entry for the discrete GPU, print its registers.
#
# usage: tools/metal_register_screen/screen_glossy.sh <out dir> [fragment entry...]
#        (default entries: fs_pixel fs_smooth, one per reflection style)
set -euo pipefail

if [[ $# -lt 1 ]]; then
    echo "usage: $0 <out dir> [fragment entry...]" >&2
    exit 2
fi
here=${0:A:h}
root=${here:h:h}
mkdir -p "$1"
out=${1:A}
shift
entries=("$@")
(( ${#entries} )) || entries=(fs_pixel fs_smooth)

(cd "$root" && POSTRETRO_REGISTER_SCREEN_OUT="$out" nice cargo test -j 6 -p postretro-renderer --lib -- --ignored --exact \
    render::glossy_overlay::register_screen_test::glossy_overlay_msl_capture_for_the_register_screen \
    --test-threads=1 >"$out/capture.log" 2>&1) || {
    echo "MSL capture failed; see $out/capture.log" >&2
    exit 1
}
grep -q "test result: ok. 1 passed" "$out/capture.log" || {
    echo "the capture test did not run; see $out/capture.log" >&2
    exit 1
}

xcrun swiftc -O "$here/screen.swift" -o "$out/screen"
for entry in $entries; do
    [[ -f "$out/$entry.metal" ]] || {
        echo "$entry: no captured MSL; the capture wrote $(cd "$out" && print -l *.metal(N) | tr '\n' ' ')" >&2
        exit 1
    }
    "$out/screen" "$out" "$entry" >/dev/null
    echo "$entry $(python3 -I "$here/regs.py" "$out/$entry.metallib" | grep '^fragment:')"
done
