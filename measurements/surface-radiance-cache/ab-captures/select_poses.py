"""Pick A/B camera poses from the id-34 data: where does `_bounce_fill` change the SH base most?

usage: <venv>/bin/python select_poses.py   (from anywhere; writes poses.json beside this file)

Campaign-test: decode id 34 from ab-campaign-fill-0.prl and -1.prl (sh34.py), reconstruct each
valid probe's mean irradiance (L0 tile / scale-aware L1 trilinear / L2 node mean), and take
luminance (Rec. 709). Candidates are probes 1-2 cells above solid (eye height over a floor), so
the camera looks at surfaces a player sees. Two rankings, spots kept >= 10 m apart:
  * "relative": change / (fill-0 + 0.005) among probes whose change is above the near-floor
    75th percentile -- big lift where fill 0 is dim (indirect-dominated).
  * "absolute": largest change.
Four spots come from the relative ranking, then two from the absolute one.
Each spot gets a camera 2-4 m away, placed so that the eight probes around the camera and every
sample on the camera->spot segment are valid (open space), and preferring a direction with a
surface 1-5 m beyond the spot; it looks at the spot, pitched down 12 degrees.
White room: two hand-placed poses (pillar face edge-on to both lights; far wall corner); the
script reports their local probe stats.
"""
import json, math, sys
from pathlib import Path
import numpy as np

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
sys.path.insert(0, str(HERE))
import sh34  # noqa: E402

LUMA = np.array([0.2126, 0.7152, 0.0722])
MAPS = ROOT / "content/dev/maps"
SEP_M = 10.0
PITCH = -12.0


def luminance(path):
    v, st, means, probe = sh34.load(path)
    return v, probe @ LUMA


def grid_pos(v, idx):
    x, y, z = idx
    return v["origin"] + v["cell"] * np.array([x, y, z], dtype=float)


def open_at(valid, v, p):
    """True when all eight probes around world point p are valid (p sits in an open cube)."""
    g = (np.asarray(p) - v["origin"]) / v["cell"]
    lo = np.floor(g).astype(int)
    dz, dy, dx = valid.shape
    for c in range(8):
        x, y, z = lo[0] + (c & 1), lo[1] + ((c >> 1) & 1), lo[2] + ((c >> 2) & 1)
        if not (0 <= x < dx and 0 <= y < dy and 0 <= z < dz) or not valid[z, y, x]:
            return False
    return True


def nearest_valid(valid, v, p):
    g = np.rint((np.asarray(p) - v["origin"]) / v["cell"]).astype(int)
    dz, dy, dx = valid.shape
    x, y, z = g
    return 0 <= x < dx and 0 <= y < dy and 0 <= z < dz and bool(valid[z, y, x])


def yaw_to(src, dst):
    d = np.asarray(dst) - np.asarray(src)
    return math.degrees(math.atan2(-d[0], -d[2])) % 360.0


def solve_camera(valid, v, spot):
    best = None
    for dist in (3.0, 2.5, 3.5, 2.0, 4.0):
        for k in range(16):
            th = 2 * math.pi * k / 16
            u = np.array([math.cos(th), 0.0, math.sin(th)])
            cam = spot + dist * u
            if not open_at(valid, v, cam):
                continue
            if not all(open_at(valid, v, cam - u * s) or nearest_valid(valid, v, cam - u * s)
                       for s in np.arange(0.25, dist, 0.25)):
                continue
            behind = None  # surface distance beyond the spot, away from the camera
            for s in np.arange(0.25, 8.0, 0.25):
                if not nearest_valid(valid, v, spot - u * s):
                    behind = s
                    break
            if behind is None:
                continue
            cost = abs(behind - 2.5) + 0.5 * abs(dist - 3.0)
            if best is None or cost < best[0]:
                best = (cost, cam, dist, behind)
    if best is None:
        return None
    _, cam, dist, behind = best
    return {"position": [round(float(c), 3) for c in cam], "yaw_deg": round(yaw_to(cam, spot), 1),
            "pitch_deg": PITCH, "camera_to_spot_m": dist, "surface_beyond_spot_m": float(behind)}


def probe_fields(mapkey):
    va, la = luminance(MAPS / f"ab-{mapkey}-fill-0.prl")
    vb, lb = luminance(MAPS / f"ab-{mapkey}-fill-1.prl")
    assert va["dims"] == vb["dims"]
    valid = va["validity"].reshape(la.shape) != 0
    ok = np.isfinite(la) & np.isfinite(lb)
    d = np.where(ok, lb - la, np.nan)
    # height above solid, in cells
    h = np.full(la.shape, -1)
    for y in range(la.shape[1]):
        prev = h[:, y - 1, :] if y else np.full(h[:, 0, :].shape, -1)
        h[:, y, :] = np.where(ok[:, y, :], np.where(prev >= 0, prev + 1, 0), -1)
    near = ok & (h >= 1) & (h <= 2)
    stats = {"valid_probes": int(ok.sum()), "near_floor_probes": int(near.sum()),
             "fill0_lum_pct_50_90_99": np.percentile(la[ok], [50, 90, 99]).round(5).tolist(),
             "fill1_lum_pct_50_90_99": np.percentile(lb[ok], [50, 90, 99]).round(5).tolist(),
             "delta_lum_pct_50_90_99_max": [*np.percentile(d[ok], [50, 90, 99]).round(5).tolist(),
                                            round(float(np.nanmax(d)), 5)],
             "relative_delta_pct_50_90_99": np.percentile((d / np.maximum(la, 1e-4))[ok], [50, 90, 99]).round(3).tolist()}
    return dict(v=va, la=la, lb=lb, d=d, ok=ok, valid=valid, h=h, near=near, stats=stats)


