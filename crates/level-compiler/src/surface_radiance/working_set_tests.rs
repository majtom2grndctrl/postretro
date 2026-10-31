// SH base gate against the bake it admits: the handed-off patch set and the measured peak.
// See: context/lib/build_pipeline.md §Compiler pipeline (working-set gate)

use std::path::Path;

use super::bounce_tests::{inward_box, open_floor, room_a_light, two_rooms};
use super::test_scene::{Scene, animated, point, room_lights};
use super::tests::polygon_geometry;
use super::volume_cache_tests::{coloured, lifted_annex};
use super::{ShBaseWorkingSetProjection, plan_sh_base};
use crate::bake_control::BakeControl;
use crate::cache::StageCache;
use crate::direct_sh_bake::{EmitterScene, bounce_embedded_emitters};
use crate::light_namespaces::StaticBakedLights;
use crate::live_bytes::{Report, measured};
use crate::map_data::MapLight;
use crate::sh_bake::{ShConfig, animation_table_bytes, static_light_refs};
use crate::texel_footprint::{EmbeddedEmitters, NO_EMBEDDED_EMITTERS};

/// One scene the SH base bake runs over, with its lights, bounce flags and
/// probe spacing.
pub(super) struct Fixture {
    pub(super) name: &'static str,
    pub(super) scene: Scene,
    pub(super) lights: Vec<MapLight>,
    pub(super) embedded: EmbeddedEmitters,
    pub(super) spacing: f32,
}

impl Fixture {
    fn new(name: &'static str, scene: Scene, lights: Vec<MapLight>) -> Self {
        Self {
            name,
            scene,
            lights,
            embedded: NO_EMBEDDED_EMITTERS.clone(),
            spacing: 1.0,
        }
    }

    /// This fixture with its lights classified as the pipeline classifies
    /// the bounce family, penumbra floor on.
    pub(super) fn classified(mut self) -> Self {
        let static_lights = StaticBakedLights::from_lights(&self.lights);
        self.embedded = bounce_embedded_emitters(
            &EmitterScene {
                static_lights: &static_lights,
                bvh: &self.scene.bvh,
                primitives: &self.scene.primitives,
                geometry: &self.scene.geometry,
            },
            true,
            &BakeControl::unrestricted(),
        );
        self
    }

    /// What the SH base gate predicts for this fixture at `fill`.
    fn predicted(&self, fill: f32) -> ShBaseWorkingSetProjection {
        self.scene.with_ctx(&self.lights, |ctx| {
            plan_sh_base(
                &self.scene.geometry,
                &static_light_refs(ctx),
                animation_table_bytes(ctx.animated_lights, ctx.total_light_count),
                self.spacing,
                fill,
                u64::MAX,
            )
            .expect("priced")
            .1
        })
    }

    fn bake(&self, fill: f32, cache: Option<&StageCache>, threads: usize) -> Report {
        measured(threads, || {
            self.scene.bake_with(
                &self.lights,
                self.spacing,
                fill,
                &self.embedded,
                cache,
                &BakeControl::unrestricted(),
            );
        })
        .1
    }
}

