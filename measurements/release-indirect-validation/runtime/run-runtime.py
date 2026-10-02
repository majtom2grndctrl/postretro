import os, subprocess, sys, time, json, re, signal
from pathlib import Path
root=Path(__file__).resolve().parents[3]
out=Path(__file__).resolve().parent
label,build,mapname,mode=sys.argv[1:]
env=os.environ.copy()
env.pop('WGPU_VALIDATION_INDIRECT_CALL',None)
env.update(RUST_LOG='info', POSTRETRO_CPU_TIMING='1', POSTRETRO_GPU_TIMING='1')
if mode=='on':env['WGPU_VALIDATION_INDIRECT_CALL']='1'
if mode=='off':env['WGPU_VALIDATION_INDIRECT_CALL']='0'
log=out/(label+'.log')
started=time.time()
with log.open('w') as f:
 p=subprocess.Popen([os.environ.get('RUNTIME_BINARY',str(root/'target'/build/'postretro')),str(root/'content/dev/maps'/mapname)],cwd=root,env=env,stdout=f,stderr=subprocess.STDOUT)
 print('engine_pid='+str(p.pid),flush=True)
 (out/(label+'.pid')).write_text(str(p.pid))
 checks=[]; sampler=None; activated=False; shot=False; profiling=os.environ.get("RUNTIME_PROFILE", "1")=="1"; trace=None; tracing=os.environ.get("RUNTIME_TRACE", "0")=="1"; paused=False
 try:
  while time.time()-started<150 and p.poll() is None:
   content=log.read_text(errors='replace'); windows=content.count('[CpuTiming]')
   action='activate' if not activated and 'Window ready' in content else 'check'
   if action=='activate':activated=True
   r=subprocess.run([str(out/'foreground'),str(p.pid),action],capture_output=True,text=True)
   checks.append({'elapsed':round(time.time()-started,2),'windows':windows,'state':r.stdout.strip(),'error':r.stderr.strip()})
   if activated and not shot and windows>=1:
    m=re.search(r'window_id=(\d+)',r.stdout)
    if m:
     capture=subprocess.run(['screencapture','-x','-l',m[1],str(out/(label+'.png'))],capture_output=True,text=True)
     (out/(label+'.screenshot-status.txt')).write_text(str(capture.returncode)+'\n'+capture.stderr)
    shot=True
   if profiling and windows>=2 and sampler is None:
    sampler=subprocess.Popen(['sample',str(p.pid),'10','1','-file',str(out/(label+'.sample.txt'))],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
   if tracing and windows>=(10 if profiling else 2) and trace is None:
    trace=subprocess.Popen(['xcrun','xctrace','record','--template','Metal System Trace','--attach',str(p.pid),'--time-limit','2s','--output',str(out/(label+'.trace'))],stdout=(out/(label+'.trace-status.txt')).open('w'),stderr=subprocess.STDOUT)
   if trace is not None and not paused and 'Reached specified time limit' in (out/(label+'.trace-status.txt')).read_text():
    p.send_signal(signal.SIGSTOP); paused=True
   if windows>=int(os.environ.get("RUNTIME_WINDOWS","2" if tracing and not profiling else "8")) and (not profiling or (sampler is not None and sampler.poll() is not None)) and (not tracing or (trace is not None and trace.poll() is not None)):break
   time.sleep(1)
 finally:
  if p.poll() is None:
   if paused:p.send_signal(signal.SIGCONT)
   p.terminate()
   try:p.wait(timeout=5)
   except subprocess.TimeoutExpired:p.kill();p.wait()
  if sampler is not None:sampler.wait(timeout=20)
  if trace is not None:trace.wait(timeout=20)
 elapsed=round(time.time()-started,2)
 record={'label':label,'pid':p.pid,'build':build,'map':mapname,'override':mode,'elapsed':elapsed,'exit':p.returncode,'timing_windows':log.read_text(errors='replace').count('[CpuTiming]'),'foreground_checks':checks}
 (out/(label+'.run.json')).write_text(json.dumps(record,indent=2)+'\n')
 print(json.dumps({k:v for k,v in record.items() if k!='foreground_checks'}),flush=True)
