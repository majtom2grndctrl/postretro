import xml.etree.ElementTree as E,collections,re
r=E.parse('/tmp/planar-profile/planar-gpu.xml').getroot();ids={e.get('id'):e for e in r.iter() if e.get('id')}
def el(e):return ids[e.get('ref')] if e.get('ref') else e
def fmt(e):return el(e).get('fmt','')
c=collections.Counter();s=collections.Counter();example=None
for row in r.findall('.//row'):
 a=list(row)
 if 'postretro' in fmt(a[10]):
  label=re.sub(r'\s+\( postretro.*','',fmt(a[6]));key=(fmt(a[5]),label); c[key]+=1;s[key]+=int(el(a[1]).text)
  if 'Planar World Forward' in label:example=a
for k,n in c.most_common(45):print(n,round(s[k]/n/1e6,4),k)
if example: print('EXAMPLE',[(i,el(e).tag,fmt(e),el(e).text) for i,e in enumerate(example)])