/// The in-memory scenes the surface radiance tests bake, and AC 3's
/// embedded-emitter fixture with its flags.
pub(super) fn scene_fixtures() -> Vec<Fixture> {
    let (embedded_geometry, embedded_lights) =
        crate::embedded_emitter_probe_tests::embedded_bounce_fixture();
    let white = crate::surface_materials::stats::BounceMaterials::new(
        vec![crate::surface_materials::stats::MaterialStats::with_snapped_mean([255; 3])],
        1.0,
    )
    .expect("white materials");
    let unit = 0.0254;
    let corridor = Scene::of(polygon_geometry(&inward_box(
        [0.0, 0.0, 0.0],
        [1920.0 * unit, 128.0 * unit, 128.0 * unit],
    )));
    let mut doubled = room_lights();
    doubled.extend(room_lights().into_iter().map(|mut light| {
        light.origin.y -= 0.4;
        light
    }));
    // Many small lights over a coarse grid: the layers outweigh the volume.
    let many_lights: Vec<MapLight> = (0..48)
        .map(|i| {
            let (x, z) = (0.5 + f64::from(i % 8), 0.5 + f64::from(i / 8));
            point([x, 1.0 + 0.1 * f64::from(i % 3), z], 0.1, 3.0)
        })
        .collect();
    vec![
        Fixture::new("room", Scene::new(), room_lights()),
        Fixture {
            spacing: 2.0,
            ..Fixture::new("room, 48 lights, 2 m", Scene::new(), many_lights)
        },
        Fixture::new("room, doubled lights", Scene::new(), doubled),
        Fixture::new("room, no static light", Scene::new(), Vec::new()),
        Fixture::new("room, one animated", Scene::new(), {
            let mut lights = room_lights();
            lights[1] = animated(lights[1].clone());
            lights
        }),
        Fixture::new(
            "room, coloured",
            coloured([200, 90, 40], 0.8),
            room_lights(),
        ),
        Fixture::new("room, lifted annex", lifted_annex(), room_lights()),
        Fixture::new("two rooms", Scene::of(two_rooms()), vec![room_a_light()]),
        Fixture::new(
            "open floor",
            Scene::of(open_floor()),
            vec![point([0.0, 4.0, 0.0], 0.2, 12.0)],
        ),
        Fixture::new(
            "closed white room",
            Scene::of(polygon_geometry(&inward_box(
                [0.0, 0.0, 0.0],
                [4.0, 3.0, 5.0],
            )))
            .with_materials(white),
            vec![point([1.3, 2.1, 1.7], 0.2, 6.0)],
        ),
        Fixture::new(
            "corridor",
            corridor,
            vec![point([f64::from(64.0 * unit), 1.6, 1.6], 0.1, 160.0 * unit)],
        ),
        Fixture::new(
            "embedded emitters",
            Scene::of(embedded_geometry),
            embedded_lights,
        )
        .classified(),
    ]
}

/// How a measured bake meets the build cache.
#[derive(Clone, Copy, Debug)]
enum CacheMode {
    /// No cache: `--no-cache` and `--release`.
    Cold,
    /// An empty cache: every layer and the volume miss and are written.
    WarmEmpty,
    /// Every layer hits; the gathered volume misses and is written.
    WarmVolumeMiss,
    /// The gathered volume hits.
    WarmVolumeHit,
}

const CACHE_MODES: [CacheMode; 4] = [
    CacheMode::Cold,
    CacheMode::WarmEmpty,
    CacheMode::WarmVolumeMiss,
    CacheMode::WarmVolumeHit,
];

/// One measured configuration's row.
pub(super) struct Row {
    fixture: &'static str,
    fill: f32,
    threads: usize,
    mode: &'static str,
    predicted: u64,
    report: Report,
}

impl Row {
    fn line(&self) -> String {
        let phases: Vec<String> = self
            .report
            .phases
            .iter()
            .map(|phase| format!("{} {}", phase.label, phase.peak))
            .collect();
        format!(
            "{:<24} fill {} x{} {:<16} predicted {:>10} measured {:>10} ({:.2}x) | {}",
            self.fixture,
            self.fill,
            self.threads,
            self.mode,
            self.predicted,
            self.report.peak,
            self.predicted as f64 / self.report.peak.max(1) as f64,
            phases.join(", ")
        )
    }
}

/// Bake `fixture` at `fill` on `threads` workers in `mode`, measured.
fn measure(fixture: &Fixture, fill: f32, threads: usize, mode: CacheMode) -> Report {
    let dir = tempfile::tempdir().expect("cache dir");
    let open = |dir: &Path| StageCache::new(dir).expect("cache");
    match mode {
        CacheMode::Cold => fixture.bake(fill, None, threads),
        CacheMode::WarmEmpty => fixture.bake(fill, Some(&open(dir.path())), threads),
        CacheMode::WarmVolumeMiss | CacheMode::WarmVolumeHit => {
            fixture.bake(fill, Some(&open(dir.path())), threads);
            if matches!(mode, CacheMode::WarmVolumeMiss) {
                let key = fixture.scene.volume_key_with(
                    &fixture.lights,
                    fixture.spacing,
                    fill,
                    &fixture.embedded,
                );
                std::fs::remove_file(dir.path().join(key.as_filename()))
                    .expect("the first warm bake wrote the volume entry");
            }
            fixture.bake(fill, Some(&open(dir.path())), threads)
        }
    }
}

