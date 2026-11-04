#!/usr/bin/env python3
"""Windows driver for the closet captures. Run from the worktree root.

  closet.py gen                         write the static-door and no-door maps
  closet.py bake <key> [--bin DIR]      compile a map (key: mover, static, nodoor)
  closet.py cap <label> <key> <outdir> [--bin DIR] [--env K=V ...]
"""
import json, os, subprocess, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import pngstat

S = 0.0254
ROOT = os.getcwd()
MAPS = os.path.join(ROOT, "content", "dev", "maps")
KEYS = {"mover": "mover-closet", "static": ".mover-closet-static-ref", "nodoor": ".mover-closet-nodoor-ref"}
# Views are final.py's, which produced the Mac table. The README's station 2 gives no pitch.
VIEWS = {"door": ((320, 720, 64), 270, -6), "threshold": ((320, 610, 96), 270, -38), "bright": ((320, 200, 48), 90, 0)}
eng = lambda x, y, z: [-y * S, z * S, -x * S]

def opt(args, name, default=None):
    return args[args.index(name) + 1] if name in args else default

def main():
    a = sys.argv[1:]
    bindir = opt(a, "--bin", os.path.join(ROOT, "target", "debug"))
    if a[0] == "gen":
        gen = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "gen_closet.py")
        for flag, key in (("--static-door", "static"), ("--no-door", "nodoor")):
            out = subprocess.run([sys.executable, gen, flag], capture_output=True, text=True, check=True).stdout
            open(os.path.join(MAPS, KEYS[key] + ".map"), "w", encoding="utf-8", newline="\n").write(out.replace("\r\n", "\n"))
    elif a[0] == "bake":
        m = os.path.join(MAPS, KEYS[a[1]])
        subprocess.run([os.path.join(bindir, "prl-build.exe"), m + ".map", "-o", m + ".prl"], check=True)
    elif a[0] == "cap":
        label, key, out = a[1], a[2], a[3]
        os.makedirs(out, exist_ok=True)
        env = dict(os.environ, POSTRETRO_SH_STREAMING="sync-proof")
        for i, x in enumerate(a):
            if x == "--env":
                k, v = a[i + 1].split("=", 1)
                env[k] = v
        for v, (p, yaw, pitch) in VIEWS.items():
            png = os.path.join(out, f"{label}_{v}.png")
            sc = {"map": os.path.join(MAPS, KEYS[key] + ".prl"),
                  "camera": {"position": eng(*p), "yaw_deg": yaw, "pitch_deg": pitch, "fov_deg": 100.0},
                  "resolution": [960, 540], "output": png, "force_full_resident_sh_compose": True}
            f = os.path.join(out, f"{label}_{v}.json")
            json.dump(sc, open(f, "w", encoding="utf-8"))
            r = subprocess.run([os.path.join(bindir, "postretro.exe"), "--capture", f], cwd=ROOT, env=env,
                               capture_output=True, text=True, stdin=subprocess.DEVNULL)
            open(os.path.join(out, f"{label}_{v}.log"), "w", encoding="utf-8").write(r.stdout + r.stderr)
            if r.returncode:
                print("FAIL", label, v, r.returncode, r.stderr[-400:])
                sys.exit(1)
            im = pngstat.read_png(png)
            fl = " ".join(f"{m}={pngstat.floor_row(im, m):.1f}" for m in (0.04, 0.26, 0.48)) if v == "threshold" else ""
            print(f"{label} {v}: mean {pngstat.frame_mean(im):.2f} {fl}", flush=True)

main()
