# Reduce an xctrace time-profile export: samples under a root function, self and inclusive.
import sys, xml.etree.ElementTree as ET, collections, re
path=sys.argv[1]; root_fn=sys.argv[2] if len(sys.argv)>2 else "submit_windowed_frame"
ids={}
def res(e):
    r=e.get("ref")
    return ids[r] if r is not None else e
def reg(e):
    i=e.get("id")
    if i is not None: ids[i]=e
    for c in e: reg(c)
rows=[]
tree=ET.parse(path)
for node in tree.getroot().iter("node"):
    for row in node.findall("row"):
        reg(row)
        th=res(row.find("thread")); w=res(row.find("weight"))
        tb=row.find("tagged-backtrace")
        if tb is None: continue
        tb=res(tb); bt=tb.find("backtrace")
        if bt is None: continue
        bt=res(bt)
        frames=[res(f).get("name","?") for f in bt.findall("frame")]
        st=row.find("thread-state")
        rows.append((th.get("fmt",""), int(w.text) if w is not None and w.text else 1_000_000, frames))
span=None
tsamp=[]
# per-thread totals
thr=collections.Counter()
for t,w,f in rows: thr[t]+=w
main=max(thr,key=thr.get)
tot=sum(w for t,w,f in rows if t==main)
sub=[(w,f) for t,w,f in rows if t==main and any(root_fn in x for x in f)]
st=sum(w for w,f in sub)
print(f"file={path.split('/')[-1]} samples={len(rows)} main_thread='{main}' main_ms={tot/1e6:.1f} {root_fn}_ms={st/1e6:.1f} share={st/max(tot,1):.3f}")
def dem(n):
    if not n.startswith("_R"): return n
    out=[];i=2
    while i<len(n):
        if n[i].isdigit():
            j=i
            while j<len(n) and n[j].isdigit(): j+=1
            k=int(n[i:j]); 
            if j<len(n) and n[j]=="_": j+=1
            tok=n[j:j+k]; i=j+k
            if tok and not tok.startswith(("Cs","s")) : out.append(tok)
        else: i+=1
    return "::".join(out[-4:])
def short(n):
    return dem(n)[:150]
selfc=collections.Counter(); inc=collections.Counter()
for w,f in sub:
    # frames leaf-first; cut at root fn
    k=next(i for i,x in enumerate(f) if root_fn in x)
    chain=f[:k+1]
    selfc[short(chain[0])]+=w
    for x in set(short(y) for y in chain): inc[x]+=w
print("-- inclusive within root (ms over trace)")
for n,w in inc.most_common(int(sys.argv[3]) if len(sys.argv)>3 else 45): print(f"{w/1e6:9.1f}  {n}")
print("-- self (leaf) within root")
for n,w in selfc.most_common(25): print(f"{w/1e6:9.1f}  {n}")
