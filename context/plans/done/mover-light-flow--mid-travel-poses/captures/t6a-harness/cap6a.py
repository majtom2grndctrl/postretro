# Task 6a capture driver: task 5's views and metrics (t5-harness/cap.py) with
# task 5b's finer series (t5b-harness/cap5b.py) over this build's PRLs, column
# `t6a` (prefix `t6a-`, binary `postretro-t6a`), beside the earlier columns.
#
#   python3 -I cap6a.py capture [view ...]   # missing captures only
#   python3 -I cap6a.py metrics [view ...]   # -> t6a-results.json
import json, os, sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(os.path.dirname(HERE), 't5-harness'))
sys.path.insert(0, os.path.join(os.path.dirname(HERE), 't5b-harness'))
import cap  # noqa: E402
import cap5b  # noqa: E402  (points cap at task 5's scratch dirs)

COLUMN = 't6a'
cap.EXTRA[COLUMN] = ('postretro-t6a', COLUMN, None)
VIEW_TS = cap5b.VIEW_TS
VIEW_TS['west'] = (cap.west_view, [0.20, 0.25, 0.28, 0.30, 0.32])


def metrics(views):
    out_path = os.path.join(HERE, 't6a-results.json')
    results = json.load(open(out_path)) if os.path.exists(out_path) else {}
    for view in views:
        make, ts = VIEW_TS[view]
        rows = []
        for t in ts:
            spec = make(t)
            png = os.path.join(cap.CAP, cap.label(view, t, COLUMN) + '.png')
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
            print(COLUMN, view, t, json.dumps({k: v for k, v in row.items() if k != 't'})[:240],
                  flush=True)
        results[view] = rows
    json.dump(results, open(out_path, 'w'), indent=1)


if __name__ == '__main__':
    os.makedirs(cap.CAP, exist_ok=True)
    mode, views = sys.argv[1], sys.argv[2:] or ['interior', 'east', 'west']
    if mode == 'capture':
        for view in views:
            for t in VIEW_TS[view][1]:
                cap.capture(view, t, COLUMN)
            print('captured', COLUMN, view, flush=True)
    elif mode == 'metrics':
        metrics(views)
