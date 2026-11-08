"""Watch how near the stress map's enemies come to lift 0 over a run.

usage: lift_watch.py <port> <seconds> <out.json>
Connects to a running engine's --observe-live channel and samples every 10 s.
"""
import json
import math
import socket
import struct
import sys
import time

port, seconds, out = int(sys.argv[1]), float(sys.argv[2]), sys.argv[3]


def frame(sock):
    prefix = b""
    while len(prefix) < 4:
        chunk = sock.recv(4 - len(prefix))
        if not chunk:
            raise ConnectionError("closed")
        prefix += chunk
    size = struct.unpack("<I", prefix)[0]
    body = b""
    while len(body) < size:
        chunk = sock.recv(size - len(body))
        if not chunk:
            raise ConnectionError("closed")
        body += chunk
    return json.loads(body)


def ask(sock, spec):
    body = json.dumps({"verb": "dump", "spec": spec}).encode()
    sock.sendall(struct.pack("<I", len(body)) + body)
    reply = frame(sock)
    if reply.get("status") != "ok":
        raise RuntimeError(reply)
    return reply["dump"]["entities"]


started = time.time()
sock = None
while sock is None:
    try:
        sock = socket.create_connection(("127.0.0.1", port), timeout=5)
    except OSError:
        if time.time() - started > 120:
            raise
        time.sleep(1)
sock.settimeout(30)
print("hello", frame(sock), flush=True)

samples = []
while time.time() - started < seconds:
    time.sleep(10)
    try:
        agents = {e["entity"] for e in ask(sock, {"component": "agent", "cap": 100000})}
        lift = [e for e in ask(sock, {"component": "transform", "tag": "warren_lift_0", "cap": 10})]
        moves = ask(sock, {"component": "transform", "cap": 100000})
    except (RuntimeError, OSError, ConnectionError) as error:
        print("ask failed", error, flush=True)
        try:
            sock.close()
            sock = socket.create_connection(("127.0.0.1", port), timeout=5)
            sock.settimeout(30)
            frame(sock)
        except OSError as again:
            print("reconnect failed", again, flush=True)
        continue
    if not lift:
        print("no lift yet", flush=True)
        continue
    lx, ly, lz = lift[0]["component"]["position"]
    near = []
    for e in moves:
        if e["entity"] in agents:
            x, y, z = e["component"]["position"]
            d = math.hypot(x - lx, z - lz)
            if d < 12.0:
                near.append({"entity": e["entity"], "xz_m": round(d, 2), "dy_m": round(y - ly, 2)})
    sample = {"t": round(time.time() - started), "lift": [lx, ly, lz], "agents": len(agents), "near": near}
    samples.append(sample)
    print(json.dumps(sample), flush=True)
    with open(out, "w", encoding="utf-8") as handle:
        json.dump(samples, handle, indent=1)
