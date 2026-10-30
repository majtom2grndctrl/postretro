# usage: tpcmp.py frame_ms xml [frame_ms xml ...] -> ms/frame per key bucket, main thread
import sys,re,collections,xml.etree.ElementTree as ET
KEYS=[
("render_frame_indirect","render_frame_indirect"),
("encoder finish (in render_submit)","command_encoder_finish"),
("  encode_render_pass","encode_render_pass"),
("    hal Metal draw_indexed_indirect","wgpu_hal5metal7commandNtB7_14CommandEncoderNtB9_14CommandEncoder21draw_indexed_indirect"),
("      AMD driver drawIndexedPrimitives","edPrimitives:indexType"),
("    indirect validation (wgpu-core)","indirect_validation"),
("  objc2 msg_send_check (debug_assertions)","msg_send_check"),
("queue_submit","queue_submit"),
("record_scene_passes (render_record)","record_scene_passes"),
("  cull diagnostics estimate (dev-tools)","estimate_diagnostics"),
("visibility","determine_visible_cells"),
("refresh_camera_cull_diagnostics (dev-tools, redraw)","refresh_camera_cull_diagnostics"),
("  candidate count_submitted (dev-tools)","count_submitted_candidates"),
("level_streaming prepare_drains (render_prep)","prepare_streaming_drains"),
("set_spatial_diagnostics (dev-tools)","set_spatial_diagnostics"),
("SpatialCellSetDiagnostics (dev-tools)","SpatialCellSetDiagnostics"),
]
def load(path):
    ids={}
    def reg(e):
        i=e.get("id")
        if i is not None: ids[i]=e
        for c in e: reg(c)
    res=lambda e: ids[e.get("ref")] if e.get("ref") is not None else e
    out=[]; t0=t1=None
    for row in ET.parse(path).getroot().iter("row"):
        reg(row)
        th=res(row.find("thread")).get("fmt","")
        if not th.startswith("Main Thread"): continue
        t=int(res(row.find("sample-time")).text); t0=t if t0 is None else min(t0,t); t1=t if t1 is None else max(t1,t)
        w=int(res(row.find("weight")).text)
        tb=row.find("tagged-backtrace")
        if tb is None: continue
        bt=res(res(tb).find("backtrace"))
        out.append((w,[res(f).get("name","") for f in bt.findall("frame")]))
    return out,(t1-t0)/1e6
args=sys.argv[1:]
cols=[]
for i in range(0,len(args),2):
    fms=float(args[i]); rows,span=load(args[i+1]); nfr=span/fms
    acc=collections.Counter()
    for w,fr in rows:
        for lab,k in KEYS:
            if any(k in x for x in fr): acc[lab]+=w
    # objc verify within finish only
    vf=sum(w for w,fr in rows if any("command_encoder_finish" in x for x in fr) and any("msg_send_check" in x for x in fr))
    acc["objc2 msg_send_check inside encoder finish"]=vf
    vd=sum(w for w,fr in rows if any("21draw_indexed_indirect" in x for x in fr) and any("msg_send_check" in x for x in fr))
    acc["objc2 msg_send_check inside hal draw"]=vd
    dv=sum(w for w,fr in rows if any("command_encoder_finish" in x for x in fr) and any("DrawBatcher" in x or "indirect_validation" in x for x in fr))
    acc["indirect_validation inside finish"]=dv
    cols.append((args[i+1].split("/")[-1],nfr,{k:v/1e6/nfr for k,v in acc.items()}))
labs=[l for l,_ in KEYS]+["objc2 msg_send_check inside encoder finish","objc2 msg_send_check inside hal draw","indirect_validation inside finish"]
print("ms per frame (main thread samples / frames in trace span)")
print(f"{'bucket':42s}"+"".join(f"{c[0].split('.')[1]+'.'+c[0].split('.')[2]:>22s}" for c in cols))
print(f"{'frames in span':42s}"+"".join(f"{c[1]:22.0f}" for c in cols))
for l in labs: print(f"{l:42s}"+"".join(f"{c[2].get(l,0):22.2f}" for c in cols))
