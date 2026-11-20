"""Tests for the playback trace reducer (`tools/playback_trace_reduce.py`).

The checked-in sample (`fixtures/playback_trace/`) is trimmed from one real
playback run: the Mac checkpoint of `camera-path-playback` (branch
`mac/camera-path-checkpoint` 159f47d64; AMD Radeon Pro 5300M, Metal,
campaign-test far-point, hold 900, vsync). It keeps:

* `metal-application-intervals.xml` and `metal-gpu-intervals.xml`: the marked
  encoder rows, their command buffer rows and the GPU rows joined to them for
  frames 724-727 (hold), 1133-1141 and 1246-1250, plus a few unrelated rows.
  Rows are the export's own lines; a value another row defined is inlined at
  its first use, so `id`/`ref` de-duplication stays as xctrace writes it.
* `record.playback.json`: the run's record (the minimal Task 4 schema: no
  drain flags, marks or CPU columns), its rows sliced to 720-727, 1134-1141
  and 1247-1250. Frames 720-723 precede the trace (it attached mid-hold).
* `toc.xml`: the export's table of contents, cut to its summary.
* `ioreg.log`: the clock poll from trace time 6.0 s to 10.5 s.

The expected values were checked by hand against the raw rows. Cold, growth,
change and gap marks, partial traces and late drains are covered by synthetic
cases that follow the current record schema
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
        self.assertAlmostEqual(gpu_ms(f[1136], "sh"), 1.239487, places=6)  # 4 command buffers
        self.assertAlmostEqual(gpu_ms(f[1137], "sh"), 1.883889, places=6)  # 8 rows, no overlap
        self.assertAlmostEqual(gpu_ms(f[1138], "sh"), 1.943563, places=6)  # SDMA0/GFX overlap
        self.assertAlmostEqual(gpu_ms(f[1139], "sh"), 0.019994, places=6)
        self.assertAlmostEqual(gpu_ms(f[1248], "lightmap"), 0.421549, places=6)
        self.assertEqual(f[1136]["drains"]["sh"]["encoders"], 4)
        self.assertEqual(f[1138]["drains"]["sh"]["encoders"], 3)
        self.assertAlmostEqual(gpu_ms(f[1248], "total"), 0.421549, places=6)
        for frame in (1134, 1135, 1140, 1141, 1247, 1249, 1250, 725):
            self.assertEqual(f[frame]["drains"]["total"]["state"], "none", frame)
        self.assertEqual(f[1136]["drains"]["lightmap"]["state"], "none")

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
        self.assertEqual(d["sh"]["n"], 4)
        self.assertAlmostEqual(d["sh"]["p50"], 1.239487, places=6)
        self.assertAlmostEqual(d["sh"]["max"], 1.943563, places=6)
        self.assertEqual(d["total"]["n"], 5)
        cats = self.summary["drain_gpu"]["categories"]["total"]
        self.assertEqual(cats["unknown"]["n"], 5, "a minimal record's drains are unknown")
        self.assertEqual((cats["warm"]["n"], cats["cold"]["n"], cats["growth"]["n"]), (0, 0, 0))

    def test_over_budget_charge_reports_both_readings(self):
        charge = self.summary["over_budget_charge"]
        self.assertEqual(charge["over_budget"], 5)
        rows = {f["frame"]: f for f in charge["frames"]}
        self.assertEqual(sorted(rows), [1135, 1137, 1139, 1140, 1250])
        self.assertAlmostEqual(rows[1137]["own_ms"], 1.883889, places=6)
        self.assertEqual(rows[1137]["in_flight_frames"], [1136])
        self.assertAlmostEqual(rows[1137]["own_plus_in_flight_ms"], 1.883889 + 1.239487, places=6)
        # 1137's drain ended before 1139's window opened; 1138's had not.
        self.assertEqual(rows[1139]["in_flight_frames"], [1138])
        self.assertAlmostEqual(rows[1140]["own_ms"], 0.0)
        self.assertEqual(rows[1140]["in_flight_frames"], [1139])
        self.assertEqual(rows[1250]["in_flight_frames"], [])
        self.assertEqual((charge["with_own_drain"], charge["with_own_or_in_flight_drain"],
                          charge["with_no_drain"]), (2, 3, 2))

    def test_clock_joins_the_route_span_by_time(self):
        clock = self.summary["gpu_clock"]
        self.assertEqual(clock["state"], "recorded")
        # Polls 178.571, 179.127, 179.682 and 180.239 fall in the span.
        self.assertEqual((clock["polls"], clock["readings"]), (4, 16))
        self.assertEqual(clock["median_mhz"], 902.5)
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

    def encoder(self, frame, kind, cpu_ns, gpu=(), marked=True):
        """One command buffer with one encoder; `gpu` holds (start, duration, channel)."""
        cmdbuffer = self.next_cmdbuffer
        encoder = cmdbuffer + 1
        self.next_cmdbuffer += 2
        label = COMMAND_BUFFER_LABEL[kind]
        self.app.append((cpu_ns, 50_000, f"{label}  (Frame  {frame % 7} ) ( postretro (1) )",
                         34, cmdbuffer, R.ALL_ONES))
        mark = f"  -  :pr-frame {frame}" if marked else ""
        self.app.append((cpu_ns + 1_000, 30_000,
                         f"Blit Command 0{mark}  ( Main Thread (0x1) (postretro, pid: 1) )",
                         35, cmdbuffer, encoder))
        for start, duration, channel in gpu:
            self.gpu.append((start, duration, channel, cmdbuffer, encoder,
                             f"{label}:Blit Command 0  ( postretro (1) )  0x{encoder:x}"))
        return encoder

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
        for start, dur, label, depth, cmdbuffer, encoder in self.app:
            rows.append(
                "<row>"
                f'<start-time id="{ids.new()}" fmt="t">{start}</start-time>'
                f'<duration id="{ids.new()}" fmt="d">{dur}</duration>'
                + ids.process()
                + ids.value("metal-nesting-level", str(depth), str(depth))
                + ids.value("formatted-label", label, "")
                + ids.value("metal-event-name", "Encoding", "Encoding")
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


class AttributionTests(unittest.TestCase):
    def test_p6_a_drain_runs_late_yet_stays_on_the_frame_that_recorded_it(self):
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

    def test_p7_figures_cover_matched_route_frames_and_name_the_unmatched(self):
        join = self.summary["join"]
        self.assertEqual(join["route_rows"], 12)
        self.assertEqual(join["route_matched"], 8)
        self.assertEqual(join["route_unmatched"], 4)
        self.assertEqual(join["route_unmatched_frames"], "13-16")
        self.assertEqual(self.summary["route"]["rows"], 8)
        self.assertIsNotNone(self.summary["route_record_wide"])
        self.assertEqual(self.summary["route_record_wide"]["rows"], 12)

    def test_p7_hold_frames_before_the_trace_are_not_an_error(self):
        self.assertEqual(self.summary["join"]["hold_unmatched"], 4)  # 0-2 absent, 3 first marked
        self.assertTrue(self.frames[4]["matched"])

    def test_p7_an_unmatched_or_unrun_drain_is_never_zero(self):
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
    def test_p8_p10_interval_figures_cover_unmarked_route_frames_only(self):
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
        self.assertEqual(route["p50"], 12.0)
        self.assertEqual(route["p95"], 20.0)
        self.assertEqual(route["max"], 20.0)
        self.assertEqual(route["over_budget"], 1)
        apart = summary["apart"]
        # Frame 0, the record's first row, matches without a predecessor and
        # has no interval.
        self.assertEqual((apart["hold"]["rows"], apart["hold"]["matched"], apart["hold"]["n"],
                          apart["hold"]["max"]), (3, 3, 2, 90.0))
        self.assertEqual((apart["no_draw"]["rows"], apart["no_draw"]["max"]), (1, 70.0))
        self.assertEqual(apart["marked"]["rows"], 5)
        self.assertEqual(apart["marked"]["changes"], {"vsync": 4, "extents": 1})
        self.assertEqual(apart["marked"]["gaps"], 1)
        self.assertEqual(apart["marked"]["max"], 60.0)
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
        self.assertEqual((no_draw["rows"], no_draw["matched"], no_draw["max"]), (1, 1, 40.0))
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
    def test_in_flight_charge_takes_earlier_drains_still_on_the_gpu(self):
        trace = Trace()
        for k in range(0, 5):
            trace.frame(k, k * FRAME_NS)
        # Frame 5's drain runs long, past frame 6's start and into frame 7's window.
        trace.frame(5, 5 * FRAME_NS, [("lightmap", [(5 * FRAME_NS + 1_000_000, 30_000_000,
                                                     "SDMA0")])])
        trace.frame(6, 6 * FRAME_NS)
        trace.frame(7, 7 * FRAME_NS, [sh(7, 1_000_000)])
        trace.frame(8, 8 * FRAME_NS)
        trace.frame(9, 9 * FRAME_NS)
        rows = [row(k) for k in range(10)]
        for k in (6, 7, 9):
            rows[k]["interval_us"] = 25_000
        summary = reduce_synthetic(trace, rows)
        charge = {f["frame"]: f for f in summary["over_budget_charge"]["frames"]}
        self.assertEqual(sorted(charge), [6, 7, 9])
        self.assertEqual(charge[6]["own_ms"], 0.0)
        self.assertEqual(charge[6]["in_flight_frames"], [5])
        self.assertAlmostEqual(charge[6]["own_plus_in_flight_ms"], 30.0)
        self.assertAlmostEqual(charge[7]["own_ms"], 1.0)
        self.assertEqual(charge[7]["in_flight_frames"], [5])  # ends at 5F + 31 ms > 7F - 25 ms
        self.assertAlmostEqual(charge[7]["own_plus_in_flight_ms"], 31.0)
        self.assertEqual(charge[9]["in_flight_frames"], [], "7's drain ended before 9's window")
        c = summary["over_budget_charge"]
        self.assertEqual((c["with_own_drain"], c["with_own_or_in_flight_drain"], c["with_no_drain"]),
                         (1, 2, 1))


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
        self.assertEqual((clock["median_mhz"], clock["min_mhz"], clock["max_mhz"]), (800, 700, 900))

    def test_integer_second_stamps_from_bsd_date_widen_the_span(self):
        # The span opens a microsecond after START; a whole-second stamp of
        # START may have been taken anywhere in that second.
        ioreg = (self.poll(str(self.START - 2), [10]) + self.poll(str(self.START), [600])
                 + self.poll(f"{self.START}.N", [640]) + self.poll(str(self.START + 3), [10]))
        clock = self.summary(ioreg)["gpu_clock"]
        self.assertEqual((clock["polls"], clock["median_mhz"]), (2, 620))

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
        self.assertEqual(clock["median_mhz"], 400)
        self.assertIn("whole poll log (time bases not aligned", clock["text"])

    def test_a_record_naming_no_clock_and_no_poll_reads_unrecorded(self):
        clock = self.summary(None)["gpu_clock"]
        self.assertEqual(clock["state"], "unrecorded")
        self.assertIn("unrecorded", clock["text"])


if __name__ == "__main__":
    unittest.main()
