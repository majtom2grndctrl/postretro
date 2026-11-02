import re,sys
sys.path.insert(0,'.')
from brushes import parse,plane,aabb
import os; MAP=os.path.join(os.path.dirname(os.path.abspath(__file__)), '../../../../../../content/dev/maps/occlusion-test.map')
B=[(ent,i,[plane(f) for f in faces],aabb(faces)) for ent,i,faces in parse(MAP)]
for ln in open(sys.argv[1]):
    if not ln.startswith('PORTAL'): continue
    m=re.search(r'PORTAL (\d+) .*?aspect=([\d.]+) w=([\d.]+) h=([\d.]+).*?Q\[([^\]]*)\]\.\.\[([^\]]*)\] \| n=\[([^\]]*)\] d=(\S+)',ln)
    idx,asp,w,h,lo,hi,n,d=m.groups(); n=[float(x) for x in n.split(',')]; d=float(d)
    lo=[float(x) for x in lo.split(',')]; hi=[float(x) for x in hi.split(',')]
    hits=[]
    for ent,i,pls,(blo,bhi,_) in B:
        for (bn,bd) in pls:
            dotn=sum(a*b for a,b in zip(n,bn))
            if abs(abs(dotn)-1)<1e-3 and abs(bd*(1 if dotn>0 else -1)-d)<1.0:
                hits.append(f"brush{i}{[round(x) for x in blo]}..{[round(x) for x in bhi]}"); break
    print(f"portal {idx} aspect={asp} width={float(w)/1:.2f}m height={h}m Q{lo}..{hi} plane n={n} d={d}: {len(hits)} coincident brush faces: {hits[:5]}")