def spot_record(f, rule, idx, cam):
    x, y, z = idx
    la, lb, d = f["la"], f["lb"], f["d"]
    sl = (slice(max(z - 2, 0), z + 3), slice(max(y - 2, 0), y + 3), slice(max(x - 2, 0), x + 3))
    return {"rule": rule, "probe": [int(x), int(y), int(z)], "spot": grid_pos(f["v"], idx).round(3).tolist(),
            "fill0_lum": round(float(la[z, y, x]), 5), "fill1_lum": round(float(lb[z, y, x]), 5),
            "delta_lum": round(float(d[z, y, x]), 5),
            "nbhd5_mean_fill0": round(float(np.nanmean(la[sl])), 5),
            "nbhd5_mean_delta": round(float(np.nanmean(d[sl])), 5), "camera": cam}


def ranked(mapkey, plan, prefix):
    """plan: [(rule, count)]; rules 'relative', 'absolute' (see module doc) and 'corridor'."""
    f = probe_fields(mapkey)
    va, la, d, near, valid = f["v"], f["la"], f["d"], f["near"], f["valid"]
    p75 = np.percentile(d[near], 75)
    rankings = {"relative": np.where(near & (d > p75), d / (la + 0.005), -np.inf),
                "absolute": np.where(near, d, -np.inf)}
    spots = []
    for rule, want in plan:
        if rule == "corridor":
            spots.append(corridor(f))
            continue
        score = rankings[rule]
        got = 0
        for flat in np.argsort(score, axis=None)[::-1]:
            z, y, x = np.unravel_index(flat, score.shape)
            if not np.isfinite(score[z, y, x]):
                break
            pos = grid_pos(va, (x, y, z))
            if any(np.linalg.norm(pos - np.array(s["spot"])) < SEP_M for s in spots):
                continue
            cam = solve_camera(valid, va, pos)
            if cam is None:
                continue
            spots.append(spot_record(f, rule, (x, y, z), cam))
            got += 1
            if got == want:
                break
    for i, s in enumerate(spots):
        s["name"] = f"{prefix}{i + 1}-{s['rule'][:3]}"
    return f["stats"], spots


def corridor(f):
    """Player-height view down the longest open axis run: the camera sits at a probe one cell
    above the lowest valid probe over a floor (about 1.5 m), with two open probe cells on both
    sides and above it, and looks along the axis with the most such open cells ahead, pitch -3."""
    v, ok, h, valid = f["v"], f["ok"], f["h"], f["valid"]
    dz, dy, dx = ok.shape

    def clear(x, y, z, ddx, ddz):
        # open probes 1-2 cells to each side (perpendicular to the run) and one above
        px, pz = ddz, ddx
        for k in (1, 2):
            for sgn in (1, -1):
                cx, cz = x + sgn * k * px, z + sgn * k * pz
                if not (0 <= cx < dx and 0 <= cz < dz and ok[cz, y, cx]):
                    return False
        return y + 1 < dy and ok[z, y + 1, x]

    best = None
    zz, yy, xx = np.nonzero(ok & (h == 1))
    for z, y, x in zip(zz, yy, xx):
        for ddx, ddz in ((1, 0), (-1, 0), (0, 1), (0, -1)):
            if not clear(x, y, z, ddx, ddz) or not open_at(valid, v, grid_pos(v, (x, y, z))):
                continue
            n = 0
            cx, cz = x + ddx, z + ddz
            while 0 <= cx < dx and 0 <= cz < dz and ok[cz, y, cx] and h[cz, y, cx] >= 1:
                n += 1; cx += ddx; cz += ddz
            if best is None or n > best[0]:
                best = (n, (x, y, z), (ddx, ddz))
    n, idx, (ddx, ddz) = best
    pos = grid_pos(v, idx)
    target = pos + np.array([ddx, 0.0, ddz]) * v["cell"]
    cam = {"position": [round(float(c), 3) for c in pos], "yaw_deg": round(yaw_to(pos, target), 1),
           "pitch_deg": -3.0, "open_run_m": float(n * v["cell"][0])}
    return spot_record(f, "corridor", idx, cam)


def campaign():
    return ranked("campaign", [("relative", 4), ("absolute", 2)], "c")


