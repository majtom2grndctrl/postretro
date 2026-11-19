"""Tests for the telemetry reducer against the checked-in two-run sample.

The sample (`crates/telemetry/tests/fixtures/two_run_sample.jsonl`) holds a
`prl-build` bake of stress-warren, then an engine run that loads its output,
changes level across a cancelled generation, and unloads a third level before
its reveal. The telemetry crate's own tests parse every line of it.
"""

import contextlib
import importlib.util
import io
import json
import unittest
from pathlib import Path

REPO = Path(__file__).parents[2]
SAMPLE = REPO / "crates" / "telemetry" / "tests" / "fixtures" / "two_run_sample.jsonl"

BAKE_RUN = "18f2a3b4c5d-1a2b"
ENGINE_RUN = "18f2a3b9f10-1a40"
MAP = "/work/game/maps/stress-warren.map"
PRL = "/work/game/maps/stress-warren.prl"
PRL2 = "/work/game/maps/arena-loop.prl"
PRL3 = "/work/game/maps/hub.prl"


def load_reducer_module():
    script = Path(__file__).parents[1] / "telemetry_reduce.py"
    spec = importlib.util.spec_from_file_location("telemetry_reduce_test", script)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


REDUCER = load_reducer_module()


def envelope(run, seq, kind, payload, source="engine", level=None, t_ms=0):
    return json.dumps({
        "v": 1, "run": run, "seq": seq, "t_ms": t_ms, "source": source,
        "level": level, "kind": kind, "payload": payload,
    })


def run_start(run, source="engine", producers=()):
    return envelope(run, 0, "run_start", {
        "epoch_ms": 0, "argv": [source], "version": "0.1.0", "sink": None,
        "producers": list(producers),
    }, source=source)


def phases_of(table):
    return [(p["name"], p["ms"], [(m["name"], m["ms"]) for m in p["marks"]]) for p in table]


def by_label(rows):
    return {row["label"]: row for row in rows}


class SampleSummaryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        with SAMPLE.open(encoding="utf-8") as handle:
            cls.summary = REDUCER.reduce_lines(handle)
        cls.runs = {run["run"]: run for run in cls.summary["runs"]}
        cls.loads = {(l["run"], l["generation"]): l for l in cls.summary["loads"]}

    def test_every_line_parses(self):
        self.assertEqual(self.summary["lines"], 45)
        self.assertEqual(self.summary["malformed"], 0)
        self.assertEqual(self.summary["malformed_lines"], [])
        self.assertEqual(self.summary["unknown_kinds"], {})

    def test_runs_in_file_order_with_sources_and_clean_ends(self):
        self.assertEqual([r["run"] for r in self.summary["runs"]], [BAKE_RUN, ENGINE_RUN])
        bake, engine = self.runs[BAKE_RUN], self.runs[ENGINE_RUN]
        self.assertEqual(bake["source"], "prl-build")
        self.assertEqual(engine["source"], "engine")
        for run in (bake, engine):
            self.assertEqual(run["version"], "0.1.0")
            self.assertEqual(run["end"], {"reason": "clean", "detail": None})
            self.assertEqual(run["seq_gaps"], 0)
        self.assertEqual(bake["argv"], ["prl-build", MAP, "-o", PRL])
        self.assertEqual(engine["argv"], ["postretro", PRL])

    def test_producer_state_settles_gpu_timing_pending_to_enabled(self):
        with SAMPLE.open(encoding="utf-8") as handle:
            starts = [json.loads(line) for line in handle if '"kind":"run_start"' in line]
        (engine_start,) = [s for s in starts if s["run"] == ENGINE_RUN]
        announced = {p["producer"]: p["state"] for p in engine_start["payload"]["producers"]}
        self.assertEqual(announced["gpu_timing"], "pending")

        producers = self.runs[ENGINE_RUN]["producers"]
        self.assertEqual(list(producers), [
            "load_phases", "geometry_install", "sh_streaming", "lightmap_streaming",
            "cpu_timing", "gpu_timing", "log",
        ])
        self.assertEqual(producers["gpu_timing"],
                         {"state": "enabled", "variable": "POSTRETRO_GPU_TIMING", "reason": None})
        self.assertEqual(producers["cpu_timing"],
                         {"state": "enabled", "variable": "POSTRETRO_CPU_TIMING", "reason": None})
        self.assertNotIn("pending", {p["state"] for p in producers.values()})

    def test_log_counts_per_level(self):
        self.assertEqual(self.runs[BAKE_RUN]["log_counts"], {"warn": 1})
        self.assertEqual(self.runs[ENGINE_RUN]["log_counts"], {"info": 3, "warn": 1})

    def test_bake_durations_come_from_the_summary(self):
        (bake,) = self.summary["bakes"]
        self.assertEqual(bake["run"], BAKE_RUN)
        self.assertEqual(bake["status"], "ok")
        self.assertTrue(bake["success"])
        self.assertEqual(bake["source"], MAP)
        self.assertEqual(bake["output"], PRL)
        self.assertEqual([(s["name"], s["ms"]) for s in bake["stages"]], [
            ("Parsing", 41.2), ("Partitioning", 312.5), ("Visibility", 88.0),
            ("NavMesh", 0.4), ("SH Bake", 2410.7), ("Lightmap Bake", 5120.9),
            ("Packing", 205.1),
        ])
        self.assertEqual([(s["name"], s["ms"]) for s in bake["non_stage"]],
                         [("  TexelSteps", 840.2), ("SH bound", 12.6),
                          ("Lightmap bound", 9.4)])
        self.assertEqual(bake["total_ms"], 5778.3)
        self.assertEqual(bake["skipped"], ["NavMesh"])
        self.assertEqual(bake["background"], ["Lightmap Bake"])

    def test_first_load_phase_table(self):
        load = self.loads[(ENGINE_RUN, 1)]
        self.assertEqual(load["status"], "revealed")
        self.assertEqual(load["prl"], PRL)
        self.assertEqual(phases_of(load["phases"]), [
            ("load_dispatch", 0.8, []),
            ("bundle_ready", 412.6, [("prl_parse", 120.3), ("upload", 182.4),
                                     ("textures", 96.0), ("sh_plan", 34.2)]),
            ("commit_frame", 58.4, [("commit_checks", 1.2), ("set_publish", 3.1),
                                    ("classname_dispatch", 9.8), ("model_install", 22.6),
                                    ("audio_load", 17.4), ("streaming_sessions", 4.0),
                                    ("enter_settling", 0.3)]),
            ("settle_hold", 210.0, []),
            ("reveal", 16.7, []),
            ("first_level_frame", 15.9, []),
        ])
        self.assertEqual(load["geometry_install"], {
            "has_geometry": True,
            "total_ms": 182.4,
            "phases": [
                {"name": "buffers_and_sh_streaming", "ms": 61.0},
                {"name": "sdf_atlas_and_compose", "ms": 48.2},
                {"name": "lightmap_and_sdf_shadow", "ms": 39.5},
                {"name": "bvh_and_cull", "ms": 27.1},
            ],
            "other_ms": 6.6,
        })
        self.assertEqual(load["unload_ms"], 24.3)

    def test_second_revealed_load_phase_table_has_no_bake(self):
        load = self.loads[(ENGINE_RUN, 3)]
        self.assertEqual(load["status"], "revealed")
        self.assertEqual(load["prl"], PRL2)
        self.assertEqual(phases_of(load["phases"]), [
            ("load_dispatch", 0.5, []),
            ("bundle_ready", 288.1, [("prl_parse", 71.6), ("upload", 66.0), ("textures", 40.3)]),
            ("commit_frame", 31.2, [("commit_checks", 0.9), ("set_publish", 1.8),
                                    ("model_install", 14.1), ("audio_load", 11.6),
                                    ("streaming_sessions", 2.6), ("enter_settling", 0.2)]),
            ("settle_hold", 96.4, []),
            ("reveal", 16.6, []),
            ("first_level_frame", 16.1, []),
        ])
        self.assertEqual(load["geometry_install"]["total_ms"], 66.0)
        self.assertEqual(load["unload_ms"], 11.8)
        self.assertIsNone(load["bake"])

    def test_engine_load_joins_its_bake(self):
        bake = self.loads[(ENGINE_RUN, 1)]["bake"]
        self.assertEqual(bake["run"], BAKE_RUN)
        self.assertEqual(bake["source"], MAP)
        self.assertTrue(bake["success"])
        self.assertEqual(bake["stages"][4], {"name": "SH Bake", "ms": 2410.7})
        self.assertEqual(len(bake["stages"]), 7)

    def test_level_unloaded_before_reveal(self):
        load = self.loads[(ENGINE_RUN, 4)]
        self.assertEqual(load["status"], "unloaded before reveal")
        self.assertEqual(load["prl"], PRL3)
        self.assertIsNone(load["phases"])
        self.assertEqual(load["geometry_install"]["total_ms"], 82.9)
        self.assertEqual(load["unload_ms"], 9.6)

    def test_cancelled_generation_is_a_gap(self):
        self.assertEqual(sorted(self.loads), [(ENGINE_RUN, 1), (ENGINE_RUN, 3), (ENGINE_RUN, 4)])
        self.assertEqual(self.summary["generation_gaps"],
                         [{"run": ENGINE_RUN, "generation": 2, "status": "cancelled"}])

    def test_sh_miss_buckets_per_load_include_the_partial_window(self):
        sh = self.loads[(ENGINE_RUN, 1)]["windows"]["sh"]
        # Two full windows plus the partial written at unload.
        self.assertEqual(sh, {
            "windows": 3, "partial_windows": 1,
            "misses": 14 + 4 + 3,
            "miss_reach": 6 + 1,
            "miss_pressure": 2,
            "miss_in_flight": 5 + 2,
            "miss_budget": 2 + 1,
            "miss_compose": 1,
            "miss_failed": 0,
            "miss_device_limit": 0,
            "miss_map_cap": 1,
        })
        sh3 = self.loads[(ENGINE_RUN, 3)]["windows"]["sh"]
        self.assertEqual((sh3["windows"], sh3["partial_windows"], sh3["misses"],
                          sh3["miss_reach"], sh3["miss_device_limit"]), (1, 0, 5, 3, 2))
        self.assertEqual(self.loads[(ENGINE_RUN, 4)]["windows"]["sh"]["windows"], 0)

    def test_lightmap_miss_buckets_per_load(self):
        lightmap = self.loads[(ENGINE_RUN, 1)]["windows"]["lightmap"]
        self.assertEqual(lightmap, {
            "windows": 2, "partial_windows": 1,
            "missed_outside_baked_set": 3,
            "missed_not_resident": 7 + 2,
        })
        self.assertEqual(self.loads[(ENGINE_RUN, 3)]["windows"]["lightmap"]["windows"], 0)

    def test_cpu_p50_and_max_across_windows(self):
        for cpu in (self.runs[ENGINE_RUN]["cpu"], self.loads[(ENGINE_RUN, 1)]["windows"]["cpu"]):
            self.assertEqual(cpu["windows"], 2)
            stages = by_label(cpu["stages"])
            # Count rows carry no milliseconds.
            self.assertNotIn("ticks", stages)
            render = stages["render"]
            self.assertEqual(render["windows"], 2)
            self.assertAlmostEqual(render["p50_average_ms"], 2.9)
            self.assertEqual(render["max_average_ms"], 3.0)
            self.assertEqual(render["max_ms"], 7.0)
            total = stages["total"]
            self.assertTrue(total["aggregate"])
            self.assertAlmostEqual(total["p50_average_ms"], 7.2)
            self.assertEqual(total["max_ms"], 14.2)
            self.assertEqual(stages["render_submit"]["parent"], "render")
        self.assertEqual(self.loads[(ENGINE_RUN, 3)]["windows"]["cpu"]["windows"], 0)

    def test_gpu_null_mean_is_absent_not_zero(self):
        for gpu in (self.runs[ENGINE_RUN]["gpu"], self.loads[(ENGINE_RUN, 1)]["windows"]["gpu"]):
            self.assertEqual(gpu["windows"], 2)
            passes = by_label(gpu["passes"])
            compose = passes["sh_compose"]
            self.assertEqual(compose["windows"], 1)
            self.assertEqual(compose["absent_windows"], 1)
            self.assertEqual(compose["p50_mean_ms"], 0.31)
            self.assertEqual(compose["max_mean_ms"], 0.31)
            forward = passes["forward"]
            self.assertEqual((forward["windows"], forward["absent_windows"]), (2, 0))
            self.assertAlmostEqual(forward["p50_mean_ms"], 2.97)
            self.assertEqual(forward["max_mean_ms"], 3.10)

    def test_text_and_json_render_the_summary(self):
        text = REDUCER.render_text(self.summary)
        self.assertIn("gen 4  unloaded before reveal", text)
        self.assertIn("gen 2  cancelled", text)
        self.assertIn(f"baked by {BAKE_RUN}", text)
        self.assertEqual(json.loads(json.dumps(self.summary))["malformed"], 0)

    def test_cli_prints_json(self):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            REDUCER.main([str(SAMPLE), "--json"])
        self.assertEqual(json.loads(out.getvalue())["generation_gaps"][0]["generation"], 2)


