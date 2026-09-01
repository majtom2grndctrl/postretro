#!/usr/bin/env python3
"""Apply reviewer FIND/REPLACE findings to a spec, all-or-nothing, with
collision detection.

Design constraints, each earned by a real failure in an earlier round:

  * The reviewer writes the FIND/REPLACE block to disk itself. Nothing is
    retyped by the orchestrator, so transcription cannot introduce drift.
  * FIND must match exactly once. Zero matches and two matches are both
    hard failures, never a near-match fixup.
  * Two findings whose FIND spans overlap are a COLLISION. Independent
    lenses cannot see each other; an applier that takes both produces a
    self-contradictory spec. Collisions abort the run and are the
    orchestrator's to resolve.
  * The file is written only after every selected edit resolves. A loop
    that writes at the end and raises partway through silently drops the
    edits it already printed "ok" for.
"""

import argparse
import re
import sys
from pathlib import Path

# Two accepted renderings of the same pair. Independent lenses format
# identically only when the delimiter is shown literally, so accept both
# rather than discarding correct work over punctuation.
BLOCK = re.compile(
    r"<<<FIND\n(?P<find>.*?)\nFIND>>>\s*\n<<<REPLACE\n(?P<repl>.*?)\nREPLACE>>>",
    re.DOTALL,
)
BLOCK_FENCED = re.compile(
    r"FIND:?\s*\n+```[a-z]*\n(?P<find>.*?)\n```\s*\n+"
    r"(?:\*\*)?REPLACE(?:\*\*)?:?\s*\n+```[a-z]*\n(?P<repl>.*?)\n```",
    re.DOTALL,
)


def parse(path):
    """Yield {id, file, severity, bucket, find, replace} per finding block."""
    text = Path(path).read_text()
    out = []
    # Split on level-2 headings; the id is the heading text.
    chunks = re.split(r"^## (.+)$", text, flags=re.MULTILINE)
    for i in range(1, len(chunks), 2):
        fid, body = chunks[i].strip(), chunks[i + 1]
        fid = re.split(r"\s+[\u2014\-:]\s+", fid, maxsplit=1)[0].strip()
        if fid.upper().startswith(("SELF-AUDIT", "PIN TABLE", "SUMMARY")):
            continue
        m = BLOCK.search(body) or BLOCK_FENCED.search(body)
        if not m:
            out.append({"id": fid, "find": None, "replace": None,
                        "file": None, "severity": _field(body, "SEVERITY"),
                        "bucket": _field(body, "BUCKET"),
                        "location": _field(body, "LOCATION")})
            continue
        loc = _field(body, "LOCATION") or ""
        out.append({
            "id": fid,
            "file": "research.md" if "research.md" in loc else "index.md",
            "severity": _field(body, "SEVERITY"),
            "bucket": _field(body, "BUCKET"),
            "location": loc,
            "find": m.group("find"),
            "replace": m.group("repl"),
        })
    return out


def _field(body, name):
    m = re.search(rf"^{name}:\s*(.+)$", body, flags=re.MULTILINE)
    if not m:
        m = re.search(rf"^\|\s*{name}\s*\|\s*(.+?)\s*\|\s*$", body,
                      flags=re.MULTILINE)
    if not m:
        m = re.search(rf"^\s*(?:[-*]\s*)?\*\*{name}\*\*\s*[:\-]?\s*(.+)$", body,
                      flags=re.MULTILINE)
    return m.group(1).strip() if m else None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--findings", nargs="+", required=True)
    ap.add_argument("--spec-dir", required=True)
    ap.add_argument("--only", default="", help="comma-separated finding ids")
    ap.add_argument("--apply", action="store_true", help="write; default is dry run")
    args = ap.parse_args()

    selected = {s.strip() for s in args.only.split(",") if s.strip()}
    findings = []
    for f in args.findings:
        findings.extend(parse(f))
    if selected:
        findings = [f for f in findings if f["id"] in selected]
        missing = selected - {f["id"] for f in findings}
        if missing:
            print(f"FATAL: selected ids not found in any findings file: {sorted(missing)}")
            return 2

    specs = {}
    for name in ("index.md", "research.md"):
        p = Path(args.spec_dir) / name
        if p.exists():
            specs[name] = p.read_text()

    problems, spans = [], {}
    for f in findings:
        if f["find"] is None:
            problems.append(f"{f['id']}: no FIND/REPLACE block parsed")
            continue
        text = specs.get(f["file"])
        if text is None:
            problems.append(f"{f['id']}: unknown target file {f['file']}")
            continue
        n = text.count(f["find"])
        if n != 1:
            problems.append(f"{f['id']}: FIND matches {n} times in {f['file']} (need exactly 1)")
            continue
        start = text.index(f["find"])
        spans.setdefault(f["file"], []).append((start, start + len(f["find"]), f["id"]))

    # Collision detection across lenses.
    for fname, sp in spans.items():
        sp.sort()
        for (a0, a1, aid), (b0, b1, bid) in zip(sp, sp[1:]):
            if b0 < a1:
                problems.append(
                    f"COLLISION in {fname}: {aid} and {bid} edit overlapping text — "
                    f"resolve before applying either")

    # Report dropped content: any run of 4+ consecutive words present in FIND
    # and absent from REPLACE. Not an error — intentional deletions are real —
    # but it must be seen, not inferred.
    for f in findings:
        if not f["find"]:
            continue
        fw, rep = f["find"].split(), " ".join(f["replace"].split())
        dropped, run = [], []
        for w in fw:
            run.append(w)
            if " ".join(run) not in rep:
                if len(run) > 4:
                    dropped.append(" ".join(run[:-1]))
                run = [run[-1]] if run[-1] in rep else []
        if len(run) > 4:
            dropped.append(" ".join(run))
        for d in dropped:
            print(f"  DROPS  {f['id']}: {d[:90]!r}")

    for f in findings:
        state = "resolves" if f["find"] is not None and not any(
            f["id"] in p for p in problems) else "BLOCKED"
        print(f"  {f['id']:<6} {str(f['severity']):<12} {str(f['bucket']):<14} "
              f"{f['file']} {state}")

    if problems:
        print("\nNOT APPLIED. Problems:")
        for p in problems:
            print(f"  - {p}")
        return 1

    if not args.apply:
        print(f"\nDry run OK: {len(findings)} findings resolve cleanly, no collisions.")
        return 0

    # Apply in reverse offset order per file so earlier offsets stay valid.
    for fname, sp in spans.items():
        text = specs[fname]
        by_id = {f["id"]: f for f in findings}
        for start, end, fid in sorted(sp, reverse=True):
            text = text[:start] + by_id[fid]["replace"] + text[end:]
        specs[fname] = text

    # Post-verify BEFORE writing anything.
    failures = []
    for f in findings:
        text = specs[f["file"]]
        if f["replace"] not in text:
            failures.append(f"{f['id']}: REPLACE text absent after apply")
        if f["find"] and f["find"] in text and f["find"] not in f["replace"]:
            failures.append(f"{f['id']}: FIND text still present after apply")
    if failures:
        print("\nPOST-VERIFY FAILED, nothing written:")
        for x in failures:
            print(f"  - {x}")
        return 1

    for fname, text in specs.items():
        (Path(args.spec_dir) / fname).write_text(text)
    print(f"\nApplied {len(findings)} findings; post-verify clean; files written.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