/// Every phase a bake in `mode` passes, in order.
fn expected_phases(mode: CacheMode) -> &'static [&'static str] {
    match mode {
        CacheMode::WarmVolumeHit => &["gate", "plan and probe layout", "volume load"],
        _ => &[
            "gate",
            "plan and probe layout",
            "layers",
            "sum",
            "later bounces",
            "probe gather",
            "volume store",
        ],
    }
}

/// Measure every configuration of `fixtures`: fill 0 and 1, one worker and
/// eight, cold and each warm mode. Returns a row per configuration.
pub(super) fn measure_all(fixtures: &[Fixture]) -> Vec<Row> {
    let mut rows = Vec::new();
    for fixture in fixtures {
        for fill in [0.0, 1.0] {
            let predicted = fixture.predicted(fill).estimated_peak_bytes;
            for threads in [1, 8] {
                for mode in CACHE_MODES {
                    let report = measure(fixture, fill, threads, mode);
                    let labels: Vec<&str> = report.phases.iter().map(|phase| phase.label).collect();
                    assert_eq!(
                        labels,
                        expected_phases(mode),
                        "{} {mode:?}: the ledger saw every phase",
                        fixture.name
                    );
                    rows.push(Row {
                        fixture: fixture.name,
                        fill,
                        threads,
                        mode: match mode {
                            CacheMode::Cold => "cold",
                            CacheMode::WarmEmpty => "warm empty",
                            CacheMode::WarmVolumeMiss => "warm volume miss",
                            CacheMode::WarmVolumeHit => "warm volume hit",
                        },
                        predicted,
                        report,
                    });
                }
            }
        }
    }
    rows
}

/// Assert predicted ≥ measured on every row, printing the table first.
pub(super) fn assert_predicted_covers_measured(rows: &[Row]) {
    for row in rows {
        eprintln!("{}", row.line());
    }
    let short: Vec<String> = rows
        .iter()
        .filter(|row| row.predicted < row.report.peak)
        .map(|row| format!("{}\n    at peak: {:?}", row.line(), row.report.at_peak))
        .collect();
    assert!(
        short.is_empty(),
        "the SH base gate predicts less than the bake holds:\n{}",
        short.join("\n")
    );
}

// AC 31, research P27: on every in-memory fixture the surface radiance
// tests bake, and on AC 3's embedded-emitter fixture, at fill 0 and 1, one
// worker and many, cold and warm (a fresh cache, a volume miss over cached
// layers, and a volume hit), the gate's prediction covers the bake's
// measured peak: every structure the stage allocates for itself, routed
// through the test ledger by capacity.
#[test]
fn the_sh_base_prediction_covers_the_measured_peak_on_every_fixture() {
    let rows = measure_all(&scene_fixtures());
    assert_eq!(rows.len(), 12 * 2 * 2 * 4);
    assert_predicted_covers_measured(&rows);
}

// AC 31, research P27: the same sweep over `soft_shadow_test`, a real map
// with its own tree and validity. Heavy: run on demand.
#[test]
#[ignore = "bakes soft_shadow_test 16 times; run on demand with -- --ignored"]
fn the_sh_base_prediction_covers_the_measured_peak_on_soft_shadow_test() {
    let fixture = crate::fixture_pipeline::load_fixture("soft_shadow_test");
    let lights = fixture.lights.clone();
    let fixture = Fixture::new("soft_shadow_test", Scene::of_fixture(fixture), lights).classified();
    let rows = measure_all(std::slice::from_ref(&fixture));
    assert_predicted_covers_measured(&rows);
}

// AC 31, the handoff half: the bake shades exactly the patches the gate
// counted in each light's reach. A handoff whose reach disagrees with the
// patch set it carries stops the bake before a layer is summed.
#[test]
#[should_panic(expected = "the bake shades the patches the SH base gate priced")]
fn a_handoff_whose_reach_disagrees_with_its_patches_stops_the_bake() {
    let scene = Scene::new();
    let lights = room_lights();
    scene.with_ctx(&lights, |ctx| {
        let (mut handoff, _) = plan_sh_base(
            &scene.geometry,
            &static_light_refs(ctx),
            0,
            1.0,
            0.0,
            u64::MAX,
        )
        .expect("priced");
        handoff.light_reach[0] += 1;
        super::bake_sh_base(
            ctx,
            &ShConfig { probe_spacing: 1.0 },
            handoff,
            &NO_EMBEDDED_EMITTERS,
            None,
            &BakeControl::unrestricted(),
        );
    });
}
