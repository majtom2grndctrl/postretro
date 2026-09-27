import csv,statistics
rows=list(csv.DictReader(open('out/sweep.csv')))
def pct(v,p):
    v=sorted(v); return v[round((len(v)-1)*p)] if v else 0
# arena guess: walk stairs at x 48.8..55.3; NFL arena 110x49m. Search window around it, ground layer y<16
for (name,lo,hi) in [('arena window x20..100,z-60..60,y<16',(20,0,-60),(100,16,60)),('arena window wide x0..100 y<16',(0,0,-120),(100,16,120))]:
    sel=[r for r in rows if lo[0]<=float(r['x'])<=hi[0] and lo[1]<=float(r['y'])<=hi[1] and lo[2]<=float(r['z'])<=hi[2]]
    st=[int(r['considered']) for r in sel]; vis=[int(r['visible_drawable']) for r in sel]
    print(name,'walks',len(sel),'steps p50',pct(st,.5),'p95',pct(st,.95),'max',max(st),'vis p50',pct(vis,.5),'p95',pct(vis,.95),'max',max(vis), 'walks vis>50:',sum(v>50 for v in vis))
sel=[r for r in rows if int(r['visible_drawable'])>50]
print('all walks with visible>50:',len(sel))
import collections
reg=collections.Counter((round(float(r['x'])/20)*20, round(float(r['y'])/16)*16, round(float(r['z'])/20)*20) for r in sel)
print(reg.most_common(12))
