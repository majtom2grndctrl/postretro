#!/usr/bin/env python3
"""Collect Build Summary rows from run-bake outputs into JSON.
usage: summarize.py <runs-dir> <label>... > out.json"""
import json, re, sys, os
d = sys.argv[1]
res = {}
for label in sys.argv[2:]:
    r = {}
    try:
        for l in open(os.path.join(d, label + ".summary.txt")):
            m = re.match(r"^  (\S[^\d]*?)\s+([\d.]+)s\s*$", l)
            if m:
                r[m.group(1).strip()] = float(m.group(2))
        t = open(os.path.join(d, label + ".time.txt")).read()
        m = re.search(r"wall_s=(\d+)", t); r["wall_s"] = int(m.group(1)) if m else None
        m = re.search(r"(\d+)\s+maximum resident", open(os.path.join(d, label + ".err.log"), errors="replace").read())
        r["max_rss_bytes"] = int(m.group(1)) if m else None
    except FileNotFoundError as e:
        r["error"] = str(e)
    res[label] = r
print(json.dumps(res, indent=1))
