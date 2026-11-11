// The swept-face residue, read as the renderer composes it: the probes just
// past a face the mover sweeps, composed on the CPU from the assembled id-57
// section by the renderer's piecewise rule, against a static-brush reference
// bake with the mover's brushes made static at that position. A
// termini-only build (`light_poses 2`) is composed the same way.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting) ·
// context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use glam::DVec3;
use postretro_level_format::lightmap::f32_to_f16_bits;
use postretro_level_format::mover_probe_channel::{
    MoverLightNamespace, MoverProbeChannelSection, MoverRecord, PoseMark,
};
use postretro_level_format::octahedral::{
    DEFAULT_IRRADIANCE_TILE_BORDER, DEFAULT_IRRADIANCE_TILE_DIMENSION,
};
use postretro_level_format::sh_volume::OctahedralAtlasTexel;

use super::{CONFIG, bake_base, plan_half, with_ctx};
use crate::bc6h::f16_bits_to_f32;
use crate::fixture_pipeline::{fixture_path, load_map};
use crate::mover_probe_channel::fixture_scene::Scene;
use crate::mover_probe_channel::reference_maps::{quake_offset, staticize_at};
use crate::mover_probe_channel::{
    BounceBlocks, MoverBounceRequest, MoverPayloadBars, MoverRecords, ProbeCells, channel_section,
    compact_slots, compact_tile, direct_display_levels, display_levels,
};
use crate::sh_bake::pack_octahedral_irradiance_tile;
use crate::surface_radiance::{DEFAULT_BOUNCE_FILL, ShBase};

/// The bar: three display levels (brief AC 19).
const BAR_LEVELS: f32 = 3.0;

/// Quake map units per engine metre.
const QUAKE_PER_METRE: f64 = 1.0 / 0.0254;

/// `clamp((t − s) / (e − s), 0, 1)`: the renderer's `knots::ramped`.
fn ramped(t: f32, start: f32, end: f32) -> f32 {
    let span = end - start;
    let ramped = if span.is_finite() && span > 0.0 && start.is_finite() {
        (t - start) / span
    } else {
        t
    };
    if ramped.is_finite() {
        ramped.clamp(0.0, 1.0) + 0.0
    } else {
        0.0
    }
}

/// `(t′, pose)` of every knot compose reads: the renderer's
/// `knots::effective_knots`.
fn effective_knots(record: &MoverRecord) -> Vec<(f32, usize)> {
    let last = record.knots.len() - 1;
    let mut knots = vec![(0.0, 0)];
    for (pose, &knot) in record.knots.iter().enumerate().take(last).skip(1) {
        let t = ramped(knot, record.ramp_start, record.ramp_end);
        if t < 1.0 && t > knots[knots.len() - 1].0 {
            knots.push((t, pose));
        }
    }
    knots.push((1.0, last));
    knots
}

/// `(lo pose, hi pose, weight)`: the renderer's `knots::interval`. The
/// termini are always defined; `defined` says whether a mid pose is.
fn interval(
    knots: &[(f32, usize)],
    t: f32,
    defined: impl Fn(usize) -> bool,
) -> (usize, usize, f32) {
    let last = knots.len() - 1;
    if t >= 1.0 {
        return (knots[last].1, knots[last].1, 0.0);
    }
    if t <= 0.0 || t.is_nan() {
        return (knots[0].1, knots[0].1, 0.0);
    }
    let is_defined = |at: usize| at == 0 || at == last || defined(knots[at].1);
    let at = knots.partition_point(|knot| knot.0 <= t) - 1;
    let mut lo = at;
    while !is_defined(lo) {
        lo -= 1;
    }
    if knots[lo].0 >= t {
        return (knots[lo].1, knots[lo].1, 0.0);
    }
    let mut hi = at + 1;
    while !is_defined(hi) {
        hi += 1;
    }
    (
        knots[lo].1,
        knots[hi].1,
        (t - knots[lo].0) / (knots[hi].0 - knots[lo].0),
    )
}

/// WGSL `mix`.
fn mix(a: f32, b: f32, weight: f32) -> f32 {
    a * (1.0 - weight) + b * weight
}

fn rgb(texel: &OctahedralAtlasTexel) -> [f32; 3] {
    [0, 1, 2].map(|c| f16_bits_to_f32(texel.rgba[c]))
}

fn texel([r, g, b]: [f32; 3]) -> OctahedralAtlasTexel {
    OctahedralAtlasTexel {
        rgba: [r, g, b, 1.0].map(f32_to_f16_bits),
    }
}

