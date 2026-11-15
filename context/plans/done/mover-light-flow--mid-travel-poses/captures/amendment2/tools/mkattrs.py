# Attribution AABBs (engine metres) for the door-ceiling band split, per t.
# Classes: wenc = the x 846.93 column inside the room's solid (z from the floor
# slab bottom to the roof top); wabove = that column above the roof top;
# xabove = probes over the room's interior x (850..1160) above the roof top;
# none = an empty box (keeps no probe: checks the numerator path).
import json, sys
U = 0.0254
def box(x0, x1, y0, y1, z0, z1, mode=1):
    return f'attr={mode};min={-y1*U:.5f},{z0*U:.5f},{-x1*U:.5f};max={-y0*U:.5f},{z1*U:.5f},{-x0*U:.5f}'
ts = [round(0.10 + 0.01 * i, 5) for i in range(86)] + [0.0, 1.0, 0.5]
out = {'wenc': {}, 'wabove': {}, 'xabove': {}, 'none': {'*': box(0, 0, 0, 0, 1e5, 1e5)}}
for t in ts:
    top = 176.0 * t
    k = f'{t:.5f}'
    out['wenc'][k] = box(840, 850, 150, 490, -192 + top, top)
    out['wabove'][k] = box(840, 850, 150, 490, top + 1e-3, 5000)
    out['xabove'][k] = box(850, 1160, 150, 490, top + 1e-3, 5000)
json.dump(out, open(sys.argv[1], 'w', encoding='utf-8'), indent=0)
