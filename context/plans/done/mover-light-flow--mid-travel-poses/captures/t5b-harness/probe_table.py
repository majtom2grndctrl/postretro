# Task 5b: one probe's per-pose and per-t story from a diag dump, read along
# one axis normal: enclosure, domain, bounce mark, q per light, each pose's
# bounce and direct, and the composed values against the reference at each t.
# Values are luma levels of a mid-grey receiver (display, 0-255), bounce at
# the indirect scale 0.33, direct at weight 1.
#
#   python3 -I probe_table.py <dump.jsonl> <x,y,z quake> <normal index 0-5: +x -x +y -y +z -z>
import json, sys
sys.path.insert(0, __file__.rsplit('/', 1)[0])
from cpu_interior import luma, INDIRECT

path, at, ni = sys.argv[1], [float(v) for v in sys.argv[2].split(',')], int(sys.argv[3])
header = None
best = None
for line in open(path):
    row = json.loads(line)
    if row['kind'] == 'header':
        header = row
        continue
    d = sum((row['q'][a] - at[a]) ** 2 for a in range(3))
    if best is None or d < best[0]:
        best = (d, row)
p = best[1]
print(f"probe {p['p']} quake {p['q']} valid {p['valid']} domain {p['domain']} knots {[round(k, 3) for k in header['knots']]}")
print(f"  enclosed per pose {''.join('E' if e else '.' for e in p['enc'])}, bounce marks {p.get('marks')}")
print(f"  q per light per pose {p.get('q8')}")
b = lambda v: luma([INDIRECT * c for c in v])
print('  pose | bounce  direct')
for k in range(header['pose_count']):
    print(f"  {k:4d} | {b(p['pose_b'][k][ni]):6.2f} {luma(p['pose_d'][k][ni]):6.2f}")
print(f"  base bounce {b(p['base_b'][ni]):.2f}")
print('  t     | composed bounce direct | reference bounce direct')
for row in p['at']:
    rb = f"{b(row['rb'][ni]):6.2f} {luma(row['rd'][ni]):6.2f}" if row['rvalid'] else '  (invalid)'
    print(f"  {row['t']:.3f} | {b(row['b'][ni]):6.2f} {luma(row['d'][ni]):6.2f}        | {rb}")