/// One build of a map: its id-57 section assembled and round-tripped through
/// its bytes, id 34 as the SH base bake hands it on, and its probe grid.
struct Build {
    scene: Scene,
    section: MoverProbeChannelSection,
    base: ShBase,
    slots: Vec<u32>,
    /// Dense probe index → (cell, local bit).
    local: HashMap<usize, (u32, u32)>,
    mover_id: u32,
    knots: Vec<(f32, usize)>,
    pose_count: usize,
}

impl Build {
    fn bake(path: &Path, mover: &str) -> Self {
        let scene = Scene::of(load_map(path));
        let analysed = scene.mover_poses();
        let mover_id = analysed
            .analysed
            .iter()
            .find(|analysed| analysed.label.contains(mover))
            .unwrap_or_else(|| panic!("no mover {mover}"))
            .mover_id;
        let cells = ProbeCells::of_layout(&scene.layout);
        let direct = {
            let lights = scene.static_channel_lights();
            scene.bake(&lights, &cells)
        };
        let (section, base) = with_ctx(&scene.fixture, |ctx, poses| {
            let mut plan = plan_half(&scene.fixture, ctx, poses, DEFAULT_BOUNCE_FILL);
            let mut request = std::mem::take(&mut plan.request);
            request.jobs.retain(|job| job.mover_id == mover_id);
            let (_, base, outcome) = bake_base(ctx, &plan, &request, DEFAULT_BOUNCE_FILL, None);
            let blocks = BounceBlocks::new(&outcome.movers, &cells);
            let section = channel_section(
                &direct,
                &blocks,
                &cells,
                &MoverRecords::new(&scene.fixture.kinematic_movers, poses),
                &MoverPayloadBars::with_override(Some(u64::MAX)),
                |_, _, _| false,
                |_| true,
            )
            .expect("assembles")
            .expect("a section");
            let section =
                MoverProbeChannelSection::from_bytes(&section.to_bytes()).expect("decodes");
            (section, base)
        });
        let mut local = HashMap::new();
        for (cell, &mask) in cells.valid_masks.iter().enumerate() {
            let bits = (0..64u32).filter(|bit| mask & (1 << bit) != 0);
            for (bit, probe) in bits.zip(cells.valid_probes(cell)) {
                local.insert(probe, (cell as u32, bit));
            }
        }
        let record = section.mover_record(mover_id).expect("the mover's record");
        let (knots, pose_count) = (effective_knots(record), record.pose_count());
        let slots = compact_slots(&scene.layout);
        Self {
            scene,
            section,
            base,
            slots,
            local,
            mover_id,
            knots,
            pose_count,
        }
    }

    /// The probe's id-34 tile.
    fn base_tile(&self, probe: usize) -> Vec<OctahedralAtlasTexel> {
        compact_tile(&self.base.section, self.slots[probe] as usize)
    }

    /// The probe's bounce as compose reads it at `t` (`mb_entry_blend`): the
    /// two defined states around t′ (stored, or id 34 where a terminus stores
    /// none or a mid pose reads the base), lerped texel by texel.
    fn bounce(&self, probe: usize, t: f32) -> Vec<OctahedralAtlasTexel> {
        let base = self.base_tile(probe);
        let (cell, local) = self.local[&probe];
        let Some(view) = self.section.bounce_entry_views().find(|view| {
            view.cell == cell
                && view.entry.mover_id == self.mover_id
                && view.entry.affected_probe_mask() & (1 << local) != 0
        }) else {
            return base;
        };
        let defined = |pose: usize| view.entry.mark(pose, local) != PoseMark::Unmarked;
        let (lo, hi, weight) = interval(&self.knots, t, defined);
        let state = |pose: usize| -> Vec<[f32; 3]> {
            match view.pose_tile(pose, local) {
                Some(halves) => halves
                    .to_vec()
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .map(|texel| texel.map(f16_bits_to_f32))
                    .collect(),
                None => base.iter().map(rgb).collect(),
            }
        };
        let (a, b) = (state(lo), state(hi));
        a.iter()
            .zip(&b)
            .map(|(a, b)| texel([0, 1, 2].map(|c| mix(a[c], b[c], weight))))
            .collect()
    }

    /// Whether the section's across marks name mid pose `pose` of this
    /// mover for local probe `local` of `cell`: direct blends across it.
    fn across(&self, cell: u32, local: u32, pose: usize) -> bool {
        self.section
            .listed_position(cell as usize)
            .and_then(|position| self.section.across_entry(position, self.mover_id))
            .is_some_and(|across| across.is_across(pose, local))
    }