def movement_feel():
    return ranked("movement-feel", [("relative", 2), ("absolute", 2), ("corridor", 1)], "m")


INCH = 0.0254


def engine(x, y, z):
    """TrenchBroom map units (z up) -> engine metres (y up)."""
    return np.array([-y * INCH, z * INCH, -x * INCH])


def movers():
    """mover-setpiece: frame the sliding wall (authored closed in the dividing-wall opening, map
    x 96-640, y 1040-1072, z 0-224). One pose is the README's station 1 (8.1 m south, the typical
    read). The other two take, on each side of the wall, the near-floor probe within 1.5 m of the
    wall face with the largest relative change, and look at it square-on from 4.5 m away."""
    f = probe_fields("mover-setpiece")
    v, la, d, near = f["v"], f["la"], f["d"], f["near"]
    poses = []
    s1 = engine(368, 720, 64)
    poses.append({"name": "v1-station1-wall", "rule": "README station 1",
                  "camera": {"position": s1.round(3).tolist(), "yaw_deg": 90.0, "pitch_deg": 0.0}})
    dz, dy, dx = la.shape
    zz, yy, xx = np.nonzero(near)
    for side, face_y, sign in (("south", 1040, -1), ("north", 1072, +1)):
        best = None
        for z, y, x in zip(zz, yy, xx):
            p = grid_pos(v, (x, y, z))
            my, mx = -p[0] / INCH, -p[2] / INCH  # back to map units
            off = (my - face_y) * sign  # metres-ish in front of the face
            if not (0 < off * INCH <= 1.5 and 120 <= mx <= 616):
                continue
            score = d[z, y, x] / (la[z, y, x] + 0.005)
            if best is None or score > best[0]:
                best = (score, (x, y, z))
        _, idx = best
        p = grid_pos(v, idx)
        cam = p.copy()
        cam[0] = p[0] - sign * 4.5  # engine x = -map y; step away from the face
        cam[1] = 1.6
        poses.append(spot_record(f, f"{side} face, max relative change", idx,
                                 {"position": cam.round(3).tolist(), "yaw_deg": round(yaw_to(cam, [p[0], 1.6, p[2]]), 1),
                                  "pitch_deg": -8.0}))
        poses[-1]["name"] = f"v{len(poses)}-{side}-face"
    return f["stats"], poses


def whiteroom():
    va, la = luminance(MAPS / "ab-whiteroom-fill-0.prl")
    vb, lb = luminance(MAPS / "ab-whiteroom-fill-1.prl")
    ok = np.isfinite(la) & np.isfinite(lb)
    d = lb - la
    stats = {"valid_probes": int(ok.sum()),
             "fill0_lum_pct_50_90_99": np.percentile(la[ok], [50, 90, 99]).round(5).tolist(),
             "fill1_lum_pct_50_90_99": np.percentile(lb[ok], [50, 90, 99]).round(5).tolist(),
             "delta_lum_pct_50_90_99_max": [*np.percentile(d[ok], [50, 90, 99]).round(5).tolist(),
                                            round(float(np.nanmax(d[ok])), 5)]}
    # Map (x, y, z) inches -> engine (-y, z, -x) metres. Pillar centre map (512, 512): engine
    # (-13.0, -13.0). Lights at map (263|775, 523, 280): both on the pillar's x axis, so the
    # pillar's map +-y faces are edge-on to both lights and see almost no direct light.
    poses = [
        {"name": "w1-pillar-shadow-face", "spot": [-12.2, 1.0, -13.0],
         "camera": {"position": [-6.0, 1.6, -13.0], "yaw_deg": 90.0, "pitch_deg": -10.0}},
        {"name": "w2-far-corner", "spot": [-0.5, 1.5, -0.5],
         "camera": {"position": [-4.5, 1.6, -4.5], "yaw_deg": 225.0, "pitch_deg": -5.0}},
    ]
    for p in poses:
        g = np.rint((np.array(p["spot"]) - va["origin"]) / va["cell"]).astype(int)
        sl = (slice(max(g[2] - 2, 0), g[2] + 3), slice(max(g[1] - 2, 0), g[1] + 3), slice(max(g[0] - 2, 0), g[0] + 3))
        p["nbhd5_mean_fill0"] = round(float(np.nanmean(la[sl])), 5)
        p["nbhd5_mean_delta"] = round(float(np.nanmean(d[sl])), 5)
    return stats, poses


SECTIONS = {"campaign": campaign, "whiteroom": whiteroom, "movement-feel": movement_feel,
            "mover-setpiece": movers}

if __name__ == "__main__":
    want = sys.argv[1:] or list(SECTIONS)
    path = HERE / "poses.json"
    out = json.loads(path.read_text()) if path.exists() else {}
    for key in want:
        st, poses = SECTIONS[key]()
        out[key] = {"stats": st, "poses": poses}
        print(json.dumps(out[key], indent=1))
    path.write_text(json.dumps(out, indent=1) + "\n")
