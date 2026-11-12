# Seam/face-band scan: for an authored light_poses N, the smallest distance
# (mm) between any travel-axis brush plane at any pose and a lattice layer.
import sys; sys.path.insert(0, '.')
import a2map
R = 'C:/Users/danhi/Projects/Personal/postretro/content/dev/maps/'
def planes(mv):
    a, b = mv['waypoints'][0], mv['waypoints'][-1]
    d = [b[i] - a[i] for i in range(3)]
    ax = max(range(3), key=lambda i: abs(d[i]))
    ps = sorted({v for lo, hi in mv['brushes'] for v in (lo[ax], hi[ax])})
    return ax, d[ax], ps
def scan(mapname, mover, n, lat=None):
    lat = lat or a2map.lattice(R + mapname + '.prl')
    mv = [m for m in a2map.movers(R + mapname + '.map') if m['name'] == mover][0]
    ax, L, ps = planes(mv)
    layers = a2map.layers_map(lat, ax)
    best = []
    for i, t in enumerate(a2map.pose_ts(n)):
        for p in ps:
            q = p + L * t
            for l in layers:
                best.append((abs(q - l) * 25.4, i, t, p, l))
    best.sort()
    return best
if __name__ == '__main__':
    mapname, mover = sys.argv[1], sys.argv[2]
    for n in map(int, sys.argv[3].split(',')):
        b = scan(mapname, mover, n)
        near = [x for x in b if x[0] < 2.54]
        print(f'{mover} N={n}: min {b[0][0]:.3f} mm (pose {b[0][1]}, plane {b[0][3]}, layer {b[0][4]:.3f}); within 2.54 mm: {len(near)}',
              ' '.join(f'[p{x[1]} {x[0]:.2f}mm pl{x[3]:g}]' for x in near[:6]))
