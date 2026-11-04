import json, os, subprocess
S=0.0254; R="/Users/dhiester/Projects/Personal/postretro"; H=os.path.dirname(os.path.abspath(__file__))
O=f"{H}/final_out"; os.makedirs(O,exist_ok=True); os.makedirs(f"{H}/final_scenes",exist_ok=True)
eng=lambda x,y,z:[-y*S,z*S,-x*S]
env=dict(os.environ,POSTRETRO_SH_STREAMING="sync-proof")
V={"door":((320,720,64),270,-6),"threshold":((320,610,96),270,-38),"bright":((320,200,48),90,0)}
M={"today":"mover-closet","reference":".mover-closet-static-ref","flush":".mover-closet-flush-ref","nostraddle":".mover-closet-nostraddle-ref"}
for mk,m in M.items():
  for v,(p,yaw,pitch) in V.items():
    for k,bits in (("all",None),("direct",8)):
      n=f"{mk}_{v}_{k}"
      sc={"map":f"{R}/content/dev/maps/{m}.prl","camera":{"position":eng(*p),"yaw_deg":yaw,"pitch_deg":pitch,"fov_deg":100.0},"resolution":[960,540],"output":f"{O}/{n}.png","force_full_resident_sh_compose":True}
      if bits: sc["light_term_mask"]=bits
      f=f"{H}/final_scenes/{n}.json"; json.dump(sc,open(f,"w"))
      r=subprocess.run(["nice","-n","5",f"{R}/target/debug/postretro","--capture",f],cwd=R,env=env,capture_output=True,text=True)
      if r.returncode: print("FAIL",n,r.stderr[-300:])
