import json, os, sys
sys.path.insert(0, os.path.dirname(__file__))
import cap
S = cap.S
R = json.load(open(os.path.join(S, 'results.json')))
rows = R['interior']
ts = [r['t'] for r in rows]
series = [
    ('today (83dfb5317)', '#2a78d6', [r['frame_mean']['today'] for r in rows], '', 'circle'),
    ('through-base', '#eb6834', [r['frame_mean']['tb'] for r in rows], '', 'square'),
    ('lerp', '#1baf7a', [r['frame_mean']['lerp'] for r in rows], '', 'diamond'),
    ('id 57 stripped', '#eda100', [r['frame_mean']['strip-new'] for r in rows], '', 'triangle'),
]
nd = []
for t in ts:
    p = os.path.join(cap.CAP, f'interior-t{t:.2f}-nodepth.png')
    nd.append(cap.frame_mean(cap.read_luma(p)) if os.path.exists(p) else None)
series.append(('new, depth block stripped (diagnostic)', '#e87ba4', nd, '', 'circle'))
W, H = 1000, 600
L, Rm, T, B = 60, 30, 110, 60
pw, ph = W - L - Rm, H - T - B
ymax = 32
X = lambda t: L + t * pw
Y = lambda v: T + ph - v / ymax * ph
o = [f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{W}" viewBox="0 0 {W} {W}" font-family="Arial" font-size="13">',
     f'<rect width="{W}" height="{W}" fill="#fcfcfb"/>',
     f'<text x="{L}" y="26" font-size="16" fill="#1f1f1d">Rising room interior: frame mean luma against travel t′ (mover-setpiece, --release)</text>',
     f'<text x="{L}" y="45" fill="#5f5e58">Camera rides the room at (860, 320, floor + 72), yaw 0, pitch −8, fov 100; Rec. 709 luma 0–255. Knots at t′ 0, 0.2 … 1.</text>']
for v in range(0, 33, 4):
    o.append(f'<line x1="{L}" x2="{L+pw}" y1="{Y(v):.1f}" y2="{Y(v):.1f}" stroke="#e4e3de" stroke-width="1"/>')
    o.append(f'<text x="{L-8}" y="{Y(v)+4:.1f}" text-anchor="end" fill="#5f5e58">{v}</text>')
for i in range(0, 11):
    t = i / 10
    o.append(f'<text x="{X(t):.1f}" y="{T+ph+20}" text-anchor="middle" fill="#5f5e58">{t:.1f}</text>')
for k in range(1, 5):
    t = k / 5
    o.append(f'<line x1="{X(t):.1f}" x2="{X(t):.1f}" y1="{T}" y2="{T+ph}" stroke="#d6d5ce" stroke-width="1"/>')
o.append(f'<text x="{L+pw/2}" y="{H-14}" text-anchor="middle" fill="#3d3d3a">travel t′ (grey verticals: mid-pose knots; through-base mostly hidden under lerp)</text>')
o.append(f'<line x1="{L}" x2="{L+pw}" y1="{T+ph}" y2="{T+ph}" stroke="#8a8984" stroke-width="1"/>')
def marker(shape, x, y, c):
    if shape == 'circle':
        return f'<circle cx="{x:.1f}" cy="{y:.1f}" r="4" fill="{c}" stroke="#fcfcfb" stroke-width="2"/>'
    if shape == 'square':
        return f'<rect x="{x-4:.1f}" y="{y-4:.1f}" width="8" height="8" fill="{c}" stroke="#fcfcfb" stroke-width="2"/>'
    if shape == 'diamond':
        return f'<polygon points="{x:.1f},{y-5:.1f} {x+5:.1f},{y:.1f} {x:.1f},{y+5:.1f} {x-5:.1f},{y:.1f}" fill="{c}" stroke="#fcfcfb" stroke-width="2"/>'
    return f'<polygon points="{x:.1f},{y-5:.1f} {x+5:.1f},{y+4:.1f} {x-5:.1f},{y+4:.1f}" fill="{c}" stroke="#fcfcfb" stroke-width="2"/>'
ends = []
for name, c, vals, dash, shape in series:
    pts = [(X(t), Y(v)) for t, v in zip(ts, vals) if v is not None]
    path = 'M ' + ' L '.join(f'{x:.1f} {y:.1f}' for x, y in pts)
    o.append(f'<path d="{path}" fill="none" stroke="{c}" stroke-width="2"{(" stroke-dasharray=\"" + dash + "\"") if dash else ""}/>')
    o += [marker(shape, x, y, c) for x, y in pts]
    ends.append([pts[-1][1], name, c, shape])
# Legend: one row under the subtitle, in series order.
x = L
for name, c, vals, dash, shape in series:
    y = 72
    o.append(f'<line x1="{x}" x2="{x+24}" y1="{y}" y2="{y}" stroke="{c}" stroke-width="2"/>')
    o.append(marker(shape, x + 12, y, c))
    o.append(f'<text x="{x+30}" y="{y+4}" fill="#1f1f1d">{name}</text>')
    x += 40 + 7.2 * len(name)
o.append(f'<text x="{L}" y="96" fill="#5f5e58" font-size="12">At t′ 1 every channel build reads 9.08. Between knots through-base equals depth-stripped; at knots exact closed-pose depth darkens.</text>')
o.append('</svg>')
open(os.path.join(S, 'luma.svg'), 'w').write('\n'.join(o))
