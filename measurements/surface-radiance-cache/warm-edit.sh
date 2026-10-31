#!/bin/bash
# AC 34 / 37 procedure: cold-fill a private cache, edit ONE static light, rebuild warm.
# usage: warm-edit.sh <prl-build binary> <maps/NAME.map in the repo> <workdir>
# Works on a scratch copy of the map, so the working tree stays clean. Run it once
# per binary (today's warm build = the base-commit prl-build; new = target/release).
# Output in <workdir>: A.* (first warm build, empty cache), B.* (after the edit),
# each with .err.log (timestamped stderr), .out.log (Build Summary), .phases.json, .cache.json.
set -eu
BIN="$1"; SRC="$2"; WORK="$3"
HERE="$(cd "$(dirname "$0")" && pwd)"; ROOT="$(cd "$HERE/../.." && pwd)"
BAKED="${BAKED:-$WORK/baked}"
name="$(basename "$SRC" .map)"
mkdir -p "$WORK/m/maps" "$WORK/cache"
[ -z "$(ls -A "$WORK/cache")" ] || { echo "cache dir not empty: $WORK/cache" >&2; exit 2; }
cp "$SRC" "$WORK/m/maps/$name.map"
for x in "${SRC%.map}".generated.ts; do [ -f "$x" ] && cp "$x" "$WORK/m/maps/"; done
for d in textures scripts models sounds; do ln -sfn "$ROOT/content/dev/$d" "$WORK/m/$d"; done
# scripts-build must sit beside the binary or on PATH
export PATH="$ROOT/target/release:$PATH"
build() {  # label
  if pgrep -fl 'cargo|prl-build|rustc' | grep -v zsh >/dev/null; then echo "BUSY: other build running" >&2; exit 2; fi
  local e0; e0=$(date +%s)
  ( cd "$ROOT" && RUST_LOG=info /usr/bin/time -l caffeinate -dims "$BIN" "$WORK/m/maps/$name.map" \
      -o "$WORK/$1.prl" --no-tui --cache-dir "$WORK/cache" --baked-root "$BAKED" \
      >"$WORK/$1.out.log" 2> >(python3 -u "$HERE/ts.py" >"$WORK/$1.err.log") )
  sleep 1
  echo "$e0" >"$WORK/$1.epoch0"
  python3 "$HERE/phases.py" "$WORK" "$1" >"$WORK/$1.phases.json" 2>/dev/null || true
  python3 - "$WORK" "$1" "$HERE" "$e0" <<'PY'
import json, subprocess, sys
w, l, h, e0 = sys.argv[1:5]
p = json.load(open(f"{w}/{l}.phases.json"))[l]
if "t_sh_start" in p:
    out = subprocess.run(["python3", f"{h}/cache-window.py", f"{w}/cache", e0, str(p["t_sh_start"]), str(p["t_sh_end"])], capture_output=True, text=True).stdout
    open(f"{w}/{l}.cache.json", "w").write(out)
PY
  rm -f "$WORK/$1.prl"
  grep -E '^  (SH Bake|Delta SH Bake|Lightmap Bake|Total) ' "$WORK/$1.out.log" | tr -s ' ' | sed "s/^/$1 /"
  grep -a -o '\[cache\] sh_[a-z_]* \(hit\|miss\)' "$WORK/$1.err.log" | sort | uniq -c | sed "s/^/$1 /"
}
build A
# edit the first plain static point light (classname "light"): intensity x1.1
python3 - "$WORK/m/maps/$name.map" <<'PY'
import re, sys
p = sys.argv[1]; s = open(p).read()
for m in re.finditer(r'\{[^{}]*"classname" "light"[^{}]*\}', s):
    blk = m.group(0)
    if '"style"' in blk or '"_animated"' in blk or '"_bake_only" "1"' in blk: continue
    k = re.search(r'"light" "(\d+(?:\.\d+)?)"', blk)
    if not k: continue
    new = blk.replace(k.group(0), '"light" "%g"' % round(float(k.group(1)) * 1.1, 2))
    open(p, "w").write(s[:m.start()] + new + s[m.end():]); print("edited light:", k.group(1), "->", round(float(k.group(1)) * 1.1, 2)); break
else: sys.exit("no editable static light found")
PY
build B
echo "results in $WORK"
