import json, os, sys
import matplotlib; matplotlib.use('Agg')
import matplotlib.pyplot as plt
M = '/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/991d9ce0-b6e7-4e9e-8a81-c2d9f7c07b7d/scratchpad/motion'
D = json.load(open(os.path.join(M, 'results/dense.json')))
K = json.load(open(os.path.join(M, 'results/knots.json')))
ST = dict(head=('#d1495b', '-', 'head (6 / 8 poses)'), nodepth=('#2e86ab', '-', 'head, depth stripped'),
          dense=('#7a3db8', '--', 'dense (11 / 15 poses)'), densenodepth=('#1b9e77', '--', 'dense, depth stripped'),
          two=('#3b8b3b', ':', 'light_poses 2'), strip=('#888888', ':', 'id 57 stripped'))
for v, R in D.items():
    fig, ax = plt.subplots(figsize=(12, 5.5))
    ts = [r['t'] for r in R['rows']]
    for c, (col, ls, name) in ST.items():
        ax.plot(ts, [r['mean'][c] for r in R['rows']], ls, color=col, lw=1.4, marker='o', ms=2, label=name)
    for x in K[v]:
        ax.axvline(x, color='#444', lw=0.8, alpha=0.5)
    dk = [k / 10 for k in range(11)] if v in ('floor', 'ceiling', 'interior') else [k / 14 for k in range(15)]
    for x in dk:
        if all(abs(x - y) > 1e-4 for y in K[v]):
            ax.axvline(x, color='#7a3db8', lw=0.6, ls='--', alpha=0.5)
    ax.set_title(f"{v}: crop mean luma; grey lines = head knots, purple dashed = dense-only knots")
    ax.set_xlabel("t'"); ax.set_ylabel('crop mean luma'); ax.legend(fontsize=8)
    fig.tight_layout()
    p = os.path.join(M, f'charts/{v}-dense.png'); fig.savefig(p, dpi=110); plt.close(fig); print(p)
