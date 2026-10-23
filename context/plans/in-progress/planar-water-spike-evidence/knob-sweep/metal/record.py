import json,os,subprocess
from pathlib import Path
root=Path(__file__).resolve().parent
for name in ['planar-half','planar-quarter','d-15','r-8','ssr-high']:
 scene=json.load(open('target/planar-water-sweep/across-'+name+'-run1-scene.json'))
 scene['output']=str(root/(name+'.png'));scene['measurement']['report']=str(root/(name+'.json'))
 path=root/(name+'-scene.json');path.write_text(json.dumps(scene,indent=2)+'\n')
 command=['xcrun','xctrace','record','--template','Metal System Trace','--output',str(root/(name+'.trace')),'--time-limit','30s','--no-prompt',
 '--env','POSTRETRO_PLANAR_REFLECTION=0','--env','WGPU_VALIDATION_INDIRECT_CALL=1','--env','POSTRETRO_CPU_TIMING=1','--env','POSTRETRO_SH_STREAMING=sync-proof','--env','RUST_LOG=info',
 '--target-stdout',str(root/(name+'.log')),'--launch','--',str(Path('target/debug/postretro').resolve()),'--capture',str(path)]
 print('Recording',name,flush=True)
 result=subprocess.run(command,timeout=240)
 if result.returncode:raise SystemExit(f'record failed {name}: {result.returncode}')
 if not (root/(name+'.json')).exists():raise SystemExit('capture report missing')
 print('Exporting',name,flush=True)
 subprocess.run(['xcrun','xctrace','export','--input',str(root/(name+'.trace')),'--xpath','/trace-toc/run[@number="1"]/data/table[@schema="metal-gpu-intervals"]','--output',str(root/(name+'-gpu.xml'))],check=True,timeout=180)
 print('Finished',name,flush=True)
