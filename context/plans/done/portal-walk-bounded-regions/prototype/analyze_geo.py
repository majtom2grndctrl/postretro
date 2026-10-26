import csv
def pct(v,p):
    v=sorted(v); return v[round((len(v)-1)*p)] if v else 0
def load(d): return list(csv.DictReader(open(d+'/alt_sweep.csv')))
def rep(name, rows):
    ex=[int(r['ex_steps']) for r in rows]; eu=[int(r['ex_ns'])/1000 for r in rows]
    print(f"--- {name}: walks={len(rows)} exact steps p99/max {pct(ex,.99)}/{max(ex)} us p99/p99.9 {pct(eu,.99):.0f}/{pct(eu,.999):.0f}")
    res={}
    for v in ('rect256','oct256clip'):
        s=[int(r[v+'_steps']) for r in rows]; u=[int(r[v+'_ns'])/1000 for r in rows]
        e=[int(r[v+'_draw'])-int(r['ex_draw']) for r in rows]; ra=[int(r[v+'_draw'])/max(1,int(r['ex_draw'])) for r in rows]
        m=sum(int(r[v+'_miss_reach'])>0 for r in rows); res[v]=sum(e)
        print(f"  {v:11s} steps p99/max {pct(s,.99)}/{max(s)} us p99/p99.9 {pct(u,.99):.0f}/{pct(u,.999):.0f} extra p50/95/max {pct(e,.5)}/{pct(e,.95)}/{max(e)} ratio p95/max {pct(ra,.95):.2f}/{max(ra):.2f} mean_extra {sum(e)/len(e):.2f} misses {m}")
    print(f"  oct/rect total extra cells = {res['oct256clip']/max(1,res['rect256']):.2f}")
base=load('out_alt')
rep('hallway baseline (all)', base)
for nm,f in [('axis headings, pitch 0',lambda r: float(r['yaw'])%90==0 and float(r['pitch'])==0),
             ('diag(45) headings, pitch 0',lambda r: float(r['yaw'])%90!=0 and float(r['pitch'])==0),
             ('axis headings, pitch +-45',lambda r: float(r['yaw'])%90==0 and float(r['pitch'])!=0),
             ('diag headings, pitch +-45',lambda r: float(r['yaw'])%90!=0 and float(r['pitch'])!=0)]:
    rep('hallway '+nm,[r for r in base if f(r)])
for d,nm in (('out_geo/hall_off15','hallway yaw+15 (=world rotated 15/30deg rel.)'),('out_geo/hall_off22','hallway yaw+22.5')):
    rows=load(d); rep(nm,rows)
    rep(nm+' pitch 0',[r for r in rows if float(r['pitch'])==0])
rep('movement-feel',load('out_geo/mf'))
