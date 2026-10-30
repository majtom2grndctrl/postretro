import re,sys,glob,os
S=sys.argv[1]; B=sys.argv[2]
def sweep(side,m):
    d={}
    for l in open(f"{S}/{side}.{m}.sweep"):
        if not l.startswith("pose "): continue
        p=l.split(); lab=p[1]
        kv=dict(x.split("=",1) for x in p[7:] if "=" in x)
        d[lab]=kv
    return d
def cpu(path):
    t=open(path).read().split()
    return dict(x.split("=",1) for x in t if "=" in x)
rows=[]
for f in sorted(glob.glob(f"{B}/*.r1.cpu.txt")):
    base=os.path.basename(f); m,lab,side=base.split(".")[0],None,None
    meta=open(f.replace(".cpu.txt",".meta")).read()
    mm=re.search(r"tag=\S+ side=(\S+) map=(\S+) pose=(\S+)",meta); side,m=mm.group(1),mm.group(2)
    c=cpu(f)
    # label from tag
    tag=re.search(r"tag=(\S+)",meta).group(1)
    sw=sweep(side,m)
    live=float(c["walk_considered"])
    # find label whose safe() matches
    cands=[k for k in sw if not k.endswith("~e150")]
    safe=lambda s:s.replace(":","_").replace("/","_").replace("@","_")
    lab=[k for k in cands if f"{m}.{safe(k)}.{side}.r1"==tag][0]
    a,b=sw[lab],sw[lab+"~e150"]
    pick=min((a,b),key=lambda kv:abs(float(kv["considered"])-live))
    rows.append((m,lab,side,live,int(pick["considered"]),int(pick["drawn"]),int(pick["drawn_faces"]),int(pick["tris"]),float(c["render_submit"]),float(c["render_record"]),float(c["work"]),float(c.get("rec_cull","nan"))))
print("map label side live_tests sweep_tests drawn_cells drawn_faces tris submit record work rec_cull")
for r in rows: print(*r)
import statistics
xs=[r[6] for r in rows]; ys=[r[8] for r in rows]
n=len(xs); mx=sum(xs)/n; my=sum(ys)/n
sxx=sum((x-mx)**2 for x in xs); sxy=sum((x-mx)*(y-my) for x,y in zip(xs,ys))
b=sxy/sxx; a=my-b*mx; ss=sum((y-a-b*x)**2 for x,y in zip(xs,ys)); st=sum((y-my)**2 for y in ys)
print(f"fit submit = {a:.3f} + {b*1000:.3f} us * drawn_faces  R2={1-ss/st:.3f} n={n}")
for r in rows: print(f"  {r[0][:6]} {r[1]:24s} {r[2]:6s} faces={r[6]:5d} submit={r[8]:.2f} pred={a+b*r[6]:.2f} resid={r[8]-a-b*r[6]:+.2f}")
