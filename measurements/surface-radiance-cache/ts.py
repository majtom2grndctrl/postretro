#!/usr/bin/env python3
"""Prefix each stderr line (split on \\n or \\r) with seconds since start."""
import re, sys, time
t0 = time.time()
buf = b""
out = sys.stdout.buffer
while True:
    chunk = sys.stdin.buffer.read1(4096)
    if not chunk:
        break
    buf += chunk
    parts = re.split(rb"[\r\n]", buf)
    buf = parts.pop()
    for p in parts:
        if p.strip():
            out.write(b"%8.2f %s\n" % (time.time() - t0, p))
    out.flush()
if buf.strip():
    out.write(b"%8.2f %s\n" % (time.time() - t0, buf))