    /// The direct interval at `t` for local probe `local` of `cell`: every
    /// pose defined but those the across marks name.
    fn direct_interval(&self, cell: u32, local: u32, t: f32) -> (usize, usize, f32) {
        interval(&self.knots, t, |pose| !self.across(cell, local, pose))
    }

    /// Each gated light's surviving visibility at the probe at `t`
    /// (`mc_record_visibility`, this mover's entry alone): q at the two
    /// defined poses around t′, lerped, blending across a pose the across
    /// marks name. A light no record masks there keeps 1.
    fn visibility(&self, probe: usize, t: f32) -> BTreeMap<u32, f32> {
        let (cell, local) = self.local[&probe];
        let bit = 1u64 << local;
        let (lo, hi, weight) = self.direct_interval(cell, local, t);
        let mut out = BTreeMap::new();
        for record in self.section.direct_record_views().filter(|record| {
            record.cell == cell
                && record.record.namespace == MoverLightNamespace::StaticLight
                && record.record.probe_mask & bit != 0
        }) {
            let slot = (record.record.probe_mask & (bit - 1)).count_ones() as usize;
            for mover in record
                .movers()
                .filter(|mover| mover.mover_id == self.mover_id)
            {
                assert_eq!(mover.pose_count, self.pose_count);
                let q = mover.probe_q(slot);
                let mut visible = f32::from(q[lo]) / 255.0;
                if weight > 0.0 {
                    visible = mix(visible, f32::from(q[hi]) / 255.0, weight);
                }
                out.insert(record.record.light_index, visible);
            }
        }
        out
    }

