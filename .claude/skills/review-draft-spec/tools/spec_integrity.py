#!/usr/bin/env python3
"""Structural integrity checks for a PostRetro plan spec.

Checks the Pinned-behaviors row table, the Acceptance-criteria list, and
the cross-references among Tasks, Acceptance criteria and Invariants for
internal consistency: a pin row no task executes, a row id reused, a row
gone while prose still names it, a line-number citation that will go stale.

Specs vary in which conventions they adopt — a `Pinned behaviors` table and
its `Kind` column, bold `**ACn**` ids — so each check runs only when its
section is present, and the tool declares which checks ran and which it
skipped for want of a section. A section that IS present but yields zero
items is a parse mismatch, not a clean spec, and fails loudly rather than
passing over an empty list.

Usage: spec_integrity.py <path-to-spec-index.md>

Exit 1 on any failure (including a zero-item parse of a present section).
Run before every commit.
"""

import re
import sys
from pathlib import Path

if len(sys.argv) != 2:
    print("usage: spec_integrity.py <path-to-spec-index.md>", file=sys.stderr)
    sys.exit(2)

SPEC = Path(sys.argv[1])
if not SPEC.is_file():
    print(f"spec_integrity: no such file: {SPEC}", file=sys.stderr)
    sys.exit(2)

text = SPEC.read_text()
fail = []
ran = []
skipped = []


def section(name):
    """Body of the `## <name>` section, or None if no such heading exists.

    Matches the heading case-insensitively and takes everything up to the
    next `## ` heading (or end of file) — never a named successor. A spec
    whose sections are reordered, or which lacks a specific successor
    heading, still yields the right extent instead of an empty string.
    """
    m = re.search(rf"^## {re.escape(name)}\s*$(.*?)(?=^## |\Z)",
                  text, re.S | re.M | re.I)
    return m.group(1) if m else None


tasks = section("Tasks")
pins = section("Pinned behaviors")
acs = section("Acceptance criteria")
invariants = section("Invariants")

# --- Pin rows (convention: a "Pinned behaviors" table with a Kind column) -
ids, kind = [], {}
if pins is None:
    skipped.append("pin rows (no Pinned behaviors section — convention not adopted)")
else:
    rows = re.findall(r"^\| (P\d+\w*) \| (.+?) \|.*\| (unit|manual)[^|]*\|$",
                      pins, re.M)
    if not rows:
        fail.append("Pinned behaviors section exists but 0 pin rows parsed "
                     "— table format changed or this convention was dropped")
    else:
        ids = [r[0] for r in rows]
        kind = {r[0]: r[2] for r in rows}
        ran.append(f"pin rows ({len(ids)} rows: duplicates, order)")

        dupes = {i for i in ids if ids.count(i) > 1}
        if dupes:
            fail.append(f"duplicate pin row ids: {sorted(dupes)}")

        def sort_key(i):
            m = re.match(r"P(\d+)(\w*)", i)
            return (int(m.group(1)), m.group(2))

        if ids != sorted(ids, key=sort_key):
            out_of_order = [(a, b) for a, b in zip(ids, ids[1:])
                            if sort_key(a) > sort_key(b)]
            fail.append(f"pin rows out of order at: {out_of_order}")

        # --- Coverage: every row is executed by some task -----------------
        if tasks is None:
            skipped.append("pin-row task coverage (no Tasks section)")
        else:
            ran.append("pin-row task coverage")
            for i in ids:
                if not re.search(rf"\b{i}\b", tasks):
                    fail.append(f"{i} ({kind[i]}) is named by no task — delivered by nothing")

        # --- Manual-row group membership vs the surrounding prose ---------
        # A task that executes rows by naming them in group bullets — "**Plain
        # run** (P5, P6, ...)" — executes nothing it does not name. A row
        # discussed in that task's prose but absent from every bullet is
        # delivered by nothing. Only the specs that adopted grouped bullets
        # have anything for this to check.
        if tasks is not None:
            saw_bullets = False
            for tm in re.finditer(r"^### (Task \d+):.*?(?=^### |\Z)", tasks, re.S | re.M):
                body, tname = tm.group(0), tm.group(1)
                bullets = re.findall(r"^- \*\*[^*]+\*\*\s*\(((?:P\d+\w*(?:,\s*)?)+)\)", body, re.M)
                if not bullets:
                    continue
                saw_bullets = True
                grouped = set()
                for lst in bullets:
                    grouped |= set(re.findall(r"P\d+\w*", lst))
                for r in sorted(set(re.findall(r"\bP\d+\w*\b", body)) - grouped, key=sort_key):
                    fail.append(f"{tname} prose discusses {r} but no group bullet lists it")
                for r in sorted(grouped, key=sort_key):
                    if kind.get(r) != "manual":
                        fail.append(f"{tname} claims {r}, whose Kind column says {kind.get(r)}")
            if saw_bullets:
                ran.append("manual-row group-bullet membership")
            else:
                skipped.append("manual-row group-bullet membership "
                                "(no task uses grouped-bullet rows)")

# --- Dangling references: prose naming a pin row that does not exist ------
# Runs whether or not a Pinned behaviors table exists — a P-id mentioned
# with no table at all is exactly as dangling as one missing its own row.
ref_source = (tasks or "") + (acs or "") + (invariants or "")
referenced = set(re.findall(r"\bP\d+\w*\b", ref_source))
if referenced or ids:
    ran.append("dangling pin-id references")
    for r in sorted(referenced - set(ids), key=lambda i: (int(re.match(r"P(\d+)", i).group(1)), i)):
        fail.append(f"{r} is referenced in prose but has no row in Pinned behaviors")

