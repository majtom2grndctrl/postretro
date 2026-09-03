#!/usr/bin/env python3
"""Behavioral tests for apply_findings.py — stdlib only, run as `python3 <path>`.

`parse` and `_field` are imported directly; the all-or-nothing apply path is
driven through `main()` via subprocess against real files in a temp spec dir,
since that is the only way to observe the on-disk write (or its absence).
"""

import importlib.util
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

TOOL = Path(__file__).parents[1] / "apply_findings.py"

_spec = importlib.util.spec_from_file_location("apply_findings", TOOL)
AF = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(AF)


def _findings_file(body):
    fd = tempfile.NamedTemporaryFile("w", suffix=".md", delete=False)
    fd.write(body)
    fd.close()
    return fd.name


def block(fid, find, repl, loc="index.md", extra=""):
    """A single delimiter-form finding."""
    return (f"## {fid}\n\nLOCATION: {loc}\n{extra}\n"
            f"<<<FIND\n{find}\nFIND>>>\n<<<REPLACE\n{repl}\nREPLACE>>>\n")


def parse_body(body):
    return AF.parse(_findings_file(body))


def run(spec_files, finding_bodies, only=None, apply=False):
    """Run main() in a subprocess; return (rc, stdout, {name: on-disk text})."""
    d = tempfile.mkdtemp()
    for name, content in spec_files.items():
        Path(d, name).write_text(content)
    fpaths = []
    for i, body in enumerate(finding_bodies):
        p = Path(d, f"_find{i}.md")
        p.write_text(body)
        fpaths.append(str(p))
    cmd = [sys.executable, str(TOOL), "--spec-dir", d, "--findings", *fpaths]
    if only is not None:
        cmd += ["--only", only]
    if apply:
        cmd += ["--apply"]
    r = subprocess.run(cmd, capture_output=True, text=True)
    disk = {name: Path(d, name).read_text() for name in spec_files}
    return r.returncode, r.stdout + r.stderr, disk


# --------------------------------------------------------------------------- #
# Parsing: the two FIX renderings                                             #
# --------------------------------------------------------------------------- #
class FixRenderingTests(unittest.TestCase):
    def test_delimiter_form_parses(self):
        (f,) = parse_body(block("B1", "old text", "new text"))
        self.assertEqual((f["id"], f["find"], f["replace"]), ("B1", "old text", "new text"))

    def test_fenced_form_plain_labels_parses(self):
        body = "## F1\n\nFIND:\n```\nold\n```\nREPLACE:\n```\nnew\n```\n"
        (f,) = parse_body(body)
        self.assertEqual((f["find"], f["replace"]), ("old", "new"))

    def test_fenced_form_language_tagged_fence_parses(self):
        body = "## F1\n\nFIND:\n```md\nold\n```\nREPLACE:\n```md\nnew\n```\n"
        (f,) = parse_body(body)
        self.assertEqual((f["find"], f["replace"]), ("old", "new"))

    def test_fenced_form_bold_replace_label_parses(self):
        body = "## F1\n\nFIND:\n```\nold\n```\n**REPLACE**:\n```\nnew\n```\n"
        (f,) = parse_body(body)
        self.assertEqual((f["find"], f["replace"]), ("old", "new"))

    def test_fenced_form_bold_find_label_parses(self):
        # Regression: **FIND** was rejected while **REPLACE** was accepted.
        body = "## F1\n\n**FIND**:\n```\nold\n```\n**REPLACE**:\n```\nnew\n```\n"
        (f,) = parse_body(body)
        self.assertEqual((f["find"], f["replace"]), ("old", "new"))

    def test_multiline_find_and_replace(self):
        (f,) = parse_body(block("B1", "line one\nline two", "only one"))
        self.assertEqual(f["find"], "line one\nline two")


# --------------------------------------------------------------------------- #
# _field: three shapes                                                        #
# --------------------------------------------------------------------------- #
class FieldHelperTests(unittest.TestCase):
    def test_plain_line(self):
        self.assertEqual(AF._field("SEVERITY: Blocker\n", "SEVERITY"), "Blocker")

    def test_table_row(self):
        self.assertEqual(AF._field("| SEVERITY | Complicates |\n", "SEVERITY"), "Complicates")

    def test_bold_field(self):
        self.assertEqual(AF._field("**SEVERITY**: Nit\n", "SEVERITY"), "Nit")

    def test_bold_field_with_bullet(self):
        self.assertEqual(AF._field("- **SEVERITY**: Nit\n", "SEVERITY"), "Nit")

    def test_absent_field_is_none(self):
        self.assertIsNone(AF._field("nothing here\n", "SEVERITY"))


