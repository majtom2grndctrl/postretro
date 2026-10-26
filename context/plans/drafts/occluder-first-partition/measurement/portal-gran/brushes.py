import re, itertools, sys, json
def parse(path):
    txt=open(path).read().split('\n')
    brushes=[];cur=None;ent=-1;enttxt=[]
    for ln in txt:
        if ln.startswith('// entity'): ent=int(ln.split()[2])
        if ln.startswith('// brush'): cur=[]; continue
        if cur is not None:
            m=re.match(r'\(\s*(\S+) (\S+) (\S+) \)\s*\(\s*(\S+) (\S+) (\S+) \)\s*\(\s*(\S+) (\S+) (\S+) \)\s*(\S+)',ln)
            if m:
                v=list(map(float,m.groups()[:9])); cur.append((v[0:3],v[3:6],v[6:9],m.group(10)))
            elif ln.startswith('}'):
                brushes.append((ent,len(brushes),cur)); cur=None
    return brushes
def sub(a,b): return [a[i]-b[i] for i in range(3)]
def cross(a,b): return [a[1]*b[2]-a[2]*b[1],a[2]*b[0]-a[0]*b[2],a[0]*b[1]-a[1]*b[0]]
def dot(a,b): return sum(x*y for x,y in zip(a,b))
def plane(f):
    p0,p1,p2,_=f
    n=cross(sub(p2,p0),sub(p1,p0))  # quake winding: outward normal
    l=dot(n,n)**.5; n=[x/l for x in n]; return n,dot(n,p0)
def solve(pl):
    (n1,d1),(n2,d2),(n3,d3)=pl
    c23=cross(n2,n3); det=dot(n1,c23)
    if abs(det)<1e-9: return None
    a=[x*d1 for x in cross(n2,n3)]; b=[x*d2 for x in cross(n3,n1)]; c=[x*d3 for x in cross(n1,n2)]
    return [(a[i]+b[i]+c[i])/det for i in range(3)]
def aabb(faces):
    pls=[plane(f) for f in faces]; pts=[]
    for t in itertools.combinations(pls,3):
        p=solve(t)
        if p and all(dot(n,p)-d<=1e-4 for n,d in pls): pts.append(p)
    return [min(p[i] for p in pts) for i in range(3)],[max(p[i] for p in pts) for i in range(3)],pls
if __name__=='__main__':
    for ent,i,f in parse(sys.argv[1]):
        lo,hi,pls=aabb(f)
        print(ent,i,[round(x) for x in lo],[round(x) for x in hi],f[0][3], 'axis' if all(max(abs(c) for c in n)>.9999 for n,d in pls) else 'NONAXIS')