class EdgeCaseTests(unittest.TestCase):
    def test_run_without_run_end_reads_killed(self):
        summary = REDUCER.reduce_lines([
            run_start("aa-1"),
            envelope("aa-1", 1, "log", {"level": "info", "target": "t", "message": "m",
                                        "truncated": False}),
        ])
        (run,) = summary["runs"]
        self.assertEqual(run["end"], {"reason": "killed", "detail": None})
        self.assertEqual(run["log_counts"], {"info": 1})

    def test_geometry_install_without_unload_or_phases_is_not_revealed(self):
        summary = REDUCER.reduce_lines([
            run_start("dd-4", producers=[{"producer": "gpu_timing", "state": "pending",
                                          "variable": "POSTRETRO_GPU_TIMING", "reason": None}]),
            envelope("dd-4", 1, "geometry_install", {
                "generation": 1, "has_geometry": True, "total_ms": 5.0,
                "phases": [], "other_ms": 5.0}),
            envelope("dd-4", 2, "producer_state", {
                "producer": "gpu_timing", "state": "unsupported",
                "variable": "POSTRETRO_GPU_TIMING", "reason": "no timestamp queries"}),
        ])
        (load,) = summary["loads"]
        self.assertEqual(load["status"], "not revealed")
        self.assertEqual(summary["runs"][0]["producers"]["gpu_timing"]["reason"],
                         "no timestamp queries")

    def test_failed_bake_reports_stages_begun_and_no_durations(self):
        source = "prl-build"
        summary = REDUCER.reduce_lines([
            run_start("bb-2", source),
            envelope("bb-2", 1, "bake_stage",
                     {"stage": "Parsing", "transition": "begin", "background": False}, source),
            envelope("bb-2", 2, "bake_stage",
                     {"stage": "Parsing", "transition": "finish", "background": False}, source),
            envelope("bb-2", 3, "bake_stage",
                     {"stage": "Partitioning", "transition": "begin", "background": False}, source),
            envelope("bb-2", 4, "bake_summary",
                     {"success": False, "source": "/m/a.map", "output": "/m/a.prl",
                      "timings": None, "total_ms": None}, source),
            envelope("bb-2", 5, "run_end", {"reason": "error", "detail": "leak"}, source),
        ])
        (bake,) = summary["bakes"]
        self.assertEqual(bake["status"], "failed")
        self.assertEqual(bake["stages"], [])
        self.assertEqual(bake["non_stage"], [])
        self.assertIsNone(bake["total_ms"])
        self.assertEqual(bake["began"], ["Parsing", "Partitioning"])
        self.assertEqual(summary["runs"][0]["end"], {"reason": "error", "detail": "leak"})
        self.assertIn("no durations; stages begun: Parsing, Partitioning",
                      REDUCER.render_text(summary))

    def test_malformed_blank_and_unknown_lines(self):
        summary = REDUCER.reduce_lines([
            run_start("cc-3"),
            "",
            "{not json",
            json.dumps({"v": 2, "run": "cc-3", "seq": 1, "t_ms": 0, "source": "engine",
                        "level": None, "kind": "log", "payload": {}}),
            envelope("cc-3", 1, "future_kind", {}),
            envelope("cc-3", 2, "gpu_timing_window", {"readbacks": 120, "passes": [
                {"label": "forward", "mean_ms": 1.5, "sampled_readbacks": 120,
                 "malformed_readbacks": 0}]}),
        ])
        self.assertEqual(summary["lines"], 5)
        self.assertEqual(summary["malformed"], 2)
        self.assertEqual([m["line"] for m in summary["malformed_lines"]], [3, 4])
        self.assertEqual(summary["unknown_kinds"], {"future_kind": 1})
        (run,) = summary["runs"]
        no_level = run["no_level_windows"]
        self.assertEqual(no_level["gpu"]["windows"], 1)
        self.assertEqual(no_level["gpu"]["passes"][0]["p50_mean_ms"], 1.5)
        self.assertEqual(summary["loads"], [])

    def test_a_payload_missing_what_its_kind_needs_is_counted_not_fatal(self):
        summary = REDUCER.reduce_lines([
            run_start("dd-4"),
            envelope("dd-4", 1, "load_phases", {}),
            envelope("dd-4", 2, "load_phases", {"generation": None, "prl": None, "phases": []}),
            envelope("dd-4", 3, "producer_state", {"state": "enabled"}),
            envelope("dd-4", 4, "geometry_install", {"generation": 2, "has_geometry": True,
                                                    "total_ms": 1.0, "phases": [],
                                                    "other_ms": 0.0}),
        ])
        self.assertEqual([m["line"] for m in summary["malformed_lines"]], [2, 3, 4])
        self.assertEqual([load["generation"] for load in summary["loads"]], [2])

    def test_a_half_bad_payload_applies_nothing(self):
        summary = REDUCER.reduce_lines([
            run_start("ee-5"),
            envelope("ee-5", 1, "load_phases", {"generation": 7, "prl": "/m.prl",
                                                "phases": [{"name": "load_dispatch"}]}),
            envelope("ee-5", 2, "cpu_timing_window", {"frames": 120, "stages": [
                {"label": "render", "kind": "time", "frames": 120, "average": "1.2",
                 "max": 2.0}]}),
            envelope("ee-5", 3, "sh_streaming_window", {"session": 1, "partial": False,
                                                       "delta": {"misses": "4"}},
                     level={"gen": True, "map": "/m.prl"}),
        ])
        self.assertEqual(summary["loads"], [], "the half-bad load_phases created no load")
        (run,) = summary["runs"]
        self.assertEqual(run["cpu"]["windows"], 0)
        self.assertEqual(summary["malformed"], 3)


if __name__ == "__main__":
    unittest.main()
