"""Stream rows out of an xctrace table export, resolving id/ref dedup.

Usage: python3 -I xtrows.py <export.xml> [substring-filter ...]
Prints TSV: one row per line, columns by mnemonic, value = fmt (or text).
"""
import sys
import xml.etree.ElementTree as ET


def rows(path):
    cache = {}
    cols = []
    for ev, el in ET.iterparse(path, events=("end",)):
        if el.tag == "mnemonic":
            cols.append(el.text)
        elif el.tag == "row":
            vals = []
            for c in el:
                vals.append(resolve(c, cache))
            yield cols, vals
            el.clear()


def resolve(c, cache):
    if "ref" in c.attrib:
        return cache.get(c.attrib["ref"], "")
    v = c.attrib.get("fmt")
    if v is None:
        v = (c.text or "").strip()
        if not v and len(c):
            v = " | ".join(resolve(k, cache) for k in c)
    # cache sub-elements too (refs can point at nested ids)
    for k in c.iter():
        if "id" in k.attrib and k.attrib["id"] not in cache:
            kv = k.attrib.get("fmt")
            if kv is None:
                kv = (k.text or "").strip()
            cache[k.attrib["id"]] = kv
    if "id" in c.attrib:
        cache[c.attrib["id"]] = v
    return v


if __name__ == "__main__":
    path = sys.argv[1]
    filt = sys.argv[2:]
    header = False
    for cols, vals in rows(path):
        if not header:
            print("\t".join(cols))
            header = True
        line = "\t".join(vals)
        if not filt or any(f in line for f in filt):
            print(line)
