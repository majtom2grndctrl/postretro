import json,os,subprocess,time
from pathlib import Path
root=Path(__file__).resolve().parent
for i,mode in enumerate(['ssr-medium','ssr-high','planar','planar','ssr-high','ssr-medium']):
 name=f'repeat-{i+1}-{mode}'
 s=json.loads((root/(mode+'-scene.json')).read_text());s['output']=str(root/(name+'.png'));s['measurement']['report']=str(root/(name+'.json'))
 scene=root/(name+'-scene.json');scene.write_text(json.dumps(s,indent=2))
 env=os.environ.copy();env.update(POSTRETRO_PLANAR_REFLECTION='1' if mode=='planar' else '0',POSTRETRO_CPU_TIMING='1',POSTRETRO_SH_STREAMING='sync-proof',RUST_LOG='info')
 with (root/(name+'.log')).open('w') as out:
  result=subprocess.run(['target/debug/postretro','--capture',str(scene)],env=env,stdout=out,stderr=subprocess.STDOUT,timeout=60)
 if result.returncode:raise SystemExit(f'{name} failed {result.returncode}')
 d=json.loads((root/(name+'.json')).read_text());print(name,d['cpu_completion']['median_ms'],flush=True)