# --------------------------------------------------------------------------- #
# Finding-id extraction from headings                                         #
# --------------------------------------------------------------------------- #
class HeadingIdTests(unittest.TestCase):
    def _id(self, heading):
        body = f"## {heading}\n\n" + "<<<FIND\nx\nFIND>>>\n<<<REPLACE\ny\nREPLACE>>>\n"
        return parse_body(body)[0]["id"]

    def test_em_dash_title(self):
        self.assertEqual(self._id("B1 — Casing mismatch"), "B1")

    def test_spaced_hyphen_title(self):
        self.assertEqual(self._id("B2 - Casing mismatch"), "B2")

    def test_colon_title_no_leading_space(self):
        # Regression: the common "ID: Title" form must yield the bare id.
        self.assertEqual(self._id("B3: Casing mismatch"), "B3")

    def test_colon_title_spaced(self):
        self.assertEqual(self._id("B4 : Casing mismatch"), "B4")

    def test_bare_id_no_title(self):
        self.assertEqual(self._id("P7a"), "P7a")

    def test_skips_self_audit_pin_table_summary(self):
        body = (block("SELF-AUDIT", "a", "b")
                + block("PIN TABLE", "c", "d")
                + block("SUMMARY of findings", "e", "f")
                + block("G1", "g", "h"))
        ids = [f["id"] for f in parse_body(body)]
        self.assertEqual(ids, ["G1"])


# --------------------------------------------------------------------------- #
# File targeting from LOCATION                                                #
# --------------------------------------------------------------------------- #
class TargetingTests(unittest.TestCase):
    def test_research_md_in_location_routes_to_research(self):
        (f,) = parse_body(block("B1", "x", "y", loc="research.md section 2"))
        self.assertEqual(f["file"], "research.md")

    def test_index_when_location_says_index(self):
        (f,) = parse_body(block("B1", "x", "y", loc="index.md"))
        self.assertEqual(f["file"], "index.md")

    def test_no_location_defaults_to_index(self):
        body = "## B1\n\n<<<FIND\nx\nFIND>>>\n<<<REPLACE\ny\nREPLACE>>>\n"
        self.assertEqual(parse_body(body)[0]["file"], "index.md")

    def test_research_finding_lacking_literal_string_aborts_safely(self):
        # LOCATION that does not contain "research.md" routes to index.md;
        # when the text is not there the run aborts rather than mis-editing.
        rc, out, disk = run(
            {"index.md": "idx only\n", "research.md": "target here\n"},
            [block("B1", "target", "REP", loc="the research doc, section 2")],
            apply=True)
        self.assertEqual(rc, 1)
        self.assertIn("matches 0 times in index.md", out)
        self.assertEqual(disk["research.md"], "target here\n")
        self.assertEqual(disk["index.md"], "idx only\n")


# --------------------------------------------------------------------------- #
# --only selection                                                            #
# --------------------------------------------------------------------------- #
class OnlySelectionTests(unittest.TestCase):
    def test_only_applies_the_selected_id(self):
        rc, out, disk = run(
            {"index.md": "aaa bbb\n"},
            [block("A", "aaa", "111") + block("B", "bbb", "222")],
            only="A", apply=True)
        self.assertEqual(rc, 0)
        self.assertEqual(disk["index.md"], "111 bbb\n")

    def test_selected_id_absent_everywhere_exits_2_nothing_applied(self):
        rc, out, disk = run(
            {"index.md": "aaa\n"}, [block("A", "aaa", "111")],
            only="NOPE", apply=True)
        self.assertEqual(rc, 2)
        self.assertEqual(disk["index.md"], "aaa\n")


# --------------------------------------------------------------------------- #
# FIND must match exactly once                                                #
# --------------------------------------------------------------------------- #
class ExactlyOnceTests(unittest.TestCase):
    def test_zero_matches_is_hard_failure_nothing_written(self):
        rc, out, disk = run({"index.md": "hello\n"},
                            [block("A", "absent", "x")], apply=True)
        self.assertEqual(rc, 1)
        self.assertEqual(disk["index.md"], "hello\n")

    def test_two_matches_is_hard_failure_nothing_written(self):
        rc, out, disk = run({"index.md": "dup dup\n"},
                            [block("A", "dup", "x")], apply=True)
        self.assertEqual(rc, 1)
        self.assertIn("matches 2 times", out)
        self.assertEqual(disk["index.md"], "dup dup\n")

    def test_exactly_once_applies(self):
        rc, out, disk = run({"index.md": "unique here\n"},
                            [block("A", "unique", "changed")], apply=True)
        self.assertEqual(rc, 0)
        self.assertEqual(disk["index.md"], "changed here\n")


