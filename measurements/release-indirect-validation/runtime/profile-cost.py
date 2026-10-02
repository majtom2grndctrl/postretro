import sys,re,datetime,statistics,json
from pathlib import Path
p=Path('measurements/release-indirect-validation/runtime');name=sys.argv[1]
s=(p/(name+'.sample.txt')).read_text();tree=s.split('Total number in stack')[0]
main=int(re.search(r'^\s+(\d+) Thread_\d+(?:: main|\s+DispatchQueue_\d+: com.apple.main-thread)',tree,re.M)[1])
counts={}
for key,pattern in [('draw_batcher_add',r'DrawBatcher\S*add'),('inject_validation_pass',r'inject_validation_pass')]:
 matches=re.findall(r'^.*?\b(\d+) _R\S*'+pattern+r'\s+\(in postretro\)',tree,re.M);counts[key]=sum(map(int,matches))
start=datetime.datetime.strptime(re.search(r'Date/Time:\s+(.*)',s)[1].strip(),'%Y-%m-%d %H:%M:%S.%f %z');end=start+datetime.timedelta(seconds=10)
windows=[];previous=None
for line in (p/(name+'.log')).read_text().splitlines():
 if '[CpuTiming]' not in line:continue
 t=datetime.datetime.fromisoformat(line[1:21].replace('Z','+00:00'))
 if previous is not None and previous>=start and t<=end:
  windows.append({'end':t.isoformat(),'total_ms':float(re.search(r' total=([\d.]+)/',line)[1]),'render_submit_ms':float(re.search(r' render_submit=([\d.]+)/',line)[1])})
 previous=t
avg=statistics.mean(w['total_ms'] for w in windows) if windows else None
out={'profile':name+'.sample.txt','main_thread_samples':main,'validation_inclusive_samples':counts,'fully_contained_timing_windows':windows,'estimated_validation_cpu_ms_per_frame':sum(counts.values())/main*avg if avg is not None else None,'method':'Inclusive validation-function samples / main-thread samples × mean total frame time from complete windows contained in the 10-second in-level profile. Statistical estimate; timing acceptance uses separate unsampled runs.'}
(p/(name+'.profile-summary.json')).write_text(json.dumps(out,indent=2)+'\n');print(json.dumps(out,indent=2))
