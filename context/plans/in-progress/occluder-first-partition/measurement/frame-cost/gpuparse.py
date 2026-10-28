import sys, xml.etree.ElementTree as ET, collections
f=sys.argv[1]
tree=ET.parse(f); 
ids={}
def val(e):
    if e is None: return None
    if 'ref' in e.attrib: e=ids[e.attrib['ref']]
    return e
for e in tree.iter():
    if 'id' in e.attrib: ids[e.attrib['id']]=e
rows=[]
for r in tree.iter('row'):
    ch=list(r)
    d={}
    names=['start','duration','channel','frame','lat','depth','label','state']
    out=[]
    for c in ch[:8]:
        c=val(c); out.append(c)
    start=int(out[0].text); dur=int(out[1].text); chan=out[2].attrib.get('fmt'); depth=out[5].text
    lab=out[6].attrib.get('fmt','') if out[6] is not None else ''
    proc=''
    for c in ch:
        c=val(c)
        if c.tag=='process': proc=c.attrib.get('fmt','')
    rows.append((start,dur,chan,depth,lab,proc))
print(len(rows))
agg=collections.defaultdict(lambda:[0,0,0])
for s,d,ch,dep,lab,proc in rows:
    if 'postretro' not in proc: continue
    k=(ch,dep,lab[:70]); a=agg[k]; a[0]+=1;a[1]+=d;a[2]=max(a[2],d)
for k,a in sorted(agg.items(),key=lambda x:-x[1][1])[:60]: print(k,a[0],round(a[1]/1e6,2),'ms total mean',round(a[1]/a[0]/1e6,3),'max',round(a[2]/1e6,3))
