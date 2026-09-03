#!/usr/bin/env python3
"""Behavioral tests for spec_integrity.py — stdlib only, run as `python3 <path>`.

The tool runs at import time and calls sys.exit, so every case is driven
through a subprocess against a temp index.md. A known-clean spec is mutated
per test to trigger exactly one check; each mutation pins both the accepted
and the refused side of a rule.
"""

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

TOOL = Path(__file__).parents[1] / "spec_integrity.py"

CLEAN = """# demo-spec

## Tasks

### Task 1: Do the thing

Implements P1 and P2 by wiring the helper into the drain path directly.

### Task 2: Verify the payload

Executes P3 against a real payload and reverts every fixture afterwards.

## Pinned behaviors

| # | Scenario | Ordering | Expected outcome | Kind |
|---|---|---|---|---|
| P1 | First scenario runs | setup then act on it | the outcome holds as written | unit |
| P2 | Second scenario runs | act then read the value | the value is observed once | unit |
| P3 | Third scenario crashes | crash occurs mid run here | the marker is present after | manual |

## Acceptance criteria

- [ ] **AC1** The engine builds and the demo runs to the menu without any error today.
- [ ] **AC2** Each level starts and reaches gameplay on a clean checkout of the whole tree.

## Invariants

The queue drains in arrival order and P1 governs the drain contract for this path.

## Open questions

None outstanding at the present time.
"""


def run(spec_text, args=None):
    """Run the tool on spec_text; return (rc, combined_output)."""
    d = tempfile.mkdtemp()
    p = Path(d, "index.md")
    p.write_text(spec_text)
    cmd = [sys.executable, str(TOOL)] + ([str(p)] if args is None else args)
    r = subprocess.run(cmd, capture_output=True, text=True)
    return r.returncode, r.stdout + r.stderr


def sub(old, new, text=CLEAN):
    assert old in text, f"fixture missing: {old!r}"
    return text.replace(old, new)


# --------------------------------------------------------------------------- #
# Baseline + usage                                                            #
# --------------------------------------------------------------------------- #
class BaselineTests(unittest.TestCase):
    def test_clean_spec_passes(self):
        rc, out = run(CLEAN)
        self.assertEqual(rc, 0, out)
        self.assertIn("integrity: clean", out)

    def test_coverage_declaration_printed(self):
        rc, out = run(CLEAN)
        self.assertIn("coverage:", out)
        self.assertIn("ran:", out)
        self.assertIn("skipped:", out)

    def test_no_argument_is_usage_error(self):
        rc, out = run(CLEAN, args=[])
        self.assertEqual(rc, 2)
        self.assertIn("usage", out)

    def test_missing_file_is_error(self):
        rc = subprocess.run([sys.executable, str(TOOL), "/no/such/file.md"],
                            capture_output=True, text=True).returncode
        self.assertEqual(rc, 2)

    def test_case_insensitive_and_reordered_sections(self):
        # Lowercase heading and sections in a different order still resolve.
        t = CLEAN.replace("## Acceptance criteria", "## acceptance criteria")
        rc, out = run(t)
        self.assertEqual(rc, 0, out)


# --------------------------------------------------------------------------- #
# Pin rows                                                                    #
# --------------------------------------------------------------------------- #
class PinRowTests(unittest.TestCase):
    def test_present_section_with_zero_parsable_rows_fails_loudly(self):
        broken = "## Pinned behaviors\n\nProse but no table rows at all here.\n"
        t = sub(
            "## Pinned behaviors\n\n" + CLEAN.split("## Pinned behaviors\n\n", 1)[1]
            .split("\n## Acceptance", 1)[0],
            broken)
        rc, out = run(t)
        self.assertEqual(rc, 1)
        self.assertIn("0 pin rows parsed", out)

    def test_duplicate_pin_ids_fail(self):
        t = sub("| P2 | Second scenario runs", "| P1 | Second scenario runs")
        rc, out = run(t)
        self.assertEqual(rc, 1)
        self.assertIn("duplicate pin row ids", out)

    def test_out_of_order_pin_ids_fail(self):
        # Swap P1 and P2 row order.
        row1 = "| P1 | First scenario runs | setup then act on it | the outcome holds as written | unit |"
        row2 = "| P2 | Second scenario runs | act then read the value | the value is observed once | unit |"
        t = CLEAN.replace(row1 + "\n" + row2, row2 + "\n" + row1)
        rc, out = run(t)
        self.assertEqual(rc, 1)
        self.assertIn("out of order", out)

    def test_row_named_by_no_task_fails(self):
        # Remove P3's only mention in the Tasks section.
        t = sub("Executes P3 against a real payload",
                "Executes the payload run")
        rc, out = run(t)
        self.assertEqual(rc, 1)
        self.assertIn("named by no task", out)

    def test_p10_not_matched_by_p1_word_boundary(self):
        # A P10 row must be covered by its own task mention, not by "P1".
        t = CLEAN.replace(
            "| P3 | Third scenario crashes | crash occurs mid run here | the marker is present after | manual |",
            "| P3 | Third scenario crashes | crash occurs mid run here | the marker is present after | manual |\n"
            "| P10 | Tenth scenario runs | later step occurs here | a later outcome holds too | manual |")
        # P10 mentioned by no task -> should fail even though "P1" appears.
        rc, out = run(t)
        self.assertEqual(rc, 1)
        self.assertIn("P10 (manual) is named by no task", out)