# --------------------------------------------------------------------------- #
# Collision detection and its boundary                                        #
# --------------------------------------------------------------------------- #
class CollisionTests(unittest.TestCase):
    def test_overlapping_spans_abort_batch(self):
        rc, out, disk = run({"index.md": "AABBB\n"},
                            [block("A", "AAB", "x") + block("B", "BBB", "y")],
                            apply=True)
        self.assertEqual(rc, 1)
        self.assertIn("COLLISION", out)
        self.assertEqual(disk["index.md"], "AABBB\n")

    def test_adjacent_touching_spans_do_not_collide(self):
        # A span ending exactly where the next begins (b0 == a1) is legal.
        rc, out, disk = run({"index.md": "AABB\n"},
                            [block("A", "AA", "XX") + block("B", "BB", "YY")],
                            apply=True)
        self.assertEqual(rc, 0)
        self.assertNotIn("COLLISION", out)
        self.assertEqual(disk["index.md"], "XXYY\n")


# --------------------------------------------------------------------------- #
# All-or-nothing and reverse-offset application                               #
# --------------------------------------------------------------------------- #
class ApplyIntegrityTests(unittest.TestCase):
    def test_one_blocked_finding_leaves_file_byte_identical(self):
        orig = "hello world\n"
        rc, out, disk = run({"index.md": orig},
                            [block("A", "hello", "hi") + block("B", "absent", "z")],
                            apply=True)
        self.assertEqual(rc, 1)
        self.assertEqual(disk["index.md"], orig)

    def test_multiple_non_overlapping_findings_all_land(self):
        rc, out, disk = run({"index.md": "one two three four\n"},
                            [block("A", "one", "1") + block("B", "two", "2")
                             + block("C", "four", "4")], apply=True)
        self.assertEqual(rc, 0)
        self.assertEqual(disk["index.md"], "1 2 three 4\n")

    def test_dry_run_writes_nothing(self):
        orig = "unique here\n"
        rc, out, disk = run({"index.md": orig},
                            [block("A", "unique", "changed")], apply=False)
        self.assertEqual(rc, 0)
        self.assertEqual(disk["index.md"], orig)


# --------------------------------------------------------------------------- #
# Post-verify                                                                 #
# --------------------------------------------------------------------------- #
class PostVerifyTests(unittest.TestCase):
    def test_replace_reintroducing_another_finds_text_is_allowed(self):
        # Regression: B replaces baz->foo, legitimately reintroducing the text
        # A's FIND matched. Both edits landed; post-verify must not block them.
        rc, out, disk = run({"index.md": "foo mid baz\n"},
                            [block("A", "foo", "bar") + block("B", "baz", "foo")],
                            apply=True)
        self.assertEqual(rc, 0, out)
        self.assertEqual(disk["index.md"], "bar mid foo\n")


# --------------------------------------------------------------------------- #
# Literal (non-regex) matching                                                #
# --------------------------------------------------------------------------- #
class LiteralMatchTests(unittest.TestCase):
    def test_find_with_regex_metacharacters_is_literal(self):
        # If matching were regexified, "a.b+c (x)" would not match itself; the
        # tool uses str.count/str.index, so it matches exactly once.
        rc, out, disk = run({"index.md": "value is a.b+c (x)\n"},
                            [block("A", "a.b+c (x)", "OK")], apply=True)
        self.assertEqual(rc, 0)
        self.assertEqual(disk["index.md"], "value is OK\n")

    def test_regex_pattern_does_not_match_other_text(self):
        # A regex "a.c" would match "abc"; a literal "a.c" must not.
        rc, out, disk = run({"index.md": "abc here\n"},
                            [block("A", "a.c", "OK")], apply=True)
        self.assertEqual(rc, 1)  # literal "a.c" absent -> 0 matches -> fail
        self.assertEqual(disk["index.md"], "abc here\n")


# --------------------------------------------------------------------------- #
# DROPS advisory                                                              #
# --------------------------------------------------------------------------- #
class DropsAdvisoryTests(unittest.TestCase):
    def test_real_deletion_of_a_long_run_is_reported(self):
        rc, out, disk = run(
            {"index.md": "lead quick brown fox jumps over lazy tail\n"},
            [block("A", "lead quick brown fox jumps over lazy tail", "lead tail")])
        self.assertIn("DROPS", out)
        self.assertIn("quick brown fox jumps over lazy", out)

    def test_preserved_prefix_is_not_reported(self):
        # Regression: a long unchanged prefix must not be flagged as dropped.
        rc, out, disk = run(
            {"index.md": "a b c d e f X and more words here now\n"},
            [block("A", "a b c d e f X", "a b c d e f Y")])
        self.assertNotIn("DROPS", out)

    def test_reorder_is_not_reported_as_dropped(self):
        rc, out, disk = run(
            {"index.md": "alpha beta gamma delta epsilon zeta\n"},
            [block("A", "alpha beta gamma delta epsilon zeta",
                   "zeta epsilon delta gamma beta alpha")])
        self.assertNotIn("DROPS", out)


if __name__ == "__main__":
    unittest.main()
