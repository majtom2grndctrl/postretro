# Task 5b capture driver: task 5's views and metrics (t5-harness/cap.py) over
# the experiment PRLs, beside task 5's own captures.
#
#   python3 -I cap5b.py capture <column> [view ...]   # missing captures only
#   python3 -I cap5b.py metrics <column> [view ...]   # -> t5b-results.json
#
# Columns: task 5's (today, tb, lerp, strip-today, strip-new) read task 5's
# PRLs and captures; an experiment column `X` reads PRL prefix `X` baked by
# `bake.sh X` with the renderer's default (through-base) depth rule.
import json, os, sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(os.path.dirname(HERE), 't5-harness'))
import cap  # noqa: E402

SP = '/private/tmp/claude-501/-Users-dhiester-Projects-Personal-postretro/b8b805c8-5289-4dc3-bb1d-ebd668f82d89/scratchpad'
cap.MAPS = os.path.join(SP, 't5/ws/content/dev/maps')
cap.CAP = os.path.join(SP, 't5/cap')
cap.BIN = os.path.join(SP, 't5/bin')
EXPERIMENTS = ['blend', 'dark', 'unm', 'ctl']
for name in EXPERIMENTS:
    cap.EXTRA[name] = ('postretro-head', name, None)
    cap.EXTRA[name + '-nodepth'] = ('postretro-head', name + '-nodepth', None)
# Finer interior series around the knots, task 5's 0.05 grid included.
VIEW_TS = dict(cap.VIEWS)
VIEW_TS['interior'] = (cap.interior_view, [round(0.025 * i, 3) for i in range(41)])
VIEW_TS['east'] = (cap.east_view, [0.00, 0.05, 0.10, 0.143, 0.15, 0.20, 0.25, 0.70, 0.75, 0.80, 0.85])


def metrics(column, views):
    out_path = os.path.join(HERE, 't5b-results.json')
    results = json.load(open(out_path)) if os.path.exists(out_path) else {}
    for view in views:
        make, ts = VIEW_TS[view]
        rows = []
        for t in ts:
            spec = make(t)
            png = os.path.join(cap.CAP, cap.label(view, t, column) + '.png')
            if not os.path.exists(png):
                continue
            row = {'t': t, 'frame_mean': cap.frame_mean(cap.read_luma(png))}
            strip_png = os.path.join(cap.CAP, cap.label(view, t, 'strip-new') + '.png')
            if spec['plane'] is not None and os.path.exists(strip_png):
                blocks = cap.crop_blocks(spec)
                mine = cap.block_means(cap.read_luma(png), blocks)
                strip = cap.block_means(cap.read_luma(strip_png), blocks)
                diff = {b: mine[b] - strip[b] for b in blocks}
                e = cap.excess_metric(diff)
                e.pop('excess')
                e['mean'] = sum(diff.values()) / len(diff)
                row['12d'] = e
                own = cap.excess_metric(mine)
                own.pop('excess')
                row['12c'] = own
                row['crop_mean'] = sum(mine.values()) / len(mine)
            rows.append(row)
            print(column, view, t, json.dumps({k: v for k, v in row.items() if k != 't'})[:240], flush=True)
        results.setdefault(column, {})[view] = rows
    json.dump(results, open(out_path, 'w'), indent=1)


if __name__ == '__main__':
    os.makedirs(cap.CAP, exist_ok=True)
    mode, column, views = sys.argv[1], sys.argv[2], sys.argv[3:] or list(VIEW_TS)
    if mode == 'capture':
        for view in views:
            for t in VIEW_TS[view][1]:
                cap.capture(view, t, column)
            print('captured', column, view, flush=True)
    elif mode == 'metrics':
        metrics(column, views)
