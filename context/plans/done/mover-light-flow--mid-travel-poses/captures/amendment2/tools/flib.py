# Shared loaders for task 25c analysis: columns across the final and prototype capture folders.
#   'head', 'truth', 'ohead', 'odark', 'odtruth'  -> final/cap/<view>-t<t>-<col>[-m<mask>].png
#   'P:<bake>-<form>'                             -> amend2/cap (prototype reference captures)
import os, sys
import numpy as np
from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
F = os.path.dirname(HERE)
A = os.path.dirname(F)
REPO_TOOLS = r'C:\Users\danhi\Projects\Personal\postretro\context\plans\in-progress\mover-light-flow--mid-travel-poses\captures\amendment2\tools'
os.environ['A2_ROOT'] = A
os.environ['A2_REGIONS'] = os.path.join(REPO_TOOLS, 'dregions.json')
sys.path.insert(0, REPO_TOOLS)
import acap, aseries  # noqa: E402

H, W, B = acap.H, acap.W, 8
REGIONS = aseries.REGIONS
FCAP = os.path.join(F, 'cap')
PCAP = os.path.join(A, 'cap')


def path(view, t, col, mask=None):
    m = f'-m{mask}' if mask else ''
    if col.startswith('P:'):
        return os.path.join(PCAP, f'{view}-t{t:.5f}-{col[2:]}{m}.png')
    return os.path.join(FCAP, f'{view}-t{t:.5f}-{col}{m}.png')


_cache = {}


def luma(view, t, col, mask=None):
    p = path(view, t, col, mask)
    if p in _cache:
        return _cache[p]
    if not os.path.exists(p):
        return None
    a = np.asarray(Image.open(p).convert('RGB'), dtype=np.float64)
    L = (0.2126 * a[..., 0] + 0.7152 * a[..., 1] + 0.0722 * a[..., 2]).astype(np.float32)
    if len(_cache) > 600:
        _cache.clear()
    _cache[p] = L
    return L


def rgb(view, t, col, mask=None):
    p = path(view, t, col, mask)
    return None if not os.path.exists(p) else np.asarray(Image.open(p).convert('RGB'))


def blocks(L, m):
    return aseries.blocks(L, m)


def series(view, region, col, ts, mask=None):
    m = REGIONS[(view, region)]
    means, bl = [], []
    for t in ts:
        L = luma(view, t, col, mask)
        if L is None:
            means.append(None); bl.append(None); continue
        means.append(float(L[m].mean())); bl.append(blocks(L, m))
    return means, bl


def steps(bl, lag):
    """Per sample i >= lag: largest |block delta| from sample i-lag (None where missing)."""
    out = [None] * len(bl)
    for i in range(lag, len(bl)):
        if bl[i] is not None and bl[i - lag] is not None:
            out[i] = float(np.abs(bl[i] - bl[i - lag]).max())
    return out


def vs(a, b):
    """Per sample: (mean |a-b|, worst |a-b|, most negative a-b, blocks a-b < -3)."""
    out = []
    for x, y in zip(a, b):
        if x is None or y is None:
            out.append(None); continue
        d = x - y
        out.append((float(np.abs(d).mean()), float(np.abs(d).max()), float(d.min()), int((d < -3).sum())))
    return out


def summarize_vs(c, ts):
    v = [(x, t) for x, t in zip(c, ts) if x is not None]
    if not v:
        return None
    lo = min(v, key=lambda p: p[0][2])
    wo = max(v, key=lambda p: p[0][1])
    return dict(mean=float(np.mean([x[0] for x, _ in v])), worst=wo[0][1], worst_t=wo[1],
                darkest=lo[0][2], darkest_t=lo[1], frames_blotch=int(sum(1 for x, _ in v if x[3] > 0)), n=len(v))


def summarize_steps(st, ts, bar=3.0):
    v = [(s, t) for s, t in zip(st, ts) if s is not None]
    if not v:
        return None
    mx = max(v)
    return dict(max=mx[0], at=mx[1], over=int(sum(s > bar for s, _ in v)), n=len(v))
