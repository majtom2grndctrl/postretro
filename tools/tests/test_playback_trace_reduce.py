"""Tests for the playback trace reducer (`tools/playback_trace_reduce.py`).

The checked-in sample (`fixtures/playback_trace/`) is trimmed from one real
playback run: the first Mac run of camera-route playback (branch
`mac/camera-path-checkpoint` 159f47d64; AMD Radeon Pro 5300M, Metal,
campaign-test far-point, hold 900, vsync). These ranges are the authority the
fixture README points to. It keeps:

* `metal-application-intervals.xml` and `metal-gpu-intervals.xml`: the marked
  encoder rows, their command buffer rows and the GPU rows joined to them for
  frames 724-727 (hold), 1133-1141 and 1246-1250, plus a few unrelated rows.
  Rows are the export's own lines; a value another row defined is inlined at
  its first use, so `id`/`ref` de-duplication stays as xctrace writes it.
* `record.playback.json`: the run's record, from before the drain, cold and
  growth columns existed (no drain flags, marks or CPU columns, and no
  `gpu_clock` in its header), its rows sliced to 720-727, 1134-1141 and
  1247-1250. Frames 720-723 precede the trace (it attached mid-hold).
* `toc.xml`: the export's table of contents, cut to its summary.
* `ioreg.log`: 8 clock polls, from trace time 6.4 s to 10.4 s.

Its marks predate the site tag (`pr-frame <k>`, not `pr-frame <k> <site>`),
and wgpu relabelled the last command buffer of some submits "(wgpu internal)
Signal": frames 1136, 1139, 1141 and 1247 each hold marked work on such a
buffer, which the reducer cannot attribute.

The expected values were checked by hand against the raw rows. Site-tagged
marks, cold, growth, change and gap marks, partial traces and late drains are
covered by synthetic cases that follow the current record schema
(`crates/postretro/src/playback/record/`).
"""

import contextlib
import importlib.util
import io
import json
import lzma
import shutil
import tempfile
import unittest
from pathlib import Path

FIXTURE = Path(__file__).parent / "fixtures" / "playback_trace"
RECORD = FIXTURE / "record.playback.json"
IOREG = FIXTURE / "ioreg.log"


def load_reducer_module():
    script = Path(__file__).parents[1] / "playback_trace_reduce.py"
    spec = importlib.util.spec_from_file_location("playback_trace_reduce_test", script)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


R = load_reducer_module()


def run_cli(*args):
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        code = R.main([str(a) for a in args])
    return code, out.getvalue(), err.getvalue()


def by_frame(summary):
    return {f["frame"]: f for f in summary["frames"]}


def gpu_ms(frame, resource):
    value = frame["drains"][resource]["gpu_us"]
    return None if value is None else value / 1000.0


# ------------------------------------------------------------ the real sample


class SampleTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.summary = R.reduce_paths(RECORD, FIXTURE, IOREG)
        cls.frames = by_frame(cls.summary)

    def test_every_route_frame_in_the_sample_matches_a_frame_mark(self):
        route = [f for f in self.summary["frames"] if f["phase"] == "route"]
        self.assertEqual(len(route), 12)
        self.assertTrue(all(f["matched"] for f in route))
        self.assertEqual(self.summary["join"]["route_unmatched"], 0)

    def test_hold_frames_before_the_trace_attached_are_unmatched_not_an_error(self):
        unmatched = [f["frame"] for f in self.summary["frames"] if not f["matched"]]
        # 720-723 precede the trace; 724 is its first marked frame, whose
        # earlier encoders the attach may have missed.
        self.assertEqual(unmatched, [720, 721, 722, 723, 724])
        self.assertEqual(self.summary["join"]["hold_unmatched"], 5)
        self.assertEqual(self.summary["trace"]["marked_frames_without_record_row"], 2)

    def test_per_frame_drain_gpu_is_the_union_of_each_frames_marked_intervals(self):
        f = self.frames
        # Hand-checked unions of the raw SDMA0 and GFX rows, in ms.
        self.assertAlmostEqual(gpu_ms(f[1137], "sh"), 1.883889, places=6)  # 8 rows, no overlap
        self.assertAlmostEqual(gpu_ms(f[1138], "sh"), 1.943563, places=6)  # SDMA0/GFX overlap
        self.assertAlmostEqual(gpu_ms(f[1248], "lightmap"), 0.421549, places=6)
        self.assertEqual(f[1138]["drains"]["sh"]["encoders"], 3)
        self.assertAlmostEqual(gpu_ms(f[1248], "total"), 0.421549, places=6)
        for frame in (1134, 1135, 1140, 1249, 1250, 725):
            self.assertEqual(f[frame]["drains"]["total"]["state"], "none", frame)
            self.assertIsNone(f[frame]["unattributed"], frame)

    def test_marked_work_on_a_relabelled_buffer_makes_the_drains_missing(self):
        # Old-format marks on "(wgpu internal) Signal" command buffers. Their
        # unions, from the raw rows, in ms:
        # * 1136, 0x...464: SDMA0 7340.808168-7340.822685 (0.014517) and GFX
        #   7341.172740-7342.524690 (1.351950): 1.366467. Its four labelled SH
        #   buffers alone give 1.239487, which used to read as the whole drain.
        # * 1139, 0x...626 and 0x...62c: SDMA0 0.024335 + 0.079098 + 0.055076 +
        #   0.044113 and GFX 1.143998 + 0.243942 + 0.364242 + 0.111690: 2.066494,
        #   beside 0.019994 of labelled SH.
        # * 1141, 0x...755: 0.026446 + 0.539511 = 0.565957; 1247, 0x...27d: 0.016428.
        f = self.frames
        for frame, unattributed in ((1136, 1.366467), (1139, 2.066494), (1141, 0.565957),
                                    (1247, 0.016428)):
            work = f[frame]["unattributed"]
            self.assertAlmostEqual(work["gpu_us"] / 1000.0, unattributed, places=6, msg=frame)
            self.assertEqual(work["relabelled"], work["encoders"], frame)
            for resource in ("lightmap", "sh", "total"):
                entry = f[frame]["drains"][resource]
                self.assertEqual(entry["state"], "missing", (frame, resource))
                self.assertIsNone(entry["gpu_us"], (frame, resource))
                self.assertIn(R.UNATTRIBUTED_REASON, entry["reason"], (frame, resource))
        self.assertEqual(f[1136]["drains"]["sh"]["encoders"], 4)
        self.assertEqual(f[1139]["unattributed"]["encoders"], 2)
        u = self.summary["unattributed"]
        self.assertEqual((u["frames"], u["frame_list"], u["encoders"], u["relabelled"]),
                         (4, "1136, 1139, 1141, 1247", 5, 5))
        self.assertAlmostEqual(u["gpu_ms_sum"], 4.015346, places=6)
        self.assertAlmostEqual(u["gpu_ms_max"], 2.066494, places=6)
        trace = self.summary["trace"]
        self.assertEqual((trace["marks_with_site"], trace["marks"]), (0, 33))
        self.assertEqual(trace["marked_encoders"][R.OTHER], 5)

    def test_per_frame_columns_carry_phase_interval_and_unknown_flags(self):
        f = self.frames[1137]
        self.assertEqual((f["phase"], f["pose"], f["interval_us"]), ("route", 237, 17603))
        self.assertIsNone(f["cold"], "a minimal record's cold flag is unknown, not false")
        self.assertIsNone(f["pool_grew"])
        self.assertFalse(f["marks_recorded"])
        self.assertEqual(self.frames[725]["phase"], "hold")

    def test_per_run_route_intervals(self):
        route = self.summary["route"]
        self.assertEqual((route["rows"], route["n"]), (12, 12))
        self.assertAlmostEqual(route["p50"], 15.946)
        self.assertAlmostEqual(route["p95"], 23.102)
        self.assertAlmostEqual(route["max"], 23.102)
        self.assertEqual(route["over_budget"], 5)
        hold = self.summary["apart"]["hold"]
        self.assertEqual((hold["rows"], hold["matched"], hold["n"]), (8, 3, 3))
        self.assertAlmostEqual(hold["max"], 18.718)
        self.assertEqual(self.summary["record"]["marks_unrecorded_rows"], 20)

    def test_per_run_drain_gpu(self):
        d = self.summary["drain_gpu"]["resources"]
        self.assertEqual(d["lightmap"]["n"], 1)
        self.assertAlmostEqual(d["lightmap"]["max"], 0.421549, places=6)
        # 1137 and 1138; 1136 and 1139 hold unattributed work and read missing.
        self.assertEqual(d["sh"]["n"], 2)
        self.assertAlmostEqual(d["sh"]["p50"], 1.883889, places=6)
        self.assertAlmostEqual(d["sh"]["max"], 1.943563, places=6)
        self.assertEqual(d["total"]["n"], 3)
        self.assertEqual(self.summary["drain_gpu"]["missing"],
                         {"lightmap": 4, "sh": 4, "total": 4})  # 1136, 1139, 1141, 1247
        cats = self.summary["drain_gpu"]["categories"]["total"]
        self.assertEqual(cats["unknown"]["n"], 3, "a minimal record's drains are unknown")
        self.assertEqual((cats["warm"]["n"], cats["cold"]["n"], cats["growth"]["n"]), (0, 0, 0))

    def test_over_budget_charge_reports_both_readings(self):
        # Row k's interval runs from frame k - 1's start S(k - 1) to S(k), so
        # own is frame k - 1's drain. The trace has no frame-start event, so
        # S(k - 1) is bounded from the marked rows (trace ms below): a frame's
        # marked encoder rows and their command buffer rows all fall after its
        # own start and before the next frame's.
        #   U = min(earliest(k - 1), earliest(k) - interval_k)
        #   L = max(latest(k - 2), latest(k - 1) - interval_k - 0.001)
        # A drain ending <= L was done; one ending > U was in flight; one
        # between may have been. A drain's end is the latest end of its
        # encoders' GPU rows.
        # * 1135: 1134 drained nothing, so own is 0. L = latest(1133) =
        #   7297.568393, the end of 1133's Frame Encoder buffer row; U =
        #   min(7311.056378, 7326.173606 - 17.596) = 7308.577606. The slice
        #   drops 1132, so 1133 is uncovered, and 1134 opens the covered
        #   stretch at 7311.056378 > L: an earlier drain may be unseen, so the
        #   in-flight reading is missing.
        # * 1137: own is 1136's drain, missing: 1136 holds unattributed work.
        #   L = max(7330.643551, 7348.437393 - 17.603 - 0.001) = 7330.833393;
        #   U = min(7334.705799, 7351.752213 - 17.603) = 7334.149213. No frame
        #   before 1136 drained.
        # * 1139: own is 1138's SH, 1.943563 ms. L = max(latest(1137) =
        #   7364.473924, its Frame Encoder buffer row's end, 7380.786971 -
        #   16.684 - 0.001); U = min(earliest(1138) = 7367.581896,
        #   7384.353071 - 16.684 = 7367.669071) = 7367.581896. 1137's SH ended
        #   at 7367.090090, inside (L, U]: it may have been in flight. A bound
        #   from 1139's frame encoder alone (7375.903 ms) would call it done.
        # * 1140: own is 1139's drain, missing (unattributed). L = latest(1138)
        #   = 7380.786971; U = min(7384.353071, 7410.554540 - 17.945). 1138's SH
        #   ended at 7380.093717 and 1136's work at 7343.413473, both before L.
        # * 1250: 1249 drained nothing. L = latest(1248) = 9279.102342; U =
        #   min(9294.900917, 9309.103457 - 23.102) = 9286.001457. 1248's
        #   lightmap ended at 9274.998997 and 1247's work at 9257.188035: no
        #   drain at all.
        charge = self.summary["over_budget_charge"]
        self.assertEqual(charge["over_budget"], 5)
        rows = {f["frame"]: f for f in charge["frames"]}
        self.assertEqual(sorted(rows), [1135, 1137, 1139, 1140, 1250])
        self.assertEqual(rows[1137]["own_frame"], 1136)
        for frame, lower, upper in ((1135, 7297.568393, 7308.577606),
                                    (1137, 7330.833393, 7334.149213),
                                    (1139, 7364.473924, 7367.581896),
                                    (1140, 7380.786971, 7384.353071),
                                    (1250, 9279.102342, 9286.001457)):
            self.assertAlmostEqual(rows[frame]["start_lower_ns"] / 1e6, lower, places=6,
                                   msg=frame)
            self.assertAlmostEqual(rows[frame]["start_upper_ns"] / 1e6, upper, places=6,
                                   msg=frame)
        for frame, own in ((1135, 0.0), (1139, 1.943563), (1250, 0.0)):
            self.assertAlmostEqual(rows[frame]["own_ms"], own, places=6, msg=frame)
        for frame in (1137, 1140):
            self.assertEqual(rows[frame]["own_state"], "missing", frame)
            self.assertIn(R.UNATTRIBUTED_REASON, rows[frame]["own_reason"])
            self.assertIsNone(rows[frame]["own_plus_in_flight_ms"], frame)
        for frame in rows:
            self.assertEqual(rows[frame]["in_flight_frames"], [], frame)
            self.assertEqual(rows[frame]["in_flight_partial_frames"], [], frame)
        self.assertEqual(rows[1139]["in_flight_possible_frames"], [1137])
        self.assertIsNone(rows[1139]["own_plus_in_flight_ms"])
        self.assertEqual(rows[1135]["own_state"], "none")
        self.assertIsNone(rows[1135]["own_plus_in_flight_ms"])
        self.assertIn("opens the trace's covered stretch", rows[1135]["in_flight_reason"])
        for frame in (1137, 1140, 1250):
            self.assertIsNone(rows[frame]["in_flight_reason"], frame)
        self.assertAlmostEqual(rows[1250]["own_plus_in_flight_ms"], 0.0)
        self.assertEqual((charge["with_own_drain"], charge["with_own_or_in_flight_drain"],
                          charge["with_possible_in_flight_drain"], charge["with_no_drain"],
                          charge["own_missing"], charge["in_flight_missing"]),
                         (1, 1, 1, 1, 2, 2))
        self.assertEqual([(b["cause"], b["frame"], b["rows"])
                          for b in charge["in_flight_blanked_by"]],
                         [("uncovered", 1134, 1), ("possible", 1137, 1)])

    def test_clock_joins_the_route_span_by_time(self):
        clock = self.summary["gpu_clock"]
        self.assertEqual(clock["state"], "recorded")
        # Polls 178.571, 179.127, 179.682 and 180.239 fall in the span.
        self.assertEqual((clock["polls"], clock["readings"]), (4, 16))
        self.assertAlmostEqual(clock["median_mhz"], 902.5)
        self.assertEqual((clock["min_mhz"], clock["max_mhz"]), (746, 1132))
        self.assertIn("GPU clock: median 902.5 MHz (min 746, max 1132", clock["text"])

    def test_text_names_the_clock_beside_every_gpu_figure(self):
        code, out, _ = run_cli("--record", RECORD, "--trace-dir", FIXTURE, "--ioreg", IOREG)
        self.assertEqual(code, 0)
        clock_line = "GPU clock: median 902.5 MHz"
        for section in ("Reported apart", "Drain GPU time", "Over-budget charge", "Per frame"):
            block = out.split(section, 1)[1].splitlines()
            self.assertIn(clock_line, block[1], section)
        self.assertIn("route frames: 12; matched 12; unmatched 0", out)
        self.assertIn("change and gap marks: not in the record", out)

    def test_json_output_round_trips(self):
        code, out, _ = run_cli("--record", RECORD, "--trace-dir", FIXTURE, "--json")
        self.assertEqual(code, 0)
        summary = json.loads(out)
        self.assertEqual(summary["join"]["route_matched"], 12)
        self.assertEqual(summary["gpu_clock"]["state"], "unrecorded")

    def test_without_ioreg_the_clock_reads_unrecorded(self):
        summary = R.reduce_paths(RECORD, FIXTURE)
        self.assertEqual(summary["gpu_clock"]["state"], "unrecorded")
        # The sample's header predates `gpu_clock`: it names no clock.
        self.assertIsNone(summary["record"]["gpu_clock"])
        self.assertIn("the record names no clock", summary["gpu_clock"]["text"])
        code, out, _ = run_cli("--record", RECORD, "--trace-dir", FIXTURE)
        self.assertEqual(code, 0)
        self.assertNotIn("MHz", out)
        self.assertEqual(out.count("GPU clock: unrecorded"), 4)

    def test_without_a_toc_the_clock_covers_the_whole_log_labelled(self):
        with tempfile.TemporaryDirectory() as tmp:
            for name in ("metal-application-intervals.xml", "metal-gpu-intervals.xml"):
                shutil.copy(FIXTURE / name, tmp)
            summary = R.reduce_paths(RECORD, tmp, IOREG)
        clock = summary["gpu_clock"]
        self.assertEqual(clock["state"], "recorded")
        self.assertIn("whole poll log (time bases not aligned", clock["text"])
        self.assertEqual(clock["polls"], 8)

    def test_xz_compressed_tables_read_the_same(self):
        with tempfile.TemporaryDirectory() as tmp:
            for name in ("metal-application-intervals.xml", "metal-gpu-intervals.xml", "toc.xml"):
                with lzma.open(Path(tmp) / (name + ".xz"), "wb") as out:
                    out.write((FIXTURE / name).read_bytes())
            summary = R.reduce_paths(RECORD, tmp, IOREG)
        self.assertEqual(summary["drain_gpu"], self.summary["drain_gpu"])
        self.assertEqual(summary["gpu_clock"], self.summary["gpu_clock"])

    def test_an_explicit_toc_outside_the_trace_dir_aligns_the_clock(self):
        with tempfile.TemporaryDirectory() as tables, tempfile.TemporaryDirectory() as other:
            for name in ("metal-application-intervals.xml", "metal-gpu-intervals.xml"):
                shutil.copy(FIXTURE / name, tables)
            toc = Path(other) / "elsewhere.xml"
            shutil.copy(FIXTURE / "toc.xml", toc)
            code, out, err = run_cli("--record", RECORD, "--trace-dir", tables, "--ioreg", IOREG,
                                     "--toc", toc, "--json")
        self.assertEqual(code, 0, err)
        clock = json.loads(out)["gpu_clock"]
        self.assertEqual(clock["scope"], "the trace's matched route span")
        self.assertEqual((clock["polls"], clock["readings"]), (4, 16))
        self.assertAlmostEqual(clock["median_mhz"], 902.5)

    def test_a_corrupt_or_truncated_xz_table_exits_2(self):
        whole = lzma.compress((FIXTURE / "metal-application-intervals.xml").read_bytes())
        for name, data in (("corrupt", b"\xfd7zXZ\x00 not an xz stream"),
                           ("truncated", whole[: len(whole) // 2])):
            with self.subTest(name), tempfile.TemporaryDirectory() as tmp:
                (Path(tmp) / "metal-application-intervals.xml.xz").write_bytes(data)
                shutil.copy(FIXTURE / "metal-gpu-intervals.xml", tmp)
                code, out, err = run_cli("--record", RECORD, "--trace-dir", tmp)
                self.assertEqual(code, 2)
                self.assertEqual(out, "")
                self.assertIn("playback_trace_reduce: error:", err)


# ------------------------------------------------------------ synthetic traces

FRAME_NS = 16_666_667
APP_COLUMNS = ("start", "duration", "process", "event-depth", "event-label", "event-type",
               "cmdbuffer-id", "encoder-id")
GPU_COLUMNS = ("start", "duration", "channel-name", "frame-number", "event-label", "process",
               "cmdbuffer-id", "encoder-id")
COMMAND_BUFFER_LABEL = {"lightmap": "Streamed Lightmap Drain", "sh": "Streamed SH Upload Copies",
                        "frame": "Frame Encoder"}


class Trace:
    """Builds the two tables in the export's shape, with id/ref de-duplication."""

    def __init__(self):
        self.app = []
        self.gpu = []
        self.next_cmdbuffer = 0x1000

    def encoder(self, frame, kind, cpu_ns, gpu=(), marked=True, buffer_mark=False,
                site=True, buffer_label=None, unmarked_gpu=()):
        """One command buffer with one encoder; `gpu` holds (start, duration, channel).
        `marked` marks the encoder row, `buffer_mark` the command buffer row.
        The mark is the current `pr-frame <k> <site>`, with `kind` as the site
        or `site` when it is a string; `site=False` writes the older
        `pr-frame <k>`. `buffer_label` replaces the command buffer's label, as
        wgpu's "(wgpu internal) Signal" relabel does. `unmarked_gpu` adds a
        second, unmarked encoder to the same command buffer, as wgpu's split of
        one buffer into several Metal encoders does. Returns the marked
        encoder's id; its command buffer's id is one less."""
        cmdbuffer = self.next_cmdbuffer
        encoder = cmdbuffer + 1
        self.next_cmdbuffer += 3
        label = buffer_label or COMMAND_BUFFER_LABEL[kind]
        name = site if isinstance(site, str) else kind
        text = f"pr-frame {frame} {name}" if site else f"pr-frame {frame}"
        buffer = f"  -  :{text}" if buffer_mark else ""
        self.app.append((cpu_ns, 50_000,
                         f"{label}{buffer}  (Frame  {frame % 7} ) ( postretro (1) )",
                         34, cmdbuffer, R.ALL_ONES, "Encoding"))
        mark = f"  -  :{text}" if marked else ""
        self.app.append((cpu_ns + 1_000, 30_000,
                         f"Blit Command 0{mark}  ( Main Thread (0x1) (postretro, pid: 1) )",
                         35, cmdbuffer, encoder, "Encoding"))
        for start, duration, channel in gpu:
            self.gpu.append((start, duration, channel, cmdbuffer, encoder,
                             f"{label}:Blit Command 0  ( postretro (1) )  0x{encoder:x}"))
        if unmarked_gpu:
            second = cmdbuffer + 2
            self.app.append((cpu_ns + 32_000, 10_000,
                             "Blit Command 1  ( Main Thread (0x1) (postretro, pid: 1) )",
                             35, cmdbuffer, second, "Encoding"))
            for start, duration, channel in unmarked_gpu:
                self.gpu.append((start, duration, channel, cmdbuffer, second,
                                 f"{label}:Blit Command 1  ( postretro (1) )  0x{second:x}"))
        return encoder

    def buffer_event(self, encoder, start_ns, duration_ns, event_type):
        """Another row for `encoder`'s command buffer, of `event_type`."""
        self.app.append((start_ns, duration_ns, "Frame Encoder  ( postretro (1) )", 34,
                         encoder - 1, R.ALL_ONES, event_type))

    def frame(self, k, start_ns, drains=(), gpu_offset=4_000_000):
        """Frame k: its drains (kind, gpu rows), then its frame encoder."""
        cpu = start_ns
        for kind, gpu in drains:
            self.encoder(k, kind, cpu, gpu)
            cpu += 200_000
        self.encoder(k, "frame", cpu, [(start_ns + gpu_offset, 2_000_000, "GFX")])

    def write(self, directory):
        directory = Path(directory)
        (directory / "app.xml").write_text(self._app_xml(), encoding="utf-8")
        (directory / "gpu.xml").write_text(self._gpu_xml(), encoding="utf-8")

    @staticmethod
    def _schema(name, columns):
        cols = "".join(f"<col><mnemonic>{c}</mnemonic></col>" for c in columns)
        return f'<node xpath="x"><schema name="{name}">{cols}</schema>'

    def _app_xml(self):
        ids = Ids()
        rows = []
        for start, dur, label, depth, cmdbuffer, encoder, event_type in self.app:
            rows.append(
                "<row>"
                f'<start-time id="{ids.new()}" fmt="t">{start}</start-time>'
                f'<duration id="{ids.new()}" fmt="d">{dur}</duration>'
                + ids.process()
                + ids.value("metal-nesting-level", str(depth), str(depth))
                + ids.value("formatted-label", label, "")
                + ids.value("metal-event-name", event_type, event_type)
                + ids.value("metal-command-buffer-id", hex(cmdbuffer), str(cmdbuffer))
                + ids.value("metal-command-buffer-id", hex(encoder), str(encoder))
                + "</row>")
        return ('<?xml version="1.0"?>\n<trace-query-result>\n'
                + self._schema(R.APP_SCHEMA, APP_COLUMNS) + "\n".join(rows)
                + "\n</node></trace-query-result>\n")

    def _gpu_xml(self):
        ids = Ids()
        rows = []
        for start, dur, channel, cmdbuffer, encoder, label in self.gpu:
            rows.append(
                "<row>"
                f'<start-time id="{ids.new()}" fmt="t">{start}</start-time>'
                f'<duration id="{ids.new()}" fmt="d">{dur}</duration>'
                + ids.value("gpu-channel-name", channel, channel)
                + "<sentinel/>"
                + ids.value("formatted-label", label, "", inner=ids.process())
                + ids.process()
                + ids.value("metal-command-buffer-id", hex(cmdbuffer), str(cmdbuffer))
                + ids.value("metal-command-buffer-id", hex(encoder), str(encoder))
                + "</row>")
        # One unrelated row from another process, joined to nothing.
        rows.append('<row><start-time id="900000" fmt="t">5</start-time>'
                    '<duration id="900001" fmt="d">5</duration>'
                    '<gpu-channel-name id="900002" fmt="Channel 11">Channel 11</gpu-channel-name>'
                    '<sentinel/><formatted-label id="900003" fmt="Page On"/>'
                    '<process id="900004" fmt="WindowServer (245)"/>'
                    '<metal-command-buffer-id id="900005" fmt="0x0">0</metal-command-buffer-id>'
                    '<metal-command-buffer-id ref="900005"/></row>')
        return ('<?xml version="1.0"?>\n<trace-query-result>\n'
                + self._schema(R.GPU_SCHEMA, GPU_COLUMNS) + "\n".join(rows)
                + "\n</node></trace-query-result>\n")


class Ids:
    """xctrace's de-duplication: a repeated value is a `ref` to its first `id`."""

    def __init__(self):
        self.counter = 0
        self.seen = {}
        self.process_id = None

    def new(self):
        self.counter += 1
        return self.counter

    def value(self, tag, fmt, text, inner=""):
        key = (tag, fmt, text)
        if key in self.seen:
            return f'<{tag} ref="{self.seen[key]}"/>'
        ident = self.new()
        self.seen[key] = ident
        return f'<{tag} id="{ident}" fmt="{fmt}">{text}{inner}</{tag}>'

    def process(self):
        # Defined once, nested inside the first label as in the real GPU
        # table; every later process cell is a ref to it.
        if self.process_id is None:
            self.process_id = self.new()
            return f'<process id="{self.process_id}" fmt="postretro (1)"/>'
        return f'<process ref="{self.process_id}"/>'


DRAIN_FIELDS = dict(
    lightmap_admitted_items=0, lightmap_admitted_bytes=0, sh_admitted_items=0,
    sh_admitted_bytes=0, cold=False, cold_lightmap=False, cold_sh=False, pool_grew=False,
    lightmap_pool_grew=False, mask_pool_grew=False, lightmap_late_block_frames=0,
    sh_late_cluster_frames=0)


def row(frame, phase="route", interval_us=16_000, changes=(), gap=False, **drain):
    """A current-schema record row (`playback/record/{mod,row}.rs`)."""
    fields = dict(DRAIN_FIELDS)
    fields.update(drain)
    for resource in ("lightmap", "sh"):
        if fields[f"cold_{resource}"]:
            fields["cold"] = True
    if fields["lightmap_pool_grew"] or fields["mask_pool_grew"]:
        fields["pool_grew"] = True
    return {"frame": frame, "phase": phase, "pose": max(frame - 2, 0), "route_time_s": None,
            "interval_us": interval_us, "changes": list(changes), "gap": gap, "cpu_ns": None,
            "ticks": None, "drain": fields, "gpu": None}


def record(rows):
    return {"header": {"schema": "postretro.camera_route_playback.v1", "pacing": "frame-locked",
                       "hold_frames": 2, "present_mode": "no-vsync",
                       "cap": {"source": "seed", "decoded_bytes_per_drain": 4194304},
                       "outcome": "complete", "gpu_clock": "unrecorded"},
            "rows": rows, "life_states": []}


def reduce_synthetic(trace, rows, budget_ms=R.DEFAULT_BUDGET_MS, ioreg=None, toc=None):
    with tempfile.TemporaryDirectory() as tmp:
        trace.write(tmp)
        path = Path(tmp) / "record.json"
        path.write_text(json.dumps(record(rows)), encoding="utf-8")
        ioreg_path = None
        if ioreg is not None:
            ioreg_path = Path(tmp) / "ioreg.log"
            ioreg_path.write_text(ioreg, encoding="utf-8")
        if toc is not None:
            (Path(tmp) / "toc.xml").write_text(toc, encoding="utf-8")
        return R.reduce_paths(path, tmp, ioreg_path, budget_ms=budget_ms)


def steady_trace(frames, drains=None):
    trace = Trace()
    drains = drains or {}
    for k in frames:
        start = k * FRAME_NS
        trace.frame(k, start, drains.get(k, ()))
    return trace


def lm(k, duration=300_000, offset=1_000_000):
    return ("lightmap", [(k * FRAME_NS + offset, duration, "SDMA0")])


def sh(k, duration=500_000, offset=1_500_000):
    return ("sh", [(k * FRAME_NS + offset, duration, "GFX")])


class JoinErrorTests(unittest.TestCase):
    def test_a_record_with_no_matching_frame_marks_is_an_error(self):
        trace = steady_trace(range(100, 106))
        rows = [row(k) for k in range(0, 6)]
        with self.assertRaises(R.ReduceError) as caught:
            reduce_synthetic(trace, rows)
        self.assertIn("no record frame matches a frame mark", str(caught.exception))

    def test_the_cli_exits_non_zero_naming_the_missing_join(self):
        trace = steady_trace(range(100, 106))
        with tempfile.TemporaryDirectory() as tmp:
            trace.write(tmp)
            path = Path(tmp) / "record.json"
            path.write_text(json.dumps(record([row(k) for k in range(6)])), encoding="utf-8")
            code, out, err = run_cli("--record", path, "--trace-dir", tmp)
        self.assertNotEqual(code, 0)
        self.assertEqual(out, "")
        self.assertIn("no record frame matches a frame mark", err)

    def test_a_trace_with_no_marks_is_an_error(self):
        trace = Trace()
        for k in range(4):
            trace.encoder(k, "frame", k * FRAME_NS, [(k * FRAME_NS + 10, 10, "GFX")], marked=False)
        with self.assertRaises(R.ReduceError) as caught:
            reduce_synthetic(trace, [row(k) for k in range(4)])
        self.assertIn("no `pr-frame` mark", str(caught.exception))


class MalformedRecordTests(unittest.TestCase):
    def assert_rejected(self, record_value, *expected):
        with self.assertRaises(R.ReduceError) as caught:
            R.reduce_run(record_value, [], {})
        for text in expected:
            self.assertIn(text, str(caught.exception))

    def test_a_malformed_record_names_the_row_and_field(self):
        good = row(0)
        self.assert_rejected([good], "top level is not a JSON object")
        self.assert_rejected({"header": [], "rows": [good]}, "`header` is not an object")
        self.assert_rejected({"rows": {"0": good}}, "`rows` is not a list")
        self.assert_rejected(record([good, "row"]), "record row 1 is not an object")
        no_frame = dict(row(1))
        del no_frame["frame"]
        self.assert_rejected(record([good, no_frame]), "record row 1", "`frame`")
        self.assert_rejected(record([good, dict(row(1), frame="1")]), "record row 1", "`frame`")
        self.assert_rejected(record([good, dict(row(1), frame=True)]), "record row 1", "`frame`")
        self.assert_rejected(record([good, row(0)]), "record row 1", "repeats")
        self.assert_rejected(record([good, row(1, interval_us="16 ms")]),
                             "record row 1 (frame 1)", "`interval_us` is not a number")
        self.assert_rejected(record([good, dict(row(1), drain=[])]),
                             "record row 1 (frame 1)", "`drain` is not an object")
        self.assert_rejected(record([good, row(1, sh_admitted_items="3")]),
                             "`drain.sh_admitted_items` is not an integer")
        self.assert_rejected(record([good, dict(row(1), changes="vsync")]),
                             "`changes` is not a list of strings")
        for field in ("cap", "machine"):
            bad = record([good])
            bad["header"][field] = "seed"
            self.assert_rejected(bad, f"header's `{field}` is not an object")

    def test_the_cli_exits_2_on_a_malformed_or_unparseable_record(self):
        trace = steady_trace(range(4))
        for name, text in (("not json", "{\"rows\": [ "),
                           ("bad row", json.dumps(record([row(0), {"frame": None}]))),
                           ("top-level list", json.dumps([row(0)])),
                           ("cap not an object",
                            json.dumps(dict(record([row(0)]), header={"cap": 4194304}))),
                           ("machine not an object",
                            json.dumps(dict(record([row(0)]), header={"machine": ["m"]})))):
            with self.subTest(name), tempfile.TemporaryDirectory() as tmp:
                trace.write(tmp)
                path = Path(tmp) / "record.json"
                path.write_text(text, encoding="utf-8")
                code, out, err = run_cli("--record", path, "--trace-dir", tmp)
                self.assertEqual(code, 2)
                self.assertEqual(out, "")
                self.assertIn("playback_trace_reduce: error:", err)
                self.assertNotIn("Traceback", err)


class AttributionTests(unittest.TestCase):
    def test_a_drain_runs_late_yet_stays_on_the_frame_that_recorded_it(self):
        trace = Trace()
        for k in range(0, 10):
            trace.frame(k, k * FRAME_NS)
        # Frame 10 records a lightmap drain before its frame encoder, but the
        # GPU runs it after frame 11's encoding and GPU work have begun.
        f10, f11 = 10 * FRAME_NS, 11 * FRAME_NS
        trace.encoder(10, "lightmap", f10, [(f11 + 6_500_000, 400_000, "SDMA0"),
                                            (f11 + 6_700_000, 600_000, "GFX")])
        trace.encoder(10, "frame", f10 + 300_000, [(f10 + 4_000_000, 2_000_000, "GFX")])
        trace.frame(11, f11)  # GPU at f11 + 4 ms, before frame 10's drain
        trace.frame(12, 12 * FRAME_NS, [sh(12)])
        rows = [row(k) for k in range(13)]
        summary = reduce_synthetic(trace, rows)
        frames = by_frame(summary)
        self.assertAlmostEqual(gpu_ms(frames[10], "lightmap"), 0.8)  # union of 6.5-6.9, 6.7-7.3
        self.assertEqual(frames[11]["drains"]["total"]["state"], "none")
        self.assertEqual(frames[9]["drains"]["total"]["state"], "none")
        self.assertAlmostEqual(gpu_ms(frames[12], "sh"), 0.5)
        self.assertEqual(frames[12]["drains"]["lightmap"]["state"], "none")

    def test_nested_and_coalesced_rows_count_once(self):
        trace = Trace()
        for k in range(3):
            trace.frame(k, k * FRAME_NS)
        base = 3 * FRAME_NS
        trace.encoder(3, "sh", base, [(base + 1_000_000, 1_000_000, "GFX"),
                                      (base + 1_200_000, 300_000, "GFX"),   # nested
                                      (base + 1_000_000, 1_000_000, "SDMA0"),  # coalesced
                                      (base + 2_500_000, 500_000, "SDMA0")])
        trace.encoder(3, "lightmap", base + 100_000, [(base + 1_800_000, 400_000, "GFX")])
        trace.encoder(3, "frame", base + 300_000, [(base + 4_000_000, 1_000_000, "GFX")])
        summary = reduce_synthetic(trace, [row(k) for k in range(4)])
        f = by_frame(summary)[3]
        self.assertAlmostEqual(gpu_ms(f, "sh"), 1.5)
        self.assertAlmostEqual(gpu_ms(f, "lightmap"), 0.4)
        self.assertAlmostEqual(gpu_ms(f, "total"), 1.7)  # 1.0-2.2 and 2.5-3.0

    def test_a_command_buffer_mark_counts_for_coverage_never_as_a_drain_encoder(self):
        trace = steady_trace(range(6))
        base = 6 * FRAME_NS
        # Frame 6's SH command buffer row carries the mark as well as its
        # encoder: the buffer row names no encoder, so it is not a drain
        # encoder with no GPU row.
        trace.encoder(6, "sh", base, [(base + 1_000_000, 500_000, "GFX")], buffer_mark=True)
        trace.encoder(6, "frame", base + 300_000, [(base + 4_000_000, 1_000_000, "GFX")])
        # Frame 7's frame encoder is marked only on its command buffer row.
        base = 7 * FRAME_NS
        trace.encoder(7, "frame", base, [(base + 4_000_000, 1_000_000, "GFX")],
                      marked=False, buffer_mark=True)
        trace.frame(8, 8 * FRAME_NS)
        summary = reduce_synthetic(trace, [row(k) for k in range(9)])
        frames = by_frame(summary)
        self.assertEqual(frames[6]["drains"]["sh"]["state"], "measured")
        self.assertEqual(frames[6]["drains"]["sh"]["encoders"], 1)
        self.assertAlmostEqual(gpu_ms(frames[6], "sh"), 0.5)
        self.assertTrue(frames[7]["matched"], "a buffer-row mark places the frame encoder")
        self.assertTrue(frames[8]["matched"], "and covers the next frame")
        self.assertEqual(frames[7]["drains"]["total"]["state"], "none")

    def test_every_encoder_of_a_drain_command_buffer_counts_toward_the_drain(self):
        # Frame 5's lightmap buffer holds its marked encoder (5F + 1 ms, 0.3
        # ms) and a second, unmarked one wgpu split off, which runs late:
        # 5F + 30 ms to 5F + 30.2 ms = 113.533 ms, past S(6) = 100 ms. Frame
        # 6's frame encoder buffer also holds an unmarked render pass.
        trace = steady_trace(range(0, 5))
        base = 5 * FRAME_NS
        trace.encoder(5, "lightmap", base, [(base + 1_000_000, 300_000, "SDMA0")],
                      unmarked_gpu=[(base + 30_000_000, 200_000, "SDMA0")])
        trace.encoder(5, "frame", base + 200_000, [(base + 4_000_000, 1_000_000, "GFX")])
        base = 6 * FRAME_NS
        trace.encoder(6, "frame", base, [(base + 4_000_000, 1_000_000, "GFX")],
                      unmarked_gpu=[(base + 5_000_000, 3_000_000, "GFX")])
        for k in range(7, 10):
            trace.frame(k, k * FRAME_NS)
        rows = [row(k) for k in range(10)]
        rows[7]["interval_us"] = 25_000
        summary = reduce_synthetic(trace, rows)
        frames = by_frame(summary)
        lightmap = frames[5]["drains"]["lightmap"]
        self.assertEqual((lightmap["state"], lightmap["encoders"]), ("measured", 1))
        self.assertAlmostEqual(gpu_ms(frames[5], "lightmap"), 0.5)
        self.assertEqual(frames[6]["drains"]["total"]["state"], "none",
                         "a frame encoder's unmarked passes are never a drain")
        # Row 7: U = min(earliest(6) = 100.0, earliest(7) - 25 = 91.667) ms.
        # The split-off encoder ends after U, so frame 5's whole drain was in
        # flight; its marked encoder alone ends at 84.633 ms, inside (L, U].
        charge = {c["frame"]: c for c in summary["over_budget_charge"]["frames"]}[7]
        self.assertEqual(charge["in_flight_frames"], [5])
        self.assertEqual(charge["in_flight_possible_frames"], [])
        self.assertAlmostEqual(charge["own_plus_in_flight_ms"], 0.5)

    def test_a_drain_reads_missing_only_when_its_command_buffer_has_no_gpu_row(self):
        # The second encoder's rows alone reach the trace: the buffer has GPU
        # rows, so the drain is measured from them.
        trace = steady_trace(range(0, 3))
        base = 3 * FRAME_NS
        trace.encoder(3, "sh", base, unmarked_gpu=[(base + 1_000_000, 400_000, "GFX")])
        trace.encoder(3, "frame", base + 200_000, [(base + 4_000_000, 1_000_000, "GFX")])
        # Frame 4's SH buffer has no GPU row at all.
        trace.frame(4, 4 * FRAME_NS, [("sh", [])])
        trace.frame(5, 5 * FRAME_NS)
        summary = reduce_synthetic(trace, [row(k) for k in range(6)])
        frames = by_frame(summary)
        self.assertAlmostEqual(gpu_ms(frames[3], "sh"), 0.4)
        self.assertEqual(frames[4]["drains"]["sh"]["state"], "missing")
        self.assertIn("1 marked command buffer(s) with no GPU row",
                      frames[4]["drains"]["sh"]["reason"])

    def test_only_encoding_rows_bound_a_frames_start(self):
        # Frame 6's frame encoder buffer also has a 30 ms row of another event
        # type. It is not encoding, so it never stretches latest(6): row 8's
        # L stays at latest(6) = 6F + 0.05 ms = 100.05 ms.
        trace = steady_trace(range(0, 6))
        base = 6 * FRAME_NS
        encoder = trace.encoder(6, "frame", base, [(base + 4_000_000, 1_000_000, "GFX")])
        trace.buffer_event(encoder, base + 500, 30_000_000, "Scheduling")
        for k in range(7, 10):
            trace.frame(k, k * FRAME_NS)
        rows = [row(k) for k in range(10)]
        rows[8]["interval_us"] = 25_000
        summary = reduce_synthetic(trace, rows)
        charge = {c["frame"]: c for c in summary["over_budget_charge"]["frames"]}[8]
        self.assertAlmostEqual(charge["start_lower_ns"] / 1e6, 100.05, places=5)


class PartialTraceTests(unittest.TestCase):
    def setUp(self):
        # Record: hold 0-4, route 5-16. Trace: marks 3-12 only.
        drains = {6: [lm(6)], 8: [sh(8)], 10: [lm(10), sh(10)]}
        trace = steady_trace(range(3, 13), drains)
        # Frame 9 recorded a drain whose GPU rows the trace never got.
        trace.encoder(9, "sh", 9 * FRAME_NS - 1_000_000)
        rows = [row(k, "hold") for k in range(5)]
        rows += [row(k) for k in range(5, 17)]
        by = {r["frame"]: r for r in rows}
        by[14]["drain"]["lightmap_admitted_items"] = 3  # an unmatched frame that drained
        by[11]["drain"]["lightmap_admitted_items"] = 2  # matched, but no encoder in the trace
        self.summary = reduce_synthetic(trace, rows)
        self.frames = by_frame(self.summary)

    def test_figures_cover_matched_route_frames_and_name_the_unmatched(self):
        join = self.summary["join"]
        self.assertEqual(join["route_rows"], 12)
        self.assertEqual(join["route_matched"], 8)
        self.assertEqual(join["route_unmatched"], 4)
        self.assertEqual(join["route_unmatched_frames"], "13-16")
        self.assertEqual(self.summary["route"]["rows"], 8)
        self.assertIsNotNone(self.summary["route_record_wide"])
        self.assertEqual(self.summary["route_record_wide"]["rows"], 12)

    def test_hold_frames_before_the_trace_are_not_an_error(self):
        self.assertEqual(self.summary["join"]["hold_unmatched"], 4)  # 0-2 absent, 3 first marked
        self.assertTrue(self.frames[4]["matched"])

    def test_an_unmatched_or_unrun_drain_is_never_zero(self):
        self.assertIsNone(self.frames[14]["drains"], "unmatched frame carries no drain figure")
        self.assertEqual(self.frames[9]["drains"]["sh"]["state"], "missing")
        self.assertIsNone(self.frames[9]["drains"]["total"]["gpu_us"])
        self.assertEqual(self.frames[11]["drains"]["lightmap"]["state"], "missing")
        d = self.summary["drain_gpu"]
        self.assertEqual(d["missing"], {"lightmap": 1, "sh": 1, "total": 2})
        self.assertEqual(d["resources"]["lightmap"]["n"], 2)  # 6 and 10
        self.assertEqual(d["resources"]["sh"]["n"], 2)        # 8 and 10
        self.assertEqual(d["resources"]["total"]["n"], 3)
        self.assertAlmostEqual(d["resources"]["total"]["p50"], 0.5)
        self.assertAlmostEqual(d["resources"]["total"]["max"], 0.8)

    def test_text_names_the_unmatched_route_frames(self):
        text = R.render_text(self.summary)
        self.assertIn("route frames: 12; matched 8; unmatched 4 (frames 13-16)", text)


class RouteOnlyFigureTests(unittest.TestCase):
    def test_interval_figures_cover_unmarked_route_frames_only(self):
        trace = steady_trace(range(0, 14), {7: [lm(7)], 11: [sh(11)]})
        rows = [row(0, "hold", None), row(1, "hold", 90_000), row(2, "hold", 80_000)]
        rows += [row(3, interval_us=10_000), row(4, interval_us=12_000),
                 row(5, interval_us=20_000), row(6, "no_draw", 70_000),
                 row(7, interval_us=14_000), row(8, interval_us=60_000, gap=True),
                 row(9, interval_us=11_000)]
        # A vsync chord lands on frame 10 and marks every row to the end.
        rows += [row(k, interval_us=50_000, changes=["vsync"]) for k in (10, 11, 12)]
        rows += [row(13, interval_us=40_000, changes=["vsync", "extents"])]
        summary = reduce_synthetic(trace, rows)
        route = summary["route"]
        self.assertEqual((route["rows"], route["n"]), (5, 5))
        self.assertAlmostEqual(route["p50"], 12.0)
        self.assertAlmostEqual(route["p95"], 20.0)
        self.assertAlmostEqual(route["max"], 20.0)
        self.assertEqual(route["over_budget"], 1)
        apart = summary["apart"]
        # Frame 0, the record's first row, matches without a predecessor and
        # has no interval.
        self.assertEqual((apart["hold"]["rows"], apart["hold"]["matched"], apart["hold"]["n"]),
                         (3, 3, 2))
        self.assertAlmostEqual(apart["hold"]["max"], 90.0)
        self.assertEqual(apart["no_draw"]["rows"], 1)
        self.assertAlmostEqual(apart["no_draw"]["max"], 70.0)
        self.assertEqual(apart["marked"]["rows"], 5)
        self.assertEqual(apart["marked"]["changes"], {"vsync": 4, "extents": 1})
        self.assertEqual(apart["marked"]["gaps"], 1)
        self.assertAlmostEqual(apart["marked"]["max"], 60.0)
        self.assertEqual(apart["marked"]["drain_gpu"]["n"], 1)  # frame 11's SH drain
        # Drains on marked rows leave the drain figures too.
        self.assertEqual(summary["drain_gpu"]["resources"]["lightmap"]["n"], 1)
        self.assertEqual(summary["drain_gpu"]["resources"]["sh"]["n"], 0)

    def test_a_no_draw_frame_matches_and_its_drain_is_reported_apart(self):
        # Frame 5 streams and drains, then presents nothing: no frame encoder.
        trace = steady_trace([k for k in range(0, 9) if k != 5], {7: [lm(7)]})
        trace.encoder(5, "sh", 5 * FRAME_NS, [(5 * FRAME_NS + 1_000_000, 700_000, "GFX")])
        rows = [row(k) for k in range(9)]
        rows[5]["phase"] = "no_draw"
        rows[5]["interval_us"] = 40_000
        summary = reduce_synthetic(trace, rows)
        frames = by_frame(summary)
        self.assertTrue(frames[5]["matched"])
        self.assertTrue(frames[6]["matched"], "the frame after a no-draw row is covered")
        self.assertAlmostEqual(gpu_ms(frames[5], "sh"), 0.7)
        no_draw = summary["apart"]["no_draw"]
        self.assertEqual((no_draw["rows"], no_draw["matched"]), (1, 1))
        self.assertAlmostEqual(no_draw["max"], 40.0)
        self.assertEqual(no_draw["drain_gpu"]["n"], 1)
        self.assertAlmostEqual(no_draw["drain_gpu"]["max"], 0.7)
        self.assertEqual(summary["route"]["rows"], 8)
        self.assertEqual(summary["drain_gpu"]["resources"]["sh"]["n"], 0)
        self.assertEqual(summary["drain_gpu"]["resources"]["lightmap"]["n"], 1)

    def test_the_first_record_row_matches_without_a_predecessor(self):
        trace = steady_trace(range(0, 4))
        summary = reduce_synthetic(trace, [row(0, "hold", None)] + [row(k) for k in range(1, 4)])
        self.assertTrue(by_frame(summary)[0]["matched"])


class DrainKindTests(unittest.TestCase):
    def setUp(self):
        drains = {2: [lm(2, 100_000)], 3: [lm(3, 900_000)], 4: [sh(4, 600_000)],
                  5: [lm(5, 2_000_000)], 6: [lm(6, 300_000), sh(6, 200_000)],
                  7: [sh(7, 400_000)]}
        trace = steady_trace(range(0, 8), drains)
        rows = [row(0, "hold"), row(1), row(2), row(3, cold_lightmap=True),
                row(4, cold_sh=True), row(5, lightmap_pool_grew=True),
                row(6, cold_lightmap=True, mask_pool_grew=True), row(7)]
        rows[7].pop("drain")  # a row without drain columns: unknown, never warm
        self.summary = reduce_synthetic(trace, rows)

    def test_cold_and_pool_growth_drains_are_counted_apart_from_warm(self):
        cats = self.summary["drain_gpu"]["categories"]
        total = cats["total"]
        self.assertEqual(total["warm"]["n"], 1)      # 2
        self.assertEqual(total["cold"]["n"], 3)      # 3, 4, 6
        self.assertEqual(total["growth"]["n"], 2)    # 5, 6
        self.assertEqual(total["unknown"]["n"], 1)   # 7
        self.assertAlmostEqual(total["warm"]["max"], 0.1)
        self.assertAlmostEqual(total["growth"]["max"], 2.0)
        lightmap = cats["lightmap"]
        self.assertEqual((lightmap["warm"]["n"], lightmap["cold"]["n"], lightmap["growth"]["n"]),
                         (1, 2, 2))
        sh_cats = cats["sh"]
        self.assertNotIn("growth", sh_cats, "SH has no pool-growth flag")
        self.assertEqual((sh_cats["warm"]["n"], sh_cats["cold"]["n"], sh_cats["unknown"]["n"]),
                         (1, 1, 1))  # 6 (its SH pool stayed warm), 4, 7

    def test_per_frame_flags_follow_the_record(self):
        frames = by_frame(self.summary)
        self.assertEqual((frames[3]["cold"], frames[3]["pool_grew"]), (True, False))
        self.assertEqual((frames[5]["cold"], frames[5]["pool_grew"]), (False, True))
        self.assertEqual((frames[7]["cold"], frames[7]["pool_grew"]), (None, None))
        text = R.render_text(self.summary)
        line7 = next(l for l in text.splitlines() if l.strip().startswith("7 "))
        self.assertIn("unknown", line7)


class OverBudgetTests(unittest.TestCase):
    """F = FRAME_NS. Trace.frame encodes frame k's drains from kF, 0.2 ms
    apart, then its frame encoder; each command buffer row lasts 0.05 ms. So
    earliest(k) = kF and latest(k) = kF + 0.05 ms + 0.2 ms per drain. Row k
    bounds S(k - 1) by L = max(latest(k - 2), latest(k - 1) - interval_k -
    1 µs) and U = min(earliest(k - 1), earliest(k) - interval_k)."""

    def charges(self, summary):
        return {f["frame"]: f for f in summary["over_budget_charge"]["frames"]}

    def test_own_is_the_previous_frames_drain_and_in_flight_takes_earlier_ones(self):
        trace = Trace()
        for k in range(0, 5):
            trace.frame(k, k * FRAME_NS)
        # Frame 5's drain runs 30 ms, from 5F + 1 ms to 5F + 31 ms (114.333 ms).
        trace.frame(5, 5 * FRAME_NS, [("lightmap", [(5 * FRAME_NS + 1_000_000, 30_000_000,
                                                     "SDMA0")])])
        trace.frame(6, 6 * FRAME_NS)
        trace.frame(7, 7 * FRAME_NS, [sh(7, 1_000_000)])  # 7F + 1.5 ms to 7F + 2.5 ms (119.167)
        for k in range(8, 12):
            trace.frame(k, k * FRAME_NS)
        rows = [row(k) for k in range(12)]
        for k in (6, 7, 8, 9, 11):
            rows[k]["interval_us"] = 25_000
        summary = reduce_synthetic(trace, rows)
        charge = self.charges(summary)
        self.assertEqual(sorted(charge), [6, 7, 8, 9, 11])
        # 6: frame 5 opened its interval, so 5's drain is own.
        self.assertEqual((charge[6]["own_frame"], charge[6]["own_state"]), (5, "measured"))
        self.assertAlmostEqual(charge[6]["own_ms"], 30.0)
        self.assertEqual(charge[6]["in_flight_frames"], [])
        self.assertAlmostEqual(charge[6]["own_plus_in_flight_ms"], 30.0)
        # 7: frame 6 drained nothing. U = min(earliest(6) = 100.0,
        # earliest(7) - 25 = 91.667) = 91.667 ms; 5's drain ends after U.
        self.assertEqual(charge[7]["own_state"], "none")
        self.assertAlmostEqual(charge[7]["own_ms"], 0.0)
        self.assertAlmostEqual(charge[7]["start_upper_ns"] / 1e6, 91.666667, places=5)
        self.assertEqual(charge[7]["in_flight_frames"], [5])
        self.assertAlmostEqual(charge[7]["own_plus_in_flight_ms"], 30.0)
        # 8: own is 7's SH drain; U = min(116.667, 133.333 - 25) = 108.333 ms,
        # and 5's drain still runs after it.
        self.assertAlmostEqual(charge[8]["own_ms"], 1.0)
        self.assertEqual(charge[8]["in_flight_frames"], [5])
        self.assertAlmostEqual(charge[8]["own_plus_in_flight_ms"], 31.0)
        # 9: 8 drained nothing. L = latest(7) = 116.917 ms, U = 150 - 25 =
        # 125 ms. 5's drain ended at 114.333, before L; 7's at 119.167, between
        # L and U: it may have been in flight, so the reading is missing.
        self.assertAlmostEqual(charge[9]["start_lower_ns"] / 1e6, 116.916667, places=5)
        self.assertAlmostEqual(charge[9]["start_upper_ns"] / 1e6, 125.0, places=5)
        self.assertAlmostEqual(charge[9]["own_ms"], 0.0)
        self.assertEqual(charge[9]["in_flight_frames"], [])
        self.assertEqual(charge[9]["in_flight_possible_frames"], [7])
        self.assertIn("frame 7's drain may have been in flight", charge[9]["in_flight_reason"])
        self.assertIsNone(charge[9]["own_plus_in_flight_ms"])
        # 11: L = latest(9) = 150.05 ms; every earlier drain ended before it.
        self.assertEqual(charge[11]["in_flight_possible_frames"], [])
        self.assertAlmostEqual(charge[11]["own_plus_in_flight_ms"], 0.0)
        c = summary["over_budget_charge"]
        self.assertEqual((c["with_own_drain"], c["with_own_or_in_flight_drain"],
                          c["with_possible_in_flight_drain"], c["with_no_drain"],
                          c["own_missing"], c["in_flight_missing"]),
                         (2, 3, 1, 1, 0, 1))
        text = R.render_text(summary)
        self.assertIn("possibly 7", text)

    def test_a_late_frame_encoder_does_not_hide_an_in_flight_drain(self):
        # Frame 6 encodes its SH drain 0.5 ms after its start, but its frame
        # encoder only 9 ms after (as acquire under vsync can make it), and so
        # does frame 7. Frame 5's lightmap drain runs to 5F + 21 ms = 104.333
        # ms, past S(6) = 100 ms.
        trace = Trace()
        for k in range(0, 5):
            trace.frame(k, k * FRAME_NS)
        trace.frame(5, 5 * FRAME_NS, [("lightmap", [(5 * FRAME_NS + 1_000_000, 20_000_000,
                                                     "SDMA0")])])
        f6, f7 = 6 * FRAME_NS, 7 * FRAME_NS
        trace.encoder(6, "sh", f6 + 500_000, [(f6 + 5_000_000, 400_000, "GFX")])
        trace.encoder(6, "frame", f6 + 9_000_000, [(f6 + 12_000_000, 1_000_000, "GFX")])
        trace.encoder(7, "frame", f7 + 9_000_000, [(f7 + 12_000_000, 1_000_000, "GFX")])
        trace.frame(8, 8 * FRAME_NS)
        rows = [row(k, interval_us=16_666) for k in range(9)]
        summary = reduce_synthetic(trace, rows, budget_ms=10.0)
        charge = self.charges(summary)[7]
        # U comes from frame 6's earliest row (100.5 ms), not its late frame
        # encoder: U = min(earliest(6) = 100.5, earliest(7) - 16.666 = 109.0),
        # so the drain ending at 104.333 ms is in flight.
        self.assertAlmostEqual(charge["start_upper_ns"] / 1e6, 100.5, places=5)
        self.assertAlmostEqual(charge["own_ms"], 0.4)
        self.assertEqual(charge["in_flight_frames"], [5])
        self.assertAlmostEqual(charge["own_plus_in_flight_ms"], 20.4)

    def test_one_unknown_drain_end_names_every_reading_it_blanks(self):
        trace = steady_trace(range(0, 12))
        # Frame 3's SH encoder never got a GPU row: its end is unknown, so it
        # may still be in flight for every later window.
        trace.encoder(3, "sh", 3 * FRAME_NS - 1_000_000)
        rows = [row(k) for k in range(12)]
        for k in range(6, 11):
            rows[k]["interval_us"] = 25_000
        summary = reduce_synthetic(trace, rows)
        c = summary["over_budget_charge"]
        self.assertEqual(c["in_flight_missing"], 5)
        self.assertEqual(c["with_possible_in_flight_drain"], 5)
        self.assertEqual(c["with_own_or_in_flight_drain"], 0, "a possible drain is not known")
        self.assertEqual(c["in_flight_blanked_by"], [{
            "cause": "end_unknown", "frame": 3, "text": "frame 3's drain end is unknown",
            "rows": 5, "row_frames": "6-10"}])
        text = R.render_text(summary)
        self.assertIn("frame 3's drain end is unknown: 5 row(s) (6-10)", text)

    def test_an_unmatched_frame_that_admitted_items_blanks_later_in_flight_readings(self):
        # Every row of frame 5 is missing from the trace, though the record
        # says it admitted SH items: 5 is unmatched and 6 uncovered. Row 9's
        # window starts in the covered stretch from 7, after frame 5, but
        # frame 5's drain may still be on the GPU.
        trace = steady_trace([k for k in range(0, 12) if k != 5])
        rows = [row(k) for k in range(12)]
        rows[5]["drain"]["sh_admitted_items"] = 3
        rows[9]["interval_us"] = 25_000
        summary = reduce_synthetic(trace, rows)
        frames = by_frame(summary)
        self.assertFalse(frames[5]["matched"])
        charge = {c["frame"]: c for c in summary["over_budget_charge"]["frames"]}[9]
        self.assertEqual(charge["in_flight_partial_frames"], [5])
        self.assertIsNone(charge["own_plus_in_flight_ms"])
        self.assertEqual(summary["over_budget_charge"]["in_flight_blanked_by"], [{
            "cause": "end_unknown", "frame": 5, "text": "frame 5's drain end is unknown",
            "rows": 1, "row_frames": "9"}])

    def test_own_reads_a_hold_rows_drain(self):
        trace = steady_trace(range(0, 7), {3: [lm(3)]})
        rows = [row(k, "hold") for k in range(4)] + [row(k) for k in range(4, 7)]
        rows[4]["interval_us"] = 25_000
        charge = self.charges(reduce_synthetic(trace, rows))
        self.assertEqual(charge[4]["own_frame"], 3)
        self.assertAlmostEqual(charge[4]["own_ms"], 0.3)

    def test_own_reads_missing_when_the_previous_drain_is_missing(self):
        trace = steady_trace(range(0, 4))
        trace.frame(4, 4 * FRAME_NS, [("sh", [])])  # an SH encoder with no GPU row
        trace.frame(5, 5 * FRAME_NS)
        trace.frame(6, 6 * FRAME_NS)
        rows = [row(k) for k in range(7)]
        rows[5]["interval_us"] = 25_000
        summary = reduce_synthetic(trace, rows)
        charge = self.charges(summary)
        self.assertEqual(charge[5]["own_state"], "missing")
        self.assertIsNone(charge[5]["own_ms"])
        self.assertIn("frame 4's drain is missing", charge[5]["own_reason"])
        self.assertIsNone(charge[5]["own_plus_in_flight_ms"])
        self.assertEqual(summary["over_budget_charge"]["own_missing"], 1)
        text = R.render_text(summary)
        line = next(l for l in text.splitlines() if l.strip().startswith("5 "))
        self.assertIn("missing", line)

    def test_own_reads_missing_when_the_previous_frame_is_unmatched_or_has_no_row(self):
        # The trace attaches at frame 3, so 3 is uncovered and unmatched; 4 is
        # matched, but the frame that opened its interval is not.
        trace = steady_trace(range(3, 9))
        rows = [row(k, "hold") for k in range(3)] + [row(k) for k in range(3, 9)]
        rows[4]["interval_us"] = 25_000
        charge = self.charges(reduce_synthetic(trace, rows))
        self.assertEqual(charge[4]["own_state"], "missing")
        self.assertIn("frame 3 is not matched", charge[4]["own_reason"])
        self.assertIsNotNone(charge[4]["in_flight_reason"], "nothing before 4 is fully seen")
        self.assertIsNone(charge[4]["own_plus_in_flight_ms"])
        # The record starts at 4: frame 3 has no row to read.
        charge = self.charges(reduce_synthetic(trace, rows[4:]))
        self.assertEqual(charge[4]["own_state"], "missing")
        self.assertIn("frame 3 has no record row", charge[4]["own_reason"])

    def test_in_flight_reads_missing_when_an_in_flight_drain_is_partial(self):
        trace = steady_trace(range(0, 4))
        # Frame 4 holds two SH encoders: one runs long, the other never ran.
        base = 4 * FRAME_NS
        trace.encoder(4, "sh", base, [(base + 1_000_000, 40_000_000, "GFX")])
        trace.encoder(4, "sh", base + 100_000)
        trace.encoder(4, "frame", base + 300_000, [(base + 4_000_000, 1_000_000, "GFX")])
        for k in (5, 6, 7):
            trace.frame(k, k * FRAME_NS)
        rows = [row(k) for k in range(8)]
        rows[6]["interval_us"] = 25_000
        rows[7]["interval_us"] = 25_000
        summary = reduce_synthetic(trace, rows)
        charge = self.charges(summary)
        for k in (6, 7):
            self.assertAlmostEqual(charge[k]["own_ms"], 0.0, msg=k)
            self.assertEqual(charge[k]["in_flight_frames"], [], k)
            self.assertEqual(charge[k]["in_flight_partial_frames"], [4], k)
            self.assertIn("frame 4's drain end is unknown", charge[k]["in_flight_reason"])
            self.assertIsNone(charge[k]["own_plus_in_flight_ms"], k)
        c = summary["over_budget_charge"]
        self.assertEqual((c["in_flight_missing"], c["with_no_drain"]), (2, 0))
        text = R.render_text(summary)
        self.assertIn("partial 4", text)

    def test_in_flight_reads_missing_when_an_unmatched_frames_drain_is_in_flight(self):
        # The trace attaches at frame 3, whose long drain is in the trace but
        # whose other drains may not be: 3 is unmatched.
        trace = Trace()
        trace.frame(3, 3 * FRAME_NS, [("lightmap", [(3 * FRAME_NS + 1_000_000, 40_000_000,
                                                     "SDMA0")])])
        for k in range(4, 9):
            trace.frame(k, k * FRAME_NS)
        rows = [row(k) for k in range(9)]
        rows[5]["interval_us"] = 25_000
        charge = self.charges(reduce_synthetic(trace, rows))
        self.assertAlmostEqual(charge[5]["own_ms"], 0.0)
        self.assertEqual(charge[5]["in_flight_partial_frames"], [3])
        self.assertIsNone(charge[5]["own_plus_in_flight_ms"])


class SiteTagTests(unittest.TestCase):
    """wgpu's Metal backend relabels the last command buffer of every submit
    "(wgpu internal) Signal", so a drain's buffer label can be lost. The
    current mark, `pr-frame <k> <site>`, carries the site itself."""

    def test_the_mark_parses_with_and_without_a_site(self):
        for text, frame, site in (("Blit Command 0  -  :pr-frame 12 lightmap  ( Main", 12,
                                   "lightmap"),
                                  ("Blit Command 0  -  :pr-frame 12 sh  ( Main", 12, "sh"),
                                  ("Blit Command 0  -  :pr-frame 3 frame  ( Main", 3, "frame"),
                                  ("Blit Command 0  -  :pr-frame 1136  ( Main", 1136, None),
                                  # A site is its whole token: never a known
                                  # site's prefix.
                                  ("Blit Command 0  -  :pr-frame 7 shadow", 7, "shadow"),
                                  ("Blit Command 0  -  :pr-frame 7 sh-mover  ( Main", 7,
                                   "sh-mover"),
                                  ("Blit Command 0  -  :pr-frame 7 lightmap.v2", 7,
                                   "lightmap.v2")):
            match = R.MARK.search(text)
            self.assertEqual((int(match.group(1)), match.group(2)), (frame, site), text)

    def test_a_site_tagged_mark_on_a_signal_buffer_counts_toward_its_drain(self):
        trace = steady_trace(range(0, 5), {})
        base = 5 * FRAME_NS
        trace.encoder(5, "lightmap", base, [(base + 1_000_000, 300_000, "SDMA0")], site=True,
                      buffer_label=R.SIGNAL_LABEL)
        trace.encoder(5, "sh", base + 100_000, [(base + 2_000_000, 500_000, "GFX")], site=True,
                      buffer_label=R.SIGNAL_LABEL)
        # The site wins over a buffer label naming another kind.
        trace.encoder(5, "sh", base + 200_000, [(base + 3_000_000, 200_000, "GFX")], site=True,
                      buffer_label=COMMAND_BUFFER_LABEL["lightmap"])
        trace.encoder(5, "frame", base + 300_000, [(base + 4_000_000, 1_000_000, "GFX")],
                      site=True, buffer_label=R.SIGNAL_LABEL)
        trace.frame(6, 6 * FRAME_NS)
        summary = reduce_synthetic(trace, [row(k) for k in range(7)])
        f = by_frame(summary)[5]
        self.assertTrue(f["matched"], "a site-tagged frame mark places the frame encoder")
        self.assertIsNone(f["unattributed"])
        self.assertAlmostEqual(gpu_ms(f, "lightmap"), 0.3)
        self.assertAlmostEqual(gpu_ms(f, "sh"), 0.7)
        self.assertEqual(f["drains"]["sh"]["encoders"], 2)
        self.assertAlmostEqual(gpu_ms(f, "total"), 1.0)
        self.assertEqual(summary["unattributed"]["frames"], 0)
        self.assertEqual(summary["trace"]["marked_encoders"][R.OTHER], 0)

    def test_an_unknown_site_is_counted_apart_and_never_blanks_a_drain(self):
        # A newer engine's site this reducer does not know, even one whose
        # name starts with a known site's.
        trace = steady_trace(range(0, 5), {})
        base = 5 * FRAME_NS
        trace.encoder(5, "sh", base, [(base + 1_000_000, 500_000, "GFX")])
        for offset, site in ((100_000, "sh-mover"), (150_000, "lightmap.v2")):
            trace.encoder(5, "sh", base + offset, [(base + 2_000_000, 300_000, "GFX")],
                          site=site, buffer_label=R.SIGNAL_LABEL)
        trace.encoder(5, "frame", base + 300_000, [(base + 4_000_000, 1_000_000, "GFX")])
        trace.frame(6, 6 * FRAME_NS)
        summary = reduce_synthetic(trace, [row(k) for k in range(7)])
        f = by_frame(summary)[5]
        self.assertTrue(f["matched"])
        self.assertIsNone(f["unattributed"])
        self.assertEqual(f["drains"]["sh"]["state"], "measured")
        self.assertEqual(f["drains"]["sh"]["encoders"], 1)
        self.assertAlmostEqual(gpu_ms(f, "sh"), 0.5)
        trace_block = summary["trace"]
        self.assertEqual(trace_block["unknown_site_marks"], 2)
        self.assertEqual(trace_block["unknown_sites"], {"sh-mover": 1, "lightmap.v2": 1})
        self.assertEqual(trace_block["marked_encoders"][R.SH], 1)
        self.assertIn("marks naming an unknown site: 2 (lightmap.v2 1, sh-mover 1)",
                      R.render_text(summary))

    def test_an_old_mark_on_a_signal_buffer_makes_the_drain_missing_not_partial(self):
        trace = steady_trace(range(0, 5), {})
        base = 5 * FRAME_NS
        trace.encoder(5, "lightmap", base, [(base + 1_000_000, 300_000, "SDMA0"),
                                            (base + 1_200_000, 400_000, "GFX")],
                      buffer_label=R.SIGNAL_LABEL, site=False)
        trace.encoder(5, "sh", base + 100_000, [(base + 2_000_000, 500_000, "GFX")], site=False)
        trace.encoder(5, "frame", base + 300_000, [(base + 4_000_000, 1_000_000, "GFX")],
                      site=False)
        trace.frame(6, 6 * FRAME_NS)
        trace.frame(7, 7 * FRAME_NS)
        rows = [row(k) for k in range(8)]
        rows[6]["interval_us"] = 25_000
        summary = reduce_synthetic(trace, rows)
        f = by_frame(summary)[5]
        for resource in ("lightmap", "sh", "total"):
            entry = f["drains"][resource]
            self.assertEqual(entry["state"], "missing", resource)
            self.assertIsNone(entry["gpu_us"], resource)
            self.assertIn(R.UNATTRIBUTED_REASON, entry["reason"], resource)
        self.assertEqual(f["drains"]["total"]["causes"], ["unattributed"])
        self.assertAlmostEqual(f["unattributed"]["gpu_us"], 600.0)  # 1.0-1.6 ms
        self.assertEqual((f["unattributed"]["encoders"], f["unattributed"]["relabelled"]), (1, 1))
        u = summary["unattributed"]
        self.assertEqual((u["frames"], u["frame_list"]), (1, "5"))
        self.assertAlmostEqual(u["gpu_ms_max"], 0.6)
        d = summary["drain_gpu"]
        self.assertEqual(d["missing"], {"lightmap": 1, "sh": 1, "total": 1})
        self.assertEqual(d["resources"]["sh"]["n"], 0, "the labelled SH alone is not the drain")
        charge = {c["frame"]: c for c in summary["over_budget_charge"]["frames"]}
        self.assertEqual(charge[6]["own_state"], "missing")
        self.assertIn(R.UNATTRIBUTED_REASON, charge[6]["own_reason"])
        text = R.render_text(summary)
        self.assertIn("unattributed 1", text)
        self.assertIn('1 on a "(wgpu internal) Signal" command buffer', text)
        line = next(l for l in text.splitlines() if l.strip().startswith("5 "))
        self.assertIn("0.600", line)

    def test_a_marked_unattributed_drain_with_a_known_end_is_not_in_flight_after_it(self):
        # Frame 2's unattributed work ends at 2F + 1.6 ms, long before frame
        # 6 starts: it never blanks row 7's in-flight reading.
        trace = steady_trace(range(0, 2), {})
        base = 2 * FRAME_NS
        trace.encoder(2, "lightmap", base, [(base + 1_000_000, 600_000, "SDMA0")],
                      buffer_label=R.SIGNAL_LABEL, site=False)
        trace.encoder(2, "frame", base + 300_000, [(base + 4_000_000, 1_000_000, "GFX")])
        for k in range(3, 9):
            trace.frame(k, k * FRAME_NS)
        rows = [row(k) for k in range(9)]
        rows[7]["interval_us"] = 25_000
        charge = {c["frame"]: c for c in reduce_synthetic(trace, rows)["over_budget_charge"]
                  ["frames"]}
        self.assertIsNone(charge[7]["in_flight_reason"])
        self.assertAlmostEqual(charge[7]["own_plus_in_flight_ms"], 0.0)


class ClockTests(unittest.TestCase):
    TOC = ('<?xml version="1.0"?><trace-toc><run number="1"><info><summary>'
           '<start-date>2026-11-19T17:29:30.000-08:00</start-date>'
           '</summary></info></run></trace-toc>')
    START = 1795138170  # 2026-11-20T01:29:30Z

    def summary(self, ioreg, toc=True):
        trace = steady_trace(range(0, 30), {10: [lm(10)]})
        rows = [row(k) for k in range(30)]
        return reduce_synthetic(trace, rows, ioreg=ioreg, toc=self.TOC if toc else None)

    def poll(self, t, clocks):
        return f"{t}\n" + "".join(f'"Device Utilization %"=50\n"Core Clock(MHz)"={c}\n'
                                  f'"Temperature(C)"=80\n' for c in clocks)

    def test_polls_join_the_route_span_by_time(self):
        # The route spans trace 0-0.5 s: epoch START .. START + 0.5.
        ioreg = (self.poll(f"{self.START - 5}.250", [100]) + self.poll(f"{self.START}.100", [700])
                 + self.poll(f"{self.START}.400", [900, 0]) + self.poll(f"{self.START + 9}.0", [50]))
        clock = self.summary(ioreg)["gpu_clock"]
        self.assertEqual(clock["state"], "recorded")
        self.assertEqual((clock["polls"], clock["readings"]), (2, 2))
        self.assertAlmostEqual(clock["median_mhz"], 800)
        self.assertEqual((clock["min_mhz"], clock["max_mhz"]), (700, 900))

    def test_whole_second_poll_stamps_widen_the_span(self):
        # The span opens a microsecond after START; a whole-second stamp of
        # START may have been taken anywhere in that second.
        ioreg = (self.poll(str(self.START - 2), [10]) + self.poll(str(self.START), [600])
                 + self.poll(f"{self.START}.N", [640]) + self.poll(str(self.START + 3), [10]))
        clock = self.summary(ioreg)["gpu_clock"]
        self.assertEqual(clock["polls"], 2)
        self.assertAlmostEqual(clock["median_mhz"], 620)

    def test_zero_or_missing_readings_read_unrecorded_never_pinned(self):
        ioreg = self.poll(f"{self.START}.200", [0, 0]) + f"{self.START}.300\n"
        summary = self.summary(ioreg)
        clock = summary["gpu_clock"]
        self.assertEqual(clock["state"], "unrecorded")
        self.assertNotIn("median_mhz", clock)
        text = R.render_text(summary)
        self.assertNotIn("MHz", text)
        self.assertEqual(text.count("GPU clock: unrecorded"), 4)

    def test_unaligned_time_bases_report_the_whole_log_labelled(self):
        ioreg = self.poll("12.5", [300]) + self.poll("13.0", [500])
        clock = self.summary(ioreg, toc=False)["gpu_clock"]
        self.assertAlmostEqual(clock["median_mhz"], 400)
        self.assertIn("whole poll log (time bases not aligned", clock["text"])

    def test_an_unparseable_toc_start_date_is_quoted(self):
        toc = self.TOC.replace("2026-11-19T17:29:30.000-08:00", "yesterday at noon")
        trace = steady_trace(range(0, 30), {10: [lm(10)]})
        summary = reduce_synthetic(trace, [row(k) for k in range(30)],
                                   ioreg=self.poll("12.5", [300]), toc=toc)
        clock = summary["gpu_clock"]
        self.assertIn("whole poll log (time bases not aligned", clock["text"])
        self.assertIn("start date `yesterday at noon` did not parse", clock["text"])
        self.assertNotIn("no trace table of contents", clock["text"])

    def test_start_dates_normalize_for_older_pythons(self):
        for text, expected in (("2026-11-19T17:29:30.000-08:00", self.START),
                               ("2026-11-20T01:29:30Z", self.START),
                               ("2026-11-20T01:29:30.25Z", self.START + 0.25),
                               ("2026-11-20T01:29:30.1234567+00:00", self.START + 0.123456),
                               ("2026-11-19T17:29:30-0800", self.START)):
            self.assertAlmostEqual(R.parse_start_date(text), expected, places=6, msg=text)
        for text in ("2026-11-20T01:29:30", "yesterday", "2026-13-40T01:29:30Z"):
            self.assertIsNone(R.parse_start_date(text), text)

    def test_a_record_naming_an_unrecorded_clock_and_no_poll_reads_unrecorded(self):
        clock = self.summary(None)["gpu_clock"]
        self.assertEqual(clock["state"], "unrecorded")
        self.assertIn("the record names the clock `unrecorded`", clock["text"])


if __name__ == "__main__":
    unittest.main()