    /// What compose reads at the probe, for a miss's report: the interval
    /// around `t`, each pose's bounce mark, and each gated light's q per pose.
    fn explain(&self, probe: usize, t: f32) -> String {
        let (cell, local) = self.local[&probe];
        let bit = 1u64 << local;
        let span = self.direct_interval(cell, local, t);
        let marks: Vec<char> = self
            .section
            .bounce_entry_views()
            .find(|view| view.cell == cell && view.entry.mover_id == self.mover_id)
            .map(|view| {
                (0..self.pose_count)
                    .map(|pose| match view.entry.mark(pose, local) {
                        PoseMark::Stored => 'S',
                        PoseMark::Base => 'B',
                        PoseMark::Unmarked => '.',
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut q = Vec::new();
        for record in self.section.direct_record_views().filter(|record| {
            record.cell == cell
                && record.record.namespace == MoverLightNamespace::StaticLight
                && record.record.probe_mask & bit != 0
        }) {
            let slot = (record.record.probe_mask & (bit - 1)).count_ones() as usize;
            for mover in record
                .movers()
                .filter(|mover| mover.mover_id == self.mover_id)
            {
                q.push(format!(
                    "light {} {:?}",
                    record.record.light_index,
                    mover.probe_q(slot)
                ));
            }
        }
        format!(
            "knots {:?}; direct interval poses {}..{} weight {:.3}; bounce marks [{}]; q {}",
            self.knots.iter().map(|knot| knot.0).collect::<Vec<_>>(),
            span.0,
            span.1,
            span.2,
            marks.iter().collect::<String>(),
            q.join(", ")
        )
    }

    /// The probe's direct SH as compose reads it at `t`: every static
    /// light's term scaled by its surviving visibility.
    fn direct(&self, probe: usize, t: f32) -> Vec<OctahedralAtlasTexel> {
        let visibility = self.visibility(probe, t);
        let mut sum = [0f32; 27];
        for light in self.scene.static_channel_lights() {
            let visible = visibility.get(&light.light_index).copied().unwrap_or(1.0);
            let term = self.scene.term(&light, probe);
            for (total, value) in sum.iter_mut().zip(term) {
                *total += visible * value;
            }
        }
        sh_tile(&sum)
    }
}

fn sh_tile(coefficients: &[f32; 27]) -> Vec<OctahedralAtlasTexel> {
    pack_octahedral_irradiance_tile(
        coefficients,
        true,
        DEFAULT_IRRADIANCE_TILE_DIMENSION,
        DEFAULT_IRRADIANCE_TILE_BORDER,
    )
}

/// The map with mover `name` made static at `t`, baked: id 34 at every
/// bounce, and every light's direct term traced past the static brushes.
struct Reference {
    scene: Scene,
    base: ShBase,
    slots: Vec<u32>,
}

impl Reference {
    fn bake(path: &Path) -> Self {
        let scene = Scene::of(load_map(path));
        let base = with_ctx(&scene.fixture, |ctx, poses| {
            let plan = plan_half(&scene.fixture, ctx, poses, DEFAULT_BOUNCE_FILL);
            bake_base(
                ctx,
                &plan,
                &MoverBounceRequest::default(),
                DEFAULT_BOUNCE_FILL,
                None,
            )
            .1
        });
        let slots = compact_slots(&scene.layout);
        Self { scene, base, slots }
    }

    fn bounce(&self, probe: usize) -> Vec<OctahedralAtlasTexel> {
        compact_tile(&self.base.section, self.slots[probe] as usize)
    }

    fn direct(&self, probe: usize) -> Vec<OctahedralAtlasTexel> {
        let mut sum = [0f32; 27];
        for light in self.scene.static_channel_lights() {
            for (total, value) in sum.iter_mut().zip(self.scene.term(&light, probe)) {
                *total += value;
            }
        }
        sh_tile(&sum)
    }
}

/// Which side of the mover the probes sit on.
#[derive(Clone, Copy)]
enum Face {
    /// Just west of the brushes' west face (−x), within its y/z extent.
    West,
    /// Just above the brushes' top (+z), within their x/y footprint.
    Top,
}

/// The worst reading over one group of probes.
#[derive(Default)]
struct Worst {
    probes: usize,
    bounce: (f32, usize),
    direct: (f32, usize),
}

impl Worst {
    fn passes(&self) -> bool {
        self.bounce.0 <= BAR_LEVELS && self.direct.0 <= BAR_LEVELS
    }
}

/// Compose the probes one lattice step past `face` at each `t` in both
/// builds, against the static reference at that position. Returns, per t,
/// the mid-pose build's worst then the termini-only build's.
fn swept_face_against_reference(
    map: &str,
    mover: &str,
    face: Face,
    ts: &[f32],
) -> Vec<(f32, Worst, Worst)> {
    let text = std::fs::read_to_string(fixture_path(map)).expect("dev map");
    let named = format!("\"name\" \"{mover}\"");
    assert_eq!(text.matches(&named).count(), 1, "{map}: one {mover}");
    let dir = tempfile::tempdir().expect("maps dir");
    let write = |name: &str, text: &str| {
        let path = dir.path().join(format!("{name}.map"));
        std::fs::write(&path, text).expect("write map");
        path
    };
    let posed = Build::bake(&write(map, &text), mover);
    let termini = Build::bake(
        &write(
            &format!("{map}-termini"),
            &text.replace(&named, &format!("{named}\n\"light_poses\" \"2\"")),
        ),
        mover,
    );
    assert!(posed.pose_count > 2, "{map}: {mover} bakes mid poses");
    assert_eq!(termini.pose_count, 2, "{map}: light_poses 2 bakes termini");

    let analysed = posed.scene.mover_poses();
    let analysed = analysed
        .analysed
        .iter()
        .find(|analysed| analysed.mover_id == posed.mover_id)
        .expect("the mover");
    let first = &analysed.poses[0].bounds;
    let last = &analysed.poses[analysed.poses.len() - 1].bounds;
    let spacing = f64::from(CONFIG.probe_spacing) * QUAKE_PER_METRE;
    let mut out = Vec::new();
    for &t in ts {
        // Both movers run a straight two-waypoint path with no ramp, so the
        // brushes sit at t′ of the way from the first pose to the last.
        let offset: DVec3 = (last.min - first.min) * f64::from(t);
        let reference_path = write(
            &format!("{map}-{mover}-at-{t}"),
            &staticize_at(&text, mover, quake_offset(offset)),
        );
        let reference = Reference::bake(&reference_path);
        assert_eq!(
            (posed.scene.layout.dims, posed.scene.layout.world_min),
            (
                reference.scene.layout.dims,
                reference.scene.layout.world_min
            ),
            "{map}: both builds lay one probe grid"
        );
        let corners = [
            quake_offset(first.min + offset),
            quake_offset(first.max + offset),
        ];
        let lo: [f64; 3] = std::array::from_fn(|a| corners[0][a].min(corners[1][a]));
        let hi: [f64; 3] = std::array::from_fn(|a| corners[0][a].max(corners[1][a]));
        let within = |q: [f64; 3], a: usize| lo[a] <= q[a] && q[a] <= hi[a];
        let group: Vec<usize> = (0..posed.scene.layout.probe_positions.len())
            .filter(|&probe| {
                posed.scene.layout.validity[probe] != 0
                    && reference.scene.layout.validity[probe] != 0
            })
            .filter(|&probe| {
                let q = quake_offset(posed.scene.layout.probe_positions[probe]);
                match face {
                    Face::West => {
                        lo[0] - spacing < q[0] && q[0] < lo[0] && within(q, 1) && within(q, 2)
                    }
                    Face::Top => {
                        hi[2] < q[2] && q[2] < hi[2] + spacing && within(q, 0) && within(q, 1)
                    }
                }
            })
            .collect();
        assert!(!group.is_empty(), "{map} t′ {t}: probes past the face");
        let read = |build: &Build| {
            let mut worst = Worst {
                probes: group.len(),
                ..Worst::default()
            };
            for &probe in &group {
                let bounce = display_levels(&build.bounce(probe, t), &reference.bounce(probe));
                let direct =
                    direct_display_levels(&build.direct(probe, t), &reference.direct(probe));
                if bounce > worst.bounce.0 {
                    worst.bounce = (bounce, probe);
                }
                if direct > worst.direct.0 {
                    worst.direct = (direct, probe);
                }
            }
            worst
        };
        let (with_poses, termini_only) = (read(&posed), read(&termini));
        for (label, worst) in [("mid poses", &with_poses), ("termini only", &termini_only)] {
            eprintln!(
                "{map} {mover} t′ {t:.2}, {} probe(s) past the face: {label}: bounce worst \
                 {:.2} levels (probe {} at quake {:?}), direct worst {:.2} levels (probe {} at \
                 quake {:?})",
                worst.probes,
                worst.bounce.0,
                worst.bounce.1,
                quake_offset(posed.scene.layout.probe_positions[worst.bounce.1]),
                worst.direct.0,
                worst.direct.1,
                quake_offset(posed.scene.layout.probe_positions[worst.direct.1]),
            );
        }
        if !with_poses.passes() {
            for (term, (levels, probe)) in
                [("bounce", with_poses.bounce), ("direct", with_poses.direct)]
            {
                if levels > BAR_LEVELS {
                    eprintln!(
                        "  mid-pose {term} miss at probe {probe}: {}",
                        posed.explain(probe, t)
                    );
                }
            }
        }
        out.push((t, with_poses, termini_only));
    }
    out
}

fn assert_mid_poses_pass_and_termini_miss(map: &str, rows: &[(f32, Worst, Worst)]) {
    let misses: Vec<String> = rows
        .iter()
        .filter(|(_, with_poses, _)| !with_poses.passes())
        .map(|(t, worst, _)| {
            format!(
                "t′ {t}: bounce {:.2}, direct {:.2} levels",
                worst.bounce.0, worst.direct.0
            )
        })
        .collect();
    assert!(
        misses.is_empty(),
        "{map}: the mid-pose build misses the {BAR_LEVELS}-level bar:\n{}",
        misses.join("\n")
    );
    assert!(
        rows.iter().any(|(_, _, termini)| !termini.passes()),
        "{map}: the termini-only build must miss the bar somewhere"
    );
}

// On em_sweeper at t′ 0.30, the probes just west of the slab's west face
// (one lattice step, inside its y/z extent) compose bounce and direct
// within three display levels of a build with the slab static there. The
// same map with `light_poses 2` (today's termini-only states) misses: those
// probes still lerp from their t = 0 shadow. t′ 0.25 is not held (owner,
// 2026-11-10): those probes sit inside the slab until t 0.232, and pose
// states one lattice step apart cannot describe that crossing.
#[test]
fn the_sweepers_west_face_probes_compose_within_three_levels_of_a_static_slab() {
    let rows = swept_face_against_reference("enemy-movers", "em_sweeper", Face::West, &[0.30]);
    assert_mid_poses_pass_and_termini_miss("enemy-movers", &rows);
}

// On mover-setpiece at t′ 0.4 and 0.6, the probes just above the rising
// room's roof (one lattice step, inside its footprint) compose bounce and
// direct within three display levels of a build with the room static
// there. The same map with `light_poses 2` misses: those probes lerp toward
// their dark raised-room state.
#[test]
fn the_roof_tops_probes_compose_within_three_levels_of_a_static_room() {
    let rows =
        swept_face_against_reference("mover-setpiece", "setpiece_room", Face::Top, &[0.4, 0.6]);
    assert_mid_poses_pass_and_termini_miss("mover-setpiece", &rows);
}
