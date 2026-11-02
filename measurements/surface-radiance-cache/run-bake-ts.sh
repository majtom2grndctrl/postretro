#!/bin/bash
# One cold, uncached --release bake under pinned conditions.
# usage: run-bake.sh <before|after> <map.map> <label> [outdir]
# Writes <outdir>/<label>.{prl,out.log,err.log,time.txt}; prints wall seconds.
set -u
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SCRATCH="${SCRATCH:-/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/bff9e759-cf43-4be0-9557-5b1392586530/scratchpad}"
which_bin="$1"; map="$2"; label="$3"; out="${4:-$SCRATCH/runs}"
case "$which_bin" in
  before) BIN="$SCRATCH/bin/prl-build-base";;
  after)  BIN="$ROOT/target/release/prl-build";;
esac
mkdir -p "$out"
if pgrep -fl 'cargo|prl-build|rustc' | grep -v "run-bake" >/dev/null; then echo "BUSY: other build running" >&2; pgrep -fl 'cargo|prl-build|rustc' >&2; exit 2; fi
echo "pgrep-check: clean" > "$out/$label.time.txt"
cd "$ROOT"
start=$(date +%s)
RUST_LOG="${RUST_LOG:-info}" /usr/bin/time -l caffeinate -dims "$BIN" "$map" -o "$out/$label.prl" --release --no-tui \
  --baked-root "$SCRATCH/baked" >"$out/$label.out.log" 2> >(python3 -u "$ROOT/measurements/surface-radiance-cache/ts.py" >"$out/$label.err.log")
rc=$?
end=$(date +%s)
echo "bin=$which_bin map=$map rc=$rc wall_s=$((end-start))" >> "$out/$label.time.txt"
sed -n '/^Build Summary:/,/^Warnings/p' "$out/$label.out.log" | grep -v '^Warnings' > "$out/$label.summary.txt"
[ "${KEEP:-0}" = 1 ] || rm -f "$out/$label.prl"
echo "$label rc=$rc wall=$((end-start))s"
exit $rc
