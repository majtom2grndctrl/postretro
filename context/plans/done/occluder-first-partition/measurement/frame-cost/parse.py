import re,glob,sys,statistics as st
def parse(line):
    d={}
    for m in re.finditer(r'(\w+)=([\d.]+)/([\d.]+)(?:ms)?(?:\((\d+)/(\d+)\))?',line): d[m.group(1)]=(float(m.group(2)),float(m.group(3)))
    m=re.search(r'frames=\d+ total=([\d.]+)/([\d.]+)',line)
    return d
rows={}
keys=['total','work','wait','visibility','portal_walk','walk_considered','walk_accepted','render','render_record','render_submit','rec_pre_scene','rec_cull','rec_cull_diagnostics','rec_ui','rec_shadow_depth','rec_shadow_reach','rec_forward','lightmap_residency','lightmap_drain','render_sh_drain','rec_light_slots','rec_animated_lm','rec_sh_compose']
order=['P1','P2','G1','G2','G3','L2','C1','C2','V1']
print('pose map | total avg/maxwin | work avg/max | wait | walk ms | considered/accepted | render_submit | render_record | rec_cull | rec_shadow_depth | lm_res+drain | sh_drain')
for m in ['base','wall','nopillars']:
  for p in order:
    L=[l for l in open(f'logs/{p}_{m}.log') if '[CpuTiming]' in l][3:]
    ds=[parse(l) for l in L]
    def avg(k):
        v=[d[k][0] for d in ds if k in d]; return st.mean(v) if v else float('nan')
    def mx(k):
        v=[d[k][1] for d in ds if k in d]; return max(v) if v else float('nan')
    def worstwin(k):
        v=[d[k][0] for d in ds if k in d]; return max(v) if v else float('nan')
    print(f"{p} {m} n={len(ds)} | tot {avg('total'):.2f} (max win {worstwin('total'):.2f}, spike {mx('total'):.0f}) | work {avg('work'):.2f}/{mx('work'):.1f} | wait {avg('wait'):.2f} | walk {avg('portal_walk'):.3f} | {avg('walk_considered'):.0f}/{avg('walk_accepted'):.0f} | submit {avg('render_submit'):.2f}/{mx('render_submit'):.1f} | rec {avg('render_record'):.2f} | cull {avg('rec_cull'):.2f} | shd {avg('rec_shadow_depth'):.3f} reach {avg('rec_shadow_reach'):.3f} | lm {avg('lightmap_residency'):.3f}+{avg('lightmap_drain'):.3f} | sh {avg('render_sh_drain'):.3f}")
