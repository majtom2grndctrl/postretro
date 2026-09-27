import re,sys,collections
S=0.0254
txt=open(sys.argv[1]).read()
pts=re.compile(r'\(\s*(-?[\d.]+)\s+(-?[\d.]+)\s+(-?[\d.]+)\s*\)')
brushes=[];cur=None;depth=0;ent=None
for line in txt.splitlines():
    l=line.strip()
    if l.startswith('"classname"'): ent=l
    if l=='{':
        depth+=1
        if depth==2: cur=[]
    elif l=='}':
        if depth==2 and cur: brushes.append((ent,cur))
        depth-=1
    elif depth==2 and l.startswith('('):
        for m in pts.findall(l): cur.append(tuple(map(float,m)))
def eng(x,y,z): return (-y*S, z*S, -x*S)
kinds=collections.Counter(); plats=[]; walks=[]
for ent,p in brushes:
    xs=[a[0] for a in p]; ys=[a[1] for a in p]; zs=[a[2] for a in p]
    dx,dy,dz=max(xs)-min(xs),max(ys)-min(ys),max(zs)-min(zs)
    if sorted([dx,dy])==[112,112] and dz==24: plats.append((min(xs),min(ys),min(zs),max(xs),max(ys),max(zs)))
    if (dx==28 and dy==256) or (dy==28 and dx==256): walks.append((min(xs),min(ys),min(zs),max(xs),max(ys),max(zs)))
print('brushes',len(brushes),'platforms',len(plats),'walk steps',len(walks))
def ebox(b):
    a=eng(b[0],b[1],b[2]); c=eng(b[3],b[4],b[5])
    return tuple(round(min(a[i],c[i]),2) for i in range(3)), tuple(round(max(a[i],c[i]),2) for i in range(3))
# group platforms by shaft (cluster by xy within 400)
groups=[]
for b in plats:
    cx,cy=(b[0]+b[3])/2,(b[1]+b[4])/2
    for g in groups:
        if abs(g[0]-cx)<300 and abs(g[1]-cy)<300: g[2].append(b);break
    else: groups.append([cx,cy,[b]])
for g in groups:
    bs=g[2]; lo=[min(b[i] for b in bs) for i in range(3)]; hi=[max(b[i+3] for b in bs) for i in range(3)]
    print('shaft spiral n=%d engine box'%len(bs), ebox(lo+hi))
if walks:
    lo=[min(b[i] for b in walks) for i in range(3)]; hi=[max(b[i+3] for b in walks) for i in range(3)]
    print('walk stairs engine box', ebox(lo+hi))