if invariants is None:
    skipped.append("invariants as a dangling-reference source (no Invariants section)")

# --- Acceptance criteria ---------------------------------------------------
# Items render as `- [ ]`/`- [x]` checkboxes in most specs and as a plain
# numbered list in a few older ones; either counts as an item. A bold id
# renders tight (`**AC1**`) or with a trailing title (`**AC1 — Title.**`) —
# both are "the AC1 token opens a bold run", so match on the id plus a word
# boundary rather than requiring the bold run to close immediately after it.
if acs is None:
    fail.append("required section 'Acceptance criteria' not found")
    ac_item_count = 0
else:
    ac_items = re.findall(r"^(?:-\s*\[[ xX]\]|\d+\.)\s+\S", acs, re.M)
    ac_item_count = len(ac_items)
    if ac_item_count == 0:
        fail.append("Acceptance criteria section exists but 0 items parsed "
                     "(checked '- [ ]'/'- [x]' checkboxes and a numbered list)")
    else:
        ran.append(f"acceptance-criteria item count ({ac_item_count} items)")

    ac_ids = re.findall(r"\*\*(AC\d+)\b", acs)
    if not ac_ids:
        skipped.append("AC-id duplicate/contiguity check (no **ACn** convention in this spec)")
    else:
        ran.append(f"AC-id duplicate/contiguity ({len(ac_ids)} ids)")
        if len(ac_ids) != len(set(ac_ids)):
            fail.append("duplicate AC ids")
        if ac_ids != [f"AC{n}" for n in range(1, len(ac_ids) + 1)]:
            fail.append(f"AC ids are not contiguous from AC1: {ac_ids}")

# --- Stale-by-construction citations --------------------------------------
ran.append("stale line-number citations")
for m in re.finditer(r"[\w/.-]+\.(?:rs|ts|toml|luau|md|wgsl):\d+", text):
    fail.append(f"line-number citation will go stale: {m.group(0)}")

# --- Pivot language (habit 6) ---------------------------------------------
ran.append("pivot language")
for pat in (r"\ban earlier draft\b", r"\bwas thought\b", r"\bno longer matters\b",
            r"\boriginally,", r"\bpreviously we\b", r"\bfewer than assumed\b"):
    for m in re.finditer(pat, text, re.I):
        fail.append(f"pivot language: {m.group(0)!r}")

# --- Unconverted review-artifact phrasing (habit 9) -----------------------
# Cut before "Open questions" case-insensitively — some specs spell it
# "Resolved questions" once resolved, which correctly disables the cut and
# leaves the whole document in scope for this check.
oq = re.search(r"^## Open questions\s*$", text, re.M | re.I)
body_wo_oq = text[:oq.start()] if oq else text
ran.append("unconverted review phrasing" + ("" if oq else " (no Open questions section to exclude)"))
for pat in (r"\bpin whether\b", r"\bTBD\b", r"\bdecide later\b", r"\bconsider adding\b"):
    for m in re.finditer(pat, body_wo_oq, re.I):
        fail.append(f"unconverted review phrasing outside Open questions: {m.group(0)!r}")

# --- Restated facts ------------------------------------------------------
# A fact stated in two places is a defect waiting for a fix to land in one of
# them. Not always avoidable — both sides of an interface need its signature,
# and summary sections restate by design — so this reports rather than fails.
# Watch the count: it should fall, never rise.
ran.append("restated facts (diagnostic, non-failing)")
_units = []
for _i, _line in enumerate(text.split("\n"), 1):
    if _line.startswith("|"):
        continue
    for _s in re.split(r"(?<=[.;])\s+", _line):
        if len(_s.split()) >= 8:
            _units.append((_i, _s.strip()))
_grams = {}
for _i, _s in _units:
    _w = [w for w in re.sub(r"[^a-z0-9 ]", " ", _s.lower()).split() if len(w) > 2]
    for _j in range(len(_w) - 5):
        _grams.setdefault(" ".join(_w[_j:_j + 6]), set()).add(_i)
_pairs = {}
for _g, _ls in _grams.items():
    if len(_ls) > 1:
        _pairs.setdefault(tuple(sorted(_ls)), set()).add(_g)
_strong = {k: v for k, v in _pairs.items() if len(v) >= 3}
if _pairs:
    print(f"restated facts: {len(_pairs)} line groups ({len(_strong)} sharing 3+ phrases)")
    for _k, _v in sorted(_strong.items(), key=lambda kv: -len(kv[1])):
        print(f"  lines {_k}: {sorted(_v, key=len)[-1]!r}")

# --- Coverage declaration --------------------------------------------------
# So a reader can never mistake "not checked" for "checked and clean".
print("coverage:")
print(f"  ran: {'; '.join(ran) if ran else '(nothing)'}")
print(f"  skipped: {'; '.join(skipped) if skipped else '(nothing)'}")

task_count = len(re.findall(r'^### Task ', tasks, re.M)) if tasks else 0
print(f"{SPEC}: {len(ids)} pin rows, {ac_item_count} acceptance criteria, "
      f"{task_count} tasks")
if fail:
    print(f"\nFAIL ({len(fail)}):")
    for f in fail:
        print(f"  - {f}")
    sys.exit(1)
print("integrity: clean")
