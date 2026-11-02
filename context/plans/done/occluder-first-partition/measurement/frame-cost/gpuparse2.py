import sys, re, xml.etree.ElementTree as ET, collections
f=sys.argv[1]
tree=ET.parse(f); ids={}
for e in tree.iter():
    if 'id' in e.attrib: ids[e.attrib['id']]=e
def val(e): return ids[e.attrib['ref']] if 'ref' in e.attrib else e
rows=[]
for r in tree.iter('row'):
    ch=[val(c) for c in r]
    start=int(ch[0].text); dur=int(ch[1].text); chan=ch[2].attrib.get('fmt'); lab=ch[6].attrib.get('fmt','')
    proc=''.join(c.attrib.get('fmt','') for c in ch if c.tag=='process')
    if 'postretro' not in proc: continue
    name=lab.split(':')[0].strip()
    name=re.sub(r'\s+0x[0-9a-f]+$','',name)
    rows.append((start,dur,chan,name))
# union per name per frame? simple: sum durations by (chan,name)
agg=collections.defaultdict(lambda:[0,0,0])
for s,d,c,n in rows:
    a=agg[(c,n)]; a[0]+=1;a[1]+=d;a[2]=max(a[2],d)
frames=sum(a[0] for (c,n),a in agg.items() if n.startswith('Screen Effects Resolve'))
span=(max(r[0]+r[1] for r in rows)-min(r[0] for r in rows))/1e9
print('resolve passes(frames)',frames,'span s',round(span,2))
tot=0
for k,a in sorted(agg.items(),key=lambda x:-x[1][1])[:int(sys.argv[2]) if len(sys.argv)>2 else 30]:
    print(f"{k[0]:12s} {k[1][:48]:48s} n={a[0]:5d} per-frame={a[1]/1e6/max(frames,1):7.3f} ms  mean={a[1]/a[0]/1e6:7.3f} max={a[2]/1e6:7.3f}")
