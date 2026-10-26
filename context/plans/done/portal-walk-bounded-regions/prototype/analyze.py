import csv,statistics,collections
rows=list(csv.DictReader(open('out/sweep.csv')))
for r in rows:
    for k in ('x','y','z'): r[k]=float(r[k])
    for k in ('considered','visible_drawable','trip','us','cam_cell','accepted','reach'): r[k]=int(r[k])
def pct(v,p):
    v=sorted(v); return v[round((len(v)-1)*p)] if v else 0
def rep(name,sel):
    st=[r['considered'] for r in sel]; vis=[r['visible_drawable'] for r in sel if not r['trip']]
    cells=len(set(r['cam_cell'] for r in sel))
    print(f"{name:34s} walks={len(sel):6d} cells={cells:4d} steps p50={pct(st,.5):6d} p95={pct(st,.95):6d} p99={pct(st,.99):6d} max={max(st) if st else 0:6d} trips={sum(r['trip'] for r in sel):3d} vis p50={pct(vis,.5)} p95={pct(vis,.95)} max={max(vis) if vis else 0}")
def box(lo,hi,m=(8,2,8)):
    return lambda r: all(lo[i]-m[i]<=r[('x','y','z')[i]]<=hi[i]+m[i] for i in range(3))
regions={
 'shaft A (-42,z21) trips':box((-46.13,18.49,17.27),(-38.4,33.73,24.99)),
 'shaft B (84,z-105)':box((80.67,18.49,-109.52),(88.39,33.73,-101.8)),
 'shaft C (-42,z-105)':box((-46.13,18.49,-109.52),(-38.4,33.73,-101.8)),
 'arena walk stairs (52,y2-10,z12-31)':box((48.77,1.63,11.68),(55.27,9.86,30.89),(4,2,4)),
}
rep('ALL',rows)
for n,f in regions.items(): rep(n,[r for r in rows if f(r)])
# arena: find region with large visible sets on ground layer near walk stairs; use cells whose sample y<16 and x>20
big=collections.defaultdict(list)
for r in rows: big[r['cam_cell']].append(r)
# rank cells by median visible
cs=sorted(big.items(), key=lambda kv:-statistics.median([r['visible_drawable'] for r in kv[1]]))
print('top cells by median visible drawable:')
for c,rs in cs[:12]:
    r0=rs[0]; st=[r['considered'] for r in rs]
    print(f"  cell {c} pos=({r0['x']:.1f},{r0['y']:.1f},{r0['z']:.1f}) vis med={statistics.median([r['visible_drawable'] for r in rs])} max={max(r['visible_drawable'] for r in rs)} steps med={statistics.median(st)} max={max(st)} trips={sum(r['trip'] for r in rs)}")
