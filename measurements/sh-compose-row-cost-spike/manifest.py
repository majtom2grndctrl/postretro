"""Write batches.json: per batch, the probe commit and binary SHA-256 every run used, and fixture prefixes.

usage: manifest.py > batches.json
The probe commit is the binary directory's `probe-<commit>` name. A batch is
honest only if every run names one binary.
"""
import collections, hashlib, json, re
from pathlib import Path

here = Path(__file__).resolve().parent
root = here.parents[1]


def sha(path, n=None):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()[: n or 64]


binaries = collections.defaultdict(set)
maps = collections.defaultdict(set)
for path in sorted((here / "runs").glob("*.run.json")):
    record = json.loads(path.read_text())
    batch = re.sub(r"-r\d+$", "", path.name[: -len(".run.json")])
    batch = batch.split("-")[0]
    binaries[batch].add(record["binary"])
    maps[batch].add(record["map"])
fixtures = {m: sha(root / "content/dev/maps" / m, 16) for ms in maps.values() for m in ms}
out = {}
for batch in sorted(binaries):
    bins = sorted(binaries[batch])
    out[batch] = {
        "binaries": [{"probe_commit": re.search(r"probe-([0-9a-f]+)", b)[1],
                      "sha256": sha(b) if Path(b).exists() else "binary deleted"} for b in bins],
        "one_binary": len(bins) == 1,
        "fixtures": {m: fixtures[m] for m in sorted(maps[batch])},
    }
print(json.dumps(out, indent=2))
