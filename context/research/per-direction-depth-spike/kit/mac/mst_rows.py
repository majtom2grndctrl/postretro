#!/usr/bin/env python3
"""Parse an xctrace metal-gpu-intervals export into rows (resolving id/ref)."""
import sys, xml.etree.ElementTree as ET

def rows(path):
    ids = {}
    tree = ET.parse(path)
    root = tree.getroot()
    out = []
    for row in root.iter('row'):
        vals = []
        for el in row:
            if el.tag == 'sentinel':
                vals.append(None); continue
            r = el.get('ref')
            if r is not None:
                el = ids[r]
            else:
                # register this element and nested ids
                for sub in el.iter():
                    if sub.get('id') is not None:
                        ids[sub.get('id')] = sub
            vals.append(el)
        # columns: start, duration, channel, frame, latency, depth, label, state, conn, color, process, gpu, subtitle, iosurf, bytes, cmdbuf, encoder, submission
        def txt(e): return None if e is None else e.text
        lab = vals[6]
        label = None; pid = None; enc = None
        if lab is not None:
            s = lab.find('string')
            if s is not None:
                s = ids.get(s.get('ref'), s) if s.get('ref') else s
                label = s.text
            p = lab.find('process')
            if p is not None:
                p = ids.get(p.get('ref'), p) if p.get('ref') else p
                pid = p.get('fmt')
            e = lab.find('metal-encoder-id')
            if e is not None:
                e = ids.get(e.get('ref'), e) if e.get('ref') else e
                enc = e.text
        proc = vals[10]
        out.append(dict(start=int(txt(vals[0])), dur=int(txt(vals[1])), chan=txt(vals[2]),
                        frame=txt(vals[3]), depth=txt(vals[5]), label=label,
                        proc=None if proc is None else proc.get('fmt'), enc=enc,
                        cmdbuf=txt(vals[15]), encoder=txt(vals[16])))
    return out

if __name__ == '__main__':
    rs = rows(sys.argv[1])
    for r in rs:
        print(r)
