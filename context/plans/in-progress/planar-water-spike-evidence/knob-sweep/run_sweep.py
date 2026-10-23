"""Run from the workspace root. Python 3, no third-party dependencies; supports Windows."""
import argparse, hashlib, json, os, subprocess
from pathlib import Path


def configurations():
    base = dict(on=True, scale=0.5, d=None, r=None, band_d=4.0, band_r=4.0)
    rows = [("ssr-high", "high", dict(base, on=False)), ("ssr-medium", "medium", dict(base, on=False)), ("planar-half", "high", base)]
    rows += [("planar-quarter", "high", dict(base, scale=0.25)), ("planar-full", "high", dict(base, scale=1.0))]
    for knob in ("d", "r"):
        for value in (30, 15, 8):
            rows.append((f"{knob}-{value}", "high", dict(base, **{knob: value})))
    rows += [("quarter-d15-r15", "high", dict(base, scale=0.25, d=15, r=15)),
             ("half-d30-r15", "high", dict(base, d=30, r=15)),
             ("fade-narrow", "high", dict(base, d=30, r=15, band_d=0.5, band_r=0.5)),
             ("fade-wide", "high", dict(base, d=30, r=15, band_d=8.0, band_r=8.0))]
    return rows


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", default="target/debug/postretro.exe" if os.name == "nt" else "target/debug/postretro")
    parser.add_argument("--output", default="target/planar-water-sweep")
    parser.add_argument("--repeats", type=int, default=2)
    parser.add_argument("--warmup-frames", type=int, default=120)
    parser.add_argument("--sample-frames", type=int, default=240)
    parser.add_argument("--pose", choices=["both", "across", "down"], default="both")
    args = parser.parse_args()
    if args.repeats < 1 or not 1 <= args.warmup_frames <= 10000 or not 1 <= args.sample_frames <= 100000:
        parser.error("positive repeats and valid warmup/sample bounds required")
    root = Path(args.output).resolve()
    if root.exists() and any(root.iterdir()):
        parser.error("output directory must be empty; use a fresh --output path")
    root.mkdir(parents=True, exist_ok=True)
    records = []
    env = os.environ.copy()
    env.update(POSTRETRO_PLANAR_REFLECTION="0", POSTRETRO_CPU_TIMING="1", POSTRETRO_GPU_TIMING="1", POSTRETRO_SH_STREAMING="sync-proof", RUST_LOG="info", WGPU_VALIDATION_INDIRECT_CALL="1")
    manifest = {"binary_sha256": hashlib.sha256(Path(args.binary).read_bytes()).hexdigest(),
                "map_sha256": hashlib.sha256(Path("content/dev/maps/reflection-lake-test.prl").read_bytes()).hexdigest(),
                "git_head": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
                "warmup_frames":args.warmup_frames,"sample_frames":args.sample_frames,"repeats":args.repeats,
                "WGPU_VALIDATION_INDIRECT_CALL":"1",
                "note":"Dirty tree possible; settings explicit in each scene; binary supplied by caller, indirect validation forced on; synchronized captures."}
    (root/"manifest.json").write_text(json.dumps(manifest, indent=2)+"\n")
    for repeat in range(args.repeats):
        rows = configurations()
        if repeat % 2: rows = list(reversed(rows))
        poses = [("across",-10.0),("down",-55.0)]
        if args.pose != "both": poses = [p for p in poses if p[0] == args.pose]
        if repeat % 2: poses.reverse()
        for pose,pitch in poses:
            for name,quality,settings in rows:
                key=f"{pose}-{name}-run{repeat+1}"
                scene={"map":"content/dev/maps/reflection-lake-test.prl","camera":{"position":[-28.8656 if pose == "down" else -16.8656,1.7,-48.768],"yaw_deg":90.0,"pitch_deg":pitch},
                       "resolution":[960,540],"reflection_quality":quality,"planar_reflection":settings,
                       "output":str(root/(key+".png")),"measurement":{"report":str(root/(key+".json")),"warmup_frames":args.warmup_frames,"sample_frames":args.sample_frames}}
                scene_path=root/(key+"-scene.json"); scene_path.write_text(json.dumps(scene,indent=2)+"\n")
                with (root/(key+".log")).open("w") as log:
                    result=subprocess.run([str(Path(args.binary).resolve()),"--capture",str(scene_path)],env=env,stdout=log,stderr=subprocess.STDOUT,timeout=90)
                if result.returncode: raise RuntimeError(f"{key} failed {result.returncode}; inspect log")
                report=json.loads((root/(key+".json")).read_text())
                text=(root/(key+".log")).read_text()
                if "SolidCellFallback" in text or "Validation Error" in text:raise RuntimeError(f"{key}: invalid run; inspect log")
                if settings['on'] and '[Planar reflection] plane=' not in text:raise RuntimeError(f"{key}: no selected plane")
                record={"key":key,"pose":pose,"configuration":name,"repeat":repeat+1,"settings":settings,"quality":quality,
                        "cpu_completion":report["cpu_completion"],"gpu_timing":report["gpu_timing"],"adapter":report['adapter'],
                        "lightmap_counters":report['lightmap_streaming']['counters']}
                records.append(record)
                (root/"results.json").write_text(json.dumps(records,indent=2)+"\n")
                print(key,round(report['cpu_completion']['median_ms'],3),flush=True)

if __name__ == "__main__": main()
