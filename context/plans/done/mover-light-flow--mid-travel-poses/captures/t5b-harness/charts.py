# Task 5b charts: the rising room interior's mean luma against t' for task 5's
# builds and the experiment PRLs (GPU captures), with the static-brush
# reference's CPU model; and the CPU model's validation against the GPU.
#   python3 -I charts.py <room-cpu.json (base dump)> <out dir>
import json, os, subprocess, sys
HERE = os.path.dirname(os.path.abspath(__file__))
R = json.load(open(os.path.join(HERE, 't5b-results.json')))
cpu = json.load(open(sys.argv[1]))
out = sys.argv[2]


def gpu(col):
    return [(r['t'], r['frame_mean']) for r in R[col]['interior']]


def chart(name, title, subtitle, series, ymax, note):
    W, H = 1000, 640
    L, Rm, T, B = 60, 30, 120, 60
    pw, ph = W - L - Rm, H - T - B
    X = lambda t: L + t * pw
    Y = lambda v: T + ph - min(v, ymax) / ymax * ph
    o = [f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{W}" viewBox="0 0 {W} {W}" font-family="Arial" font-size="13">',
         f'<rect width="{W}" height="{W}" fill="#fcfcfb"/>',
         f'<text x="{L}" y="26" font-size="16" fill="#1f1f1d">{title}</text>',
         f'<text x="{L}" y="45" fill="#5f5e58">{subtitle}</text>']
    for v in range(0, ymax + 1, 2):
        o.append(f'<line x1="{L}" x2="{L+pw}" y1="{Y(v):.1f}" y2="{Y(v):.1f}" stroke="#e4e3de"/>')
        o.append(f'<text x="{L-8}" y="{Y(v)+4:.1f}" text-anchor="end" fill="#5f5e58">{v}</text>')
    for i in range(11):
        o.append(f'<text x="{X(i/10):.1f}" y="{T+ph+20}" text-anchor="middle" fill="#5f5e58">{i/10:.1f}</text>')
    for k in range(1, 5):
        o.append(f'<line x1="{X(k/5):.1f}" x2="{X(k/5):.1f}" y1="{T}" y2="{T+ph}" stroke="#d6d5ce"/>')
    o.append(f'<line x1="{L}" x2="{L+pw}" y1="{T+ph}" y2="{T+ph}" stroke="#8a8984"/>')
    o.append(f'<text x="{L+pw/2}" y="{H-14}" text-anchor="middle" fill="#3d3d3a">travel t′ (grey verticals: mid-pose knots)</text>')
    x = L
    for label, c, pts, dash, r in series:
        path = 'M ' + ' L '.join(f'{X(t):.1f} {Y(v):.1f}' for t, v in pts)
        d = f' stroke-dasharray="{dash}"' if dash else ''
        o.append(f'<path d="{path}" fill="none" stroke="{c}" stroke-width="2"{d}/>')
        if r:
            o += [f'<circle cx="{X(t):.1f}" cy="{Y(v):.1f}" r="{r}" fill="{c}" stroke="#fcfcfb" stroke-width="1.5"/>' for t, v in pts]
        y = 70 + 20 * (series.index((label, c, pts, dash, r)) // 3)
        xx = L + 310 * (series.index((label, c, pts, dash, r)) % 3)
        o.append(f'<line x1="{xx}" x2="{xx+24}" y1="{y}" y2="{y}" stroke="{c}" stroke-width="2"{d}/>')
        o.append(f'<text x="{xx+30}" y="{y+4}" fill="#1f1f1d">{label}</text>')
    o.append(f'<text x="{L}" y="{T-8}" fill="#5f5e58" font-size="12">{note}</text>')
    o.append('</svg>')
    svg = os.path.join(out, name + '.svg')
    open(svg, 'w').write('\n'.join(o))
    # Quick Look renders the SVG (ImageMagick here has no SVG font); crop to
    # the drawing.
    ql = os.path.join(out, '.ql')
    os.makedirs(ql, exist_ok=True)
    subprocess.run(['qlmanage', '-t', '-s', str(W), '-o', ql, svg], check=True, capture_output=True)
    subprocess.run(['magick', os.path.join(ql, name + '.svg.png'), '-crop', f'{W}x{H}+0+0', '+repage',
                    os.path.join(out, name + '.png')], check=True)
    os.remove(svg)
    os.remove(os.path.join(ql, name + '.svg.png'))
    os.rmdir(ql)


ref = [(r['t'], r['ref'][0]) for r in cpu if r['t'] > 0.001]
chart('t5b-luma', 'Rising room interior: frame mean luma against travel t′ (mover-setpiece, --release, 5300M)',
      'Camera rides the room at (860, 320, floor + 72), yaw 0, pitch −8, fov 100; Rec. 709 luma 0–255; t′ every 0.025.',
      [('today (83dfb5317)', '#2a78d6', gpu('today'), '', 2.5),
       ('task 5 build (through-base)', '#eb6834', gpu('tb'), '', 2.5),
       ('enclosed mid pose blends across', '#1baf7a', gpu('blend'), '', 2.5),
       ('enclosed mid pose stores its dark state', '#b04fc4', gpu('dark'), '6 3', 0),
       ('static-brush reference (CPU model)', '#3d3d3a', ref, '2 4', 0)],
      18, 'Blend and dark overlap almost everywhere. The reference is a CPU model (no Chebyshev depth, no dynamic lights); it reads ~1 luma under the GPU on the same data.')
model = [(r['t'], r['new'][0]) for r in cpu]
today = [(r['t'], r['today'][0]) for r in cpu]
chart('t5b-cpu-model', 'CPU model of the interior frame against the GPU capture, same PRL data',
      'Model: section composed on the CPU by the renderer’s rules, 8 probes per hit as sh_sample.wgsl, no Chebyshev depth, baked lights only.',
      [('task 5 build, GPU (through-base)', '#eb6834', gpu('tb'), '', 2.5),
       ('task 5 build, CPU model', '#eb6834', model, '5 3', 0),
       ('today, GPU', '#2a78d6', gpu('today'), '', 2.5),
       ('today (light_poses 2), CPU model', '#2a78d6', today, '5 3', 0)],
      18, 'GPU line is the through-base capture; between knots it equals the depth-stripped frame (task 5). At knots exact depth darkens the GPU frame.')
print('charts written')
