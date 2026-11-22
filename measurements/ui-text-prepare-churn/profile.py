"""Shares of render_submit in a macOS `sample` call graph (M2).

usage: python3 profile.py <label>   -> reads runs/<label>.sample.txt, writes runs/<label>.profile.json

render_submit (= Renderer::submit_windowed_frame, inlined in release) is taken as
the samples of `CommandEncoder::finish` and `UploadQueue::submit` frames whose
immediate parent is `render_frame_indirect`: the two calls that body makes
(`queue.submit(once(encoder.finish()))`). Counts are inclusive samples of the
outermost matching frame inside those subtrees (nested repeats counted once). `glyphon_qwb` = samples of
queue_write_buffer frames that have a glyphon frame among their ancestors.
"""
import json, re, sys
from pathlib import Path

RUNS = Path(__file__).resolve().parent / "runs"
label = sys.argv[1]
text = (RUNS / f"{label}.sample.txt").read_text(errors="replace")
tree = text.split("Total number in stack")[0]
LINE = re.compile(r"^([\s+!:|]*?)(\d+) (.*)$")

nodes = []  # (depth, count, name)
for line in tree.splitlines():
    m = LINE.match(line)
    if not m or "(in " not in m[3] and "???" not in m[3]:
        continue
    nodes.append((len(m[1]), int(m[2]), m[3]))

PATS = {
    "render_submit": re.compile(r"CommandEncoderInterface6finish|UploadQueue6submit"),
    "triage_submissions": re.compile(r"triage_submissions"),
    "queue_write_buffer": re.compile(r"queue_write_buffer"),
    "glyphon": re.compile(r"glyphon"),
}


def submit_root(name, stack):
    return bool(PATS["render_submit"].search(name)) and bool(stack) and "render_frame_indirect" in stack[-1][1]


def outermost(pat, within):
    """Sum counts of outermost nodes matching pat whose ancestor stack satisfies within()."""
    total, stack = 0, []  # stack of (depth, name)
    matched_depth = None
    for d, c, name in nodes:
        while stack and stack[-1][0] >= d:
            stack.pop()
        if matched_depth is not None and d <= matched_depth:
            matched_depth = None
        if matched_depth is None and pat.search(name) and within(stack) and (pat is not PATS["render_submit"] or submit_root(name, stack)):
            total += c
            matched_depth = d
        stack.append((d, name))
    return total


def under(pat):
    return lambda st: any(pat.search(n) for _, n in st)


def under_submit(st):
    return any(PATS["render_submit"].search(st[i][1]) and i > 0 and "render_frame_indirect" in st[i - 1][1] for i in range(len(st)))


main = re.search(r"^\s+(\d+) Thread_\d+(?:: main|\s+DispatchQueue_\d+: com.apple.main-thread)", tree, re.M)
rs_name = None
# find the exact render_submit stage function name used in this build
for d, c, n in nodes:
    if "render_frame_indirect" in n:
        rs_name = n
        break
rs = PATS["render_submit"]
res = {
    "label": label,
    "main_thread_samples": int(main[1]) if main else None,
    "render_submit_frame": rs_name,
    "render_submit": outermost(rs, lambda st: True),
    "triage_submissions": outermost(PATS["triage_submissions"], under_submit),
    "queue_write_buffer": outermost(PATS["queue_write_buffer"], under_submit),
    "queue_write_buffer_anywhere": outermost(PATS["queue_write_buffer"], lambda st: True),
    "glyphon_qwb_anywhere": outermost(PATS["queue_write_buffer"], under(PATS["glyphon"])),
    "glyphon_anywhere": outermost(PATS["glyphon"], lambda st: True),
}
if res["render_submit"]:
    for k in ["triage_submissions", "queue_write_buffer"]:
        res[k + "_share"] = round(res[k] / res["render_submit"], 4)
(RUNS / f"{label}.profile.json").write_text(json.dumps(res, indent=2) + "\n")
print(json.dumps(res))
