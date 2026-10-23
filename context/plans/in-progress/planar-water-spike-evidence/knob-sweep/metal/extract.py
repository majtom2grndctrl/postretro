import xml.etree.ElementTree as E,collections,re,json,statistics,sys
from pathlib import Path
root=Path(__file__).resolve().parent
def stats(v):
 v=sorted(v)
 return {'count':len(v),'mean_ms':statistics.mean(v),'median_ms':statistics.median(v),'p95_ms':v[max(0,__import__('math').ceil(len(v)*.95)-1)]}
def extract(name):
 r=E.parse(root/(name+'-gpu.xml')).getroot();ids={e.get('id'):e for e in r.iter() if e.get('id')}
 def el(e):return ids[e.get('ref')] if e.get('ref') else e
 def fmt(e):return el(e).get('fmt','')
 frames=collections.defaultdict(list)
 for row in r.findall('.//row'):
  a=list(row)
  if 'postretro' not in fmt(a[10]) or fmt(a[11])!='AMD Radeon Pro 5300M':continue
  label=re.sub(r'\s+\( postretro.*','',fmt(a[6]))
  frames[el(a[15]).text].append({'start_ns':int(el(a[0]).text),'duration_ns':int(el(a[1]).text),'label':label,'channel':fmt(a[2])})
 allrows=[x for rows in frames.values() for x in rows if x['label']!='GPU Execution']
 allrows.sort(key=lambda x:x['start_ns'])
 anchors=[x['start_ns'] for x in allrows if x['label'].startswith('Depth Pre-Pass:')]
 # Each measured render starts with one main depth prepass. Last 120 render intervals exclude warmup.
 main=[]
 for i,t in enumerate(anchors):
  end=anchors[i+1] if i+1<len(anchors) else 10**30
  main.append([x for x in allrows if t<=x['start_ns']<end])
 main=main[-121:-1]
 planar_mode=any(x['label'].startswith('Planar ') for x in allrows)
 passes=collections.defaultdict(list);reflection=[];active=[];span=[];perframe=[]
 for rows in main:
  p=collections.defaultdict(int)
  for x in rows:p[x['label']]+=x['duration_ns']
  for k,v in p.items():passes[k].append(v/1e6)
  names=['Planar Candidate Cull','Planar Water Mask','Planar World Prepass','Planar World Forward','Planar Resolve'] if planar_mode else ['Fluid Reflection Pass']
  keys=[f'{n}:{n}' for n in names]
  ref=sum(p[k] for k in keys)/1e6 if all(k in p for k in keys) else None
  if ref is not None: reflection.append(ref)
  intervals=sorted((x['start_ns'],x['start_ns']+x['duration_ns']) for x in rows if x['channel']=='GFX')
  merged=[]
  for s,e in intervals:
   if merged and s<=merged[-1][1]:merged[-1][1]=max(e,merged[-1][1])
   else:merged.append([s,e])
  active.append(sum(e-s for s,e in merged)/1e6)
  span.append((max(e for s,e in merged)-min(s for s,e in merged))/1e6)
  perframe.append({'reflection_pass_sum_ms':ref,'gfx_active_union_ms':active[-1],'gfx_span_ms':span[-1]})
 d=json.load(open(root/(name+'.json')))
 out={'mode':name,'frames':len(main),'pass_ms':{k:stats(v) for k,v in passes.items()},'reflection_pass_sum_ms':stats(reflection),'gfx_active_union_ms':stats(active),'gfx_span_ms':stats(span),'frames_ms':perframe,'cpu_completion':d['cpu_completion'],'cpu_stages':d['cpu_stages']}
 (root/(name+'-summary.json')).write_text(json.dumps(out,indent=2)+'\n')
 print(name,'frames',len(main))
 for k in ['reflection_pass_sum_ms','gfx_active_union_ms','gfx_span_ms']:print(k,out[k])
 for k,v in out['pass_ms'].items():
  if any(t in k for t in ['Planar','Fluid','Textured','Depth Pre','validation']):print(k,v)
for n in sys.argv[1:]:extract(n)