# --------------------------------------------------------------------------- #
# Group-bullet vs Kind column                                                 #
# --------------------------------------------------------------------------- #
class GroupBulletTests(unittest.TestCase):
    def _with_bullets(self, bullet_line):
        return CLEAN.replace(
            "Executes P3 against a real payload and reverts every fixture afterwards.",
            bullet_line)

    def test_manual_group_bullet_agrees_with_kind(self):
        t = self._with_bullets("Executes the manual set:\n\n- **Crash run** (P3)")
        rc, out = run(t)
        self.assertEqual(rc, 0, out)
        self.assertIn("manual-row group-bullet membership", out)

    def test_group_bullet_naming_unit_row_disagrees_with_kind(self):
        # P1 is a unit row; a group bullet claiming it must be flagged.
        t = self._with_bullets("Executes the set:\n\n- **Crash run** (P1, P3)")
        rc, out = run(t)
        self.assertEqual(rc, 1)
        self.assertIn("whose Kind column says unit", out)

    def test_prose_pid_absent_from_every_bullet_is_flagged(self):
        # Task body mentions P3 in a bullet but also names P2 in prose only.
        t = self._with_bullets(
            "Also touches P2 in passing.\n\n- **Crash run** (P3)")
        rc, out = run(t)
        self.assertEqual(rc, 1)
        self.assertIn("no group bullet lists it", out)


# --------------------------------------------------------------------------- #
# Dangling references                                                         #
# --------------------------------------------------------------------------- #
class DanglingReferenceTests(unittest.TestCase):
    def test_prose_pid_with_no_row_is_dangling(self):
        t = sub("The queue drains in arrival order and P1 governs",
                "The queue drains in arrival order and P9 governs")
        rc, out = run(t)
        self.assertEqual(rc, 1)
        self.assertIn("P9 is referenced in prose but has no row", out)


# --------------------------------------------------------------------------- #
# Acceptance criteria                                                         #
# --------------------------------------------------------------------------- #
class AcceptanceCriteriaTests(unittest.TestCase):
    def test_missing_section_is_failure(self):
        t = CLEAN.replace("## Acceptance criteria", "## Not criteria")
        rc, out = run(t)
        self.assertEqual(rc, 1)
        self.assertIn("required section 'Acceptance criteria' not found", out)

    def test_present_section_zero_items_fails(self):
        t = CLEAN.replace(
            "- [ ] **AC1** The engine builds and the demo runs to the menu without any error today.\n"
            "- [ ] **AC2** Each level starts and reaches gameplay on a clean checkout of the whole tree.\n",
            "Prose only, no list items at all in this section.\n")
        rc, out = run(t)
        self.assertEqual(rc, 1)
        self.assertIn("0 items parsed", out)

    def test_numbered_list_form_is_accepted(self):
        t = CLEAN.replace(
            "- [ ] **AC1** The engine builds and the demo runs to the menu without any error today.\n"
            "- [ ] **AC2** Each level starts and reaches gameplay on a clean checkout of the whole tree.\n",
            "1. **AC1** The engine builds and the demo runs to the menu without any error today.\n"
            "2. **AC2** Each level starts and reaches gameplay on a clean checkout of the whole tree.\n")
        rc, out = run(t)
        self.assertEqual(rc, 0, out)

    def test_checked_box_form_is_accepted(self):
        t = CLEAN.replace("- [ ] **AC1**", "- [x] **AC1**")
        rc, out = run(t)
        self.assertEqual(rc, 0, out)

    def test_duplicate_ac_ids_fail(self):
        t = CLEAN.replace("**AC2** Each level", "**AC1** Each level")
        rc, out = run(t)
        self.assertEqual(rc, 1)
        self.assertIn("duplicate AC ids", out)

    def test_non_contiguous_ac_ids_fail(self):
        t = CLEAN.replace("**AC2** Each level", "**AC3** Each level")
        rc, out = run(t)
        self.assertEqual(rc, 1)
        self.assertIn("not contiguous", out)


# --------------------------------------------------------------------------- #
# Stale citations, pivot language, review phrasing                            #
# --------------------------------------------------------------------------- #
class TextHygieneTests(unittest.TestCase):
    def test_stale_line_number_citation_flagged(self):
        t = sub("wiring the helper into the drain path directly",
                "wiring the helper (see src/drain.rs:123) into the drain path")
        rc, out = run(t)
        self.assertEqual(rc, 1)
        self.assertIn("line-number citation will go stale", out)
        self.assertIn("src/drain.rs:123", out)

    def test_pivot_language_flagged(self):
        t = sub("The queue drains in arrival order",
                "In an earlier draft the queue drained in arrival order")
        rc, out = run(t)
        self.assertEqual(rc, 1)
        self.assertIn("pivot language", out)

    def test_review_phrasing_outside_open_questions_fails(self):
        t = sub("the value is observed once", "TBD once the value is observed")
        rc, out = run(t)
        self.assertEqual(rc, 1)
        self.assertIn("unconverted review phrasing", out)

    def test_review_phrasing_inside_open_questions_is_allowed(self):
        t = sub("None outstanding at the present time.",
                "TBD whether the drain order should be configurable at all.")
        rc, out = run(t)
        self.assertEqual(rc, 0, out)


# --------------------------------------------------------------------------- #
# Restated-facts diagnostic: reports, never fails                             #
# --------------------------------------------------------------------------- #
class RestatedFactsTests(unittest.TestCase):
    def test_restated_fact_prints_but_does_not_fail(self):
        dup = ("The payload marker records the outstanding levels in bake order "
               "for the run.")
        t = sub("None outstanding at the present time.",
                "None outstanding at the present time.\n\n" + dup + "\n\n" + dup)
        rc, out = run(t)
        # A near-duplicate line pair is a diagnostic, not a failure.
        self.assertEqual(rc, 0, out)


if __name__ == "__main__":
    unittest.main()
