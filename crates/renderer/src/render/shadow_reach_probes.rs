// On-demand compiled-map probes: shadow reach walk == brute force, cell-major contiguity.
// See: context/lib/rendering_pipeline.md §7.1

use std::ops::Range;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use glam::{Mat4, Vec3};
use postretro_level_loader::{
    FalloffModel, KinematicGeometry, LightType, LoadedKinematicMover, MapLight, ShadowType,
};
use postretro_render_cpu::shadow_reach::{ShadowReachIndex, ShadowReachScratch};
use postretro_render_data::cone_frustum::cone_frustum_planes;
use postretro_render_data::geometry::BvhTree;

use super::shadow_world_draws::{ShadowRegion, ShadowWorldDraws};
use crate::lighting::cube_shadow::cube_face_matrices;

/// Prebuilt maps under `content/dev/maps/`. A missing `.prl` is skipped, never
/// compiled: these bakes take minutes to hours (testing_guide.md §3, slow suites).
const MAPS: [&str; 3] = [
    "stress-warren-hallway-inspection",
    "campaign-test",
    "stress-warren-mini",
];
const HALLWAY: &str = "stress-warren-hallway-inspection";
/// Entity 825 in `stress-warren-hallway-inspection.map`: the cyan
/// `light_dynamic` the `warren_lift_0` car carries.
const LIFT_MOVER: &str = "warren_lift_0";
/// Its authored `.map` origin (-4160 0 288) in engine meters,
/// (-qy, qz, -qx) × 0.0254.
const LIFT_LIGHT_AUTHORED_ORIGIN: [f64; 3] = [0.0, 7.3152, 105.664];
const LIFT_HEIGHTS: usize = 5;

const SEED: u64 = 0x5AD0_C0DE_F110_C057;
const RANDOM_SPOTS: usize = 200;
const RANDOM_POINTS: usize = 200;

/// The slice of a loaded level the probes read. The rest of the `LevelWorld`
/// is dropped right after load.
struct ProbeMap {
    name: &'static str,
    bvh: BvhTree,
    lights: Vec<MapLight>,
    kinematic: KinematicGeometry,
    index_count: u32,
    cell_count: usize,
}

/// Each map's load outcome by name; `None` for a skipped map.
type LoadedMaps = Vec<(&'static str, Option<Arc<ProbeMap>>)>;

/// One load per map per test process, shared by both probes. The lock is held
/// across the load so the two tests never hold two copies of a large level.
static LOADED: Mutex<LoadedMaps> = Mutex::new(Vec::new());

fn probe_map(name: &'static str) -> Option<Arc<ProbeMap>> {
    if let Ok(selected) = std::env::var("POSTRETRO_PROBE_MAPS")
        && !selected.split(',').any(|map| map == name)
    {
        return None;
    }
    let mut loaded = LOADED.lock().unwrap_or_else(|poison| poison.into_inner());
    if let Some((_, map)) = loaded.iter().find(|(loaded, _)| *loaded == name) {
        return map.clone();
    }
    let path = format!(
        "{}/../../content/dev/maps/{name}.prl",
        env!("CARGO_MANIFEST_DIR")
    );
    let map = if std::path::Path::new(&path).exists() {
        // `load_prl` under the default streaming modes reads SH and lightmap
        // bodies positionally on demand, so the heavy sections stay on disk.
        let started = Instant::now();
        let mut world = postretro_level_loader::load_prl(&path)
            .unwrap_or_else(|err| panic!("{name}: load_prl({path}) failed: {err}"));
        let map = ProbeMap {
            name,
            bvh: std::mem::replace(
                &mut world.bvh,
                BvhTree {
                    nodes: Vec::new(),
                    leaves: Vec::new(),
                    root_node_index: 0,
                },
            ),
            lights: std::mem::take(&mut world.lights),
            kinematic: std::mem::take(&mut world.kinematic_geometry),
            index_count: world.indices.len() as u32,
            cell_count: world.cells.len(),
        };
        drop(world);
        println!(
            "== {name}: loaded {path} in {:.2}s ({} cells, {} BVH nodes, {} leaves, {} indices, {} lights, {} movers)",
            started.elapsed().as_secs_f64(),
            map.cell_count,
            map.bvh.nodes.len(),
            map.bvh.leaves.len(),
            map.index_count,
            map.lights.len(),
            map.kinematic.movers.len(),
        );
        Some(Arc::new(map))
    } else {
        eprintln!("== {name}: skipped, {path} is missing (prebuilt maps only)");
        None
    };
    loaded.push((name, map.clone()));
    map
}

// --- Walk == brute force ----------------------------------------------------

/// Per-probe-kind draw shape, for the report.
#[derive(Default)]
struct Tally {
    regions: u32,
    empty: u32,
    cells: u64,
    max_cells: u32,
    draws: u64,
    max_draws: u32,
    visited: u64,
    max_visited: u32,
}

impl Tally {
    fn add(&mut self, region: RegionOutcome) {
        self.regions += 1;
        self.empty += u32::from(region.cells == 0);
        self.cells += u64::from(region.cells);
        self.max_cells = self.max_cells.max(region.cells);
        self.draws += u64::from(region.draws);
        self.max_draws = self.max_draws.max(region.draws);
        self.visited += u64::from(region.visited);
        self.max_visited = self.max_visited.max(region.visited);
    }

    fn report(&self, map: &ProbeMap, kind: &str) {
        let n = f64::from(self.regions.max(1));
        println!(
            "   {kind}: {} regions ({} empty) | reached cells avg {:.1} max {} | draws avg {:.1} max {} vs {} leaves | visited nodes avg {:.1} max {} of {}",
            self.regions,
            self.empty,
            self.cells as f64 / n,
            self.max_cells,
            self.draws as f64 / n,
            self.max_draws,
            map.bvh.leaves.len(),
            self.visited as f64 / n,
            self.max_visited,
            map.bvh.nodes.len(),
        );
    }
}

#[derive(Clone, Copy)]
struct RegionOutcome {
    cells: u32,
    draws: u32,
    visited: u32,
}

struct ReachProbe<'a> {
    map: &'a ProbeMap,
    index: ShadowReachIndex,
    scratch: ShadowReachScratch,
    glue: ShadowWorldDraws,
    failures: Vec<String>,
}

impl<'a> ReachProbe<'a> {
    fn new(map: &'a ProbeMap) -> Self {
        let index = ShadowReachIndex::new(&map.bvh.nodes, &map.bvh.leaves);
        let scratch = index.scratch();
        Self {
            map,
            index,
            scratch,
            glue: ShadowWorldDraws::install(Some(&map.bvh), map.index_count),
            failures: Vec::new(),
        }
    }

    /// Walk one region and hold it against the every-leaf oracle: same cells
    /// both ways, ranges that are exactly the reached cells' merged ranges, and
    /// the renderer glue issuing those same ranges.
    fn check(
        &mut self,
        label: &dyn Fn() -> String,
        region: ShadowRegion,
        matrix: &Mat4,
    ) -> RegionOutcome {
        let planes = cone_frustum_planes(matrix);
        let ranges = self.index.reach(&planes, &mut self.scratch).to_vec();
        let mut walked = self.scratch.cells().to_vec();
        walked.sort_unstable();
        let visited = self.scratch.stats().visited_nodes;
        let oracle = self.index.brute_force_reached_cells(&planes);

        if walked != oracle {
            let walk_only: Vec<u32> = walked
                .iter()
                .copied()
                .filter(|cell| oracle.binary_search(cell).is_err())
                .collect();
            let oracle_only: Vec<u32> = oracle
                .iter()
                .copied()
                .filter(|cell| walked.binary_search(cell).is_err())
                .collect();
            let mut dedup = walked.clone();
            dedup.dedup();
            self.failures.push(format!(
                "{}: {} walk mismatch at {}: walk {} cells ({} duplicates), oracle {}; walk-only {:?}; oracle-only {:?}",
                self.map.name,
                region_name(region),
                label(),
                walked.len(),
                walked.len() - dedup.len(),
                oracle.len(),
                walk_only,
                oracle_only,
            ));
        }

        let mut expected: Vec<Range<u32>> = oracle
            .iter()
            .flat_map(|&cell| self.index.cell_ranges(cell).iter().cloned())
            .collect();
        expected.sort_unstable_by_key(|range| range.start);
        let expected = merge_abutting(expected);
        if ranges != expected {
            self.failures.push(format!(
                "{}: {} range mismatch at {}: walk issued {} ranges, oracle cells' merged ranges are {}; first walk {:?}, first oracle {:?}",
                self.map.name,
                region_name(region),
                label(),
                ranges.len(),
                expected.len(),
                ranges.iter().take(8).collect::<Vec<_>>(),
                expected.iter().take(8).collect::<Vec<_>>(),
            ));
        }

        self.glue.begin_frame();
        let issued = self.glue.ranges(region, matrix);
        if issued != ranges.as_slice() {
            self.failures.push(format!(
                "{}: {} glue mismatch at {}: ShadowWorldDraws issued {} ranges, planner {}",
                self.map.name,
                region_name(region),
                label(),
                issued.len(),
                ranges.len(),
            ));
        }

        RegionOutcome {
            cells: walked.len() as u32,
            draws: ranges.len() as u32,
            visited,
        }
    }

    /// Every region a light draws: one spot frustum, or six cube faces from
    /// the pool's own face matrices. Directional lights hold no pool slot.
    fn check_light(
        &mut self,
        label: &dyn Fn() -> String,
        light: &MapLight,
        tally: &mut Tally,
    ) -> Vec<RegionOutcome> {
        match light.light_type {
            LightType::Spot => {
                let matrix = postretro_lighting::light_space_matrix(light);
                let outcome = self.check(label, ShadowRegion::Spot(0), &matrix);
                tally.add(outcome);
                vec![outcome]
            }
            LightType::Point => cube_face_matrices(light)
                .iter()
                .enumerate()
                .map(|(face, matrix)| {
                    let face_label = || format!("{} face {face}", label());
                    let outcome =
                        self.check(&face_label, ShadowRegion::CubeFace(face as u32), matrix);
                    tally.add(outcome);
                    outcome
                })
                .collect(),
            LightType::Directional => Vec::new(),
        }
    }
}

fn region_name(region: ShadowRegion) -> String {
    match region {
        ShadowRegion::Spot(slot) => format!("spot {slot}"),
        ShadowRegion::CubeFace(layer) => format!("cube face {layer}"),
    }
}

fn merge_abutting(ranges: Vec<Range<u32>>) -> Vec<Range<u32>> {
    let mut merged: Vec<Range<u32>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        match merged.last_mut() {
            Some(last) if last.end == range.start => last.end = range.end,
            _ => merged.push(range),
        }
    }
    merged
}

/// A pool-shaped light at `origin`, so the probe frusta come from the same
/// `light_space_matrix` / `cube_face_matrices` the shadow pools upload.
fn probe_light(
    light_type: LightType,
    origin: Vec3,
    direction: Vec3,
    cone_outer: f32,
    range: f32,
) -> MapLight {
    MapLight {
        origin: origin.as_dvec3().to_array(),
        light_type,
        intensity: 1.0,
        color: [1.0; 3],
        falloff_model: FalloffModel::InverseSquared,
        falloff_range: range,
        cone_angle_inner: cone_outer * 0.5,
        cone_angle_outer: cone_outer,
        cone_direction: direction.to_array(),
        is_dynamic: true,
        casts_entity_shadows: false,
        animated_slot: None,
        tags: Vec::new(),
        cell_index: u32::MAX,
        shadow_type: ShadowType::StaticLightMap,
    }
}

/// splitmix64: a fixed-seed generator with no dependency.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }

    fn direction(&mut self) -> Vec3 {
        loop {
            let v = Vec3::new(
                self.range(-1.0, 1.0),
                self.range(-1.0, 1.0),
                self.range(-1.0, 1.0),
            );
            let len_sq = v.length_squared();
            if len_sq > 1e-4 && len_sq <= 1.0 {
                return v / len_sq.sqrt();
            }
        }
    }
}

/// A random light position: half uniform over the world bounds (often in
/// solid or void), half near a random leaf, so most frusta reach geometry.
fn random_origin(rng: &mut Rng, bvh: &BvhTree, min: Vec3, max: Vec3) -> Vec3 {
    if rng.unit() < 0.5 {
        Vec3::new(
            rng.range(min.x, max.x),
            rng.range(min.y, max.y),
            rng.range(min.z, max.z),
        )
    } else {
        let slot = (rng.next_u64() % bvh.leaves.len() as u64) as usize;
        let leaf = &bvh.leaves[slot];
        let center = (Vec3::from(leaf.aabb_min) + Vec3::from(leaf.aabb_max)) * 0.5;
        center + rng.direction() * rng.range(0.0, 2.0)
    }
}

/// Log-uniform range from half a meter to the world diagonal: small lights and
/// lights that reach most of the map both appear.
fn random_range(rng: &mut Rng, diagonal: f32) -> f32 {
    let (lo, hi) = (0.5f32.ln(), diagonal.max(1.0).ln());
    rng.range(lo, hi).exp()
}

fn world_bounds(bvh: &BvhTree) -> (Vec3, Vec3) {
    bvh.leaves.iter().fold(
        (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
        |(min, max), leaf| {
            (
                min.min(Vec3::from(leaf.aabb_min)),
                max.max(Vec3::from(leaf.aabb_max)),
            )
        },
    )
}

/// A mover's waypoint chain from its `path`, in travel order.
fn waypoint_chain(map: &ProbeMap, mover: &LoadedKinematicMover) -> Vec<Vec3> {
    let mut chain = Vec::new();
    let mut seen = Vec::new();
    let mut current = mover.path.as_str();
    while !current.is_empty() && !seen.contains(&current) {
        seen.push(current);
        let Some(waypoint) = map.kinematic.waypoints.iter().find(|w| w.name == current) else {
            break;
        };
        chain.push(waypoint.origin);
        current = waypoint.next.as_str();
    }
    chain
}

/// The point at fraction `t` of the polyline's length.
fn along(chain: &[Vec3], t: f32) -> Vec3 {
    let total: f32 = chain.windows(2).map(|w| w[0].distance(w[1])).sum();
    let mut remaining = total * t;
    for w in chain.windows(2) {
        let length = w[0].distance(w[1]);
        if remaining <= length && length > 0.0 {
            return w[0].lerp(w[1], remaining / length);
        }
        remaining -= length;
    }
    *chain.last().expect("chain has at least two waypoints")
}

/// Every light a mover carries, posed at `LIFT_HEIGHTS` points spanning its
/// whole travel. The carried light sits at the mover's position plus its
/// local offset (`light_bridge.rs` `follow_transform_position`; probed movers
/// do not spin). Returns whether the hallway lift light was probed.
fn probe_carried_lights(probe: &mut ReachProbe<'_>, tally: &mut Tally) -> bool {
    let map = probe.map;
    let mut lift_probed = false;
    for mover in &map.kinematic.movers {
        for member in &mover.carried_lights {
            let Some(light) = map.lights.get(member.alpha_light_index as usize) else {
                continue;
            };
            if light.light_type == LightType::Directional {
                continue;
            }
            let chain = waypoint_chain(map, mover);
            if chain.len() < 2 {
                println!(
                    "   carried light {} on `{}`: path `{}` resolves to {} waypoint(s); not swept",
                    member.alpha_light_index,
                    mover.name,
                    mover.path,
                    chain.len()
                );
                continue;
            }
            let is_lift = map.name == HALLWAY && mover.name == LIFT_MOVER;
            if is_lift {
                assert_eq!(
                    light.light_type,
                    LightType::Point,
                    "{LIFT_MOVER} light must be the authored point light"
                );
                assert!(light.is_dynamic, "{LIFT_MOVER} light must be light_dynamic");
                assert!(
                    light
                        .origin
                        .iter()
                        .zip(LIFT_LIGHT_AUTHORED_ORIGIN)
                        .all(|(loaded, authored)| (loaded - authored).abs() < 1e-3),
                    "{LIFT_MOVER} light origin {:?} is not entity 825's authored {:?}",
                    light.origin,
                    LIFT_LIGHT_AUTHORED_ORIGIN,
                );
                lift_probed = true;
            }
            let travel = chain[chain.len() - 1] - chain[0];
            println!(
                "   {}carried {:?} light {} on `{}` (mover origin {:?}, waypoints {:?}, travel {:.2} m, local offset {:?}, range {:.2} m):",
                if is_lift {
                    "LIFT LIGHT (entity 825) "
                } else {
                    ""
                },
                light.light_type,
                member.alpha_light_index,
                mover.name,
                mover.origin,
                chain,
                travel.length(),
                member.local_offset,
                light.falloff_range,
            );
            for step in 0..LIFT_HEIGHTS {
                let t = step as f32 / (LIFT_HEIGHTS - 1) as f32;
                let position = along(&chain, t) + member.local_offset;
                let mut posed = light.clone();
                posed.origin = position.as_dvec3().to_array();
                let label = || {
                    format!(
                        "carried light {} on `{}` at t={t:.2} {position:?}",
                        member.alpha_light_index, mover.name
                    )
                };
                let faces = probe.check_light(&label, &posed, tally);
                let cells: Vec<u32> = faces.iter().map(|f| f.cells).collect();
                let draws: Vec<u32> = faces.iter().map(|f| f.draws).collect();
                let visited: Vec<u32> = faces.iter().map(|f| f.visited).collect();
                println!(
                    "     t={t:.2} y={:.2}: reached cells {cells:?} | draws {draws:?} = {} total vs {} per-leaf draws before | visited nodes {visited:?} of {}",
                    position.y,
                    draws.iter().sum::<u32>(),
                    faces.len() * map.bvh.leaves.len(),
                    map.bvh.nodes.len(),
                );
            }
        }
    }
    lift_probed
}

/// R2 on-demand half: on each prebuilt map, the CPU walk's reached cells equal
/// the every-leaf oracle in both directions, for randomized spot and cube
/// frusta, every map light, and every carried light along its mover's travel,
/// including the hallway lift light (entity 825) at five heights. Faces come
/// from `cube_face_matrices` and spots from `light_space_matrix`, the
/// matrices the shadow pools upload. Run with:
///   cargo test -p postretro-renderer --lib shadow_reach_probes -- --ignored --nocapture
/// `POSTRETRO_PROBE_MAPS=campaign-test,…` narrows the maps.
#[test]
#[ignore = "loads large prebuilt stress maps; on-demand only"]
fn stress_maps_shadow_reach_walk_matches_brute_force() {
    let mut failures = Vec::new();
    for name in MAPS {
        let Some(map) = probe_map(name) else {
            continue;
        };
        let started = Instant::now();
        let mut probe = ReachProbe::new(&map);
        println!(
            "== {name}: walk vs brute force ({} reach cells, {} leaves, {} nodes)",
            probe.index.cell_count(),
            map.bvh.leaves.len(),
            map.bvh.nodes.len()
        );

        let (min, max) = world_bounds(&map.bvh);
        let diagonal = (max - min).length();
        let mut rng = Rng(SEED);
        let mut random_spots = Tally::default();
        for i in 0..RANDOM_SPOTS {
            let origin = random_origin(&mut rng, &map.bvh, min, max);
            let direction = rng.direction();
            let cone_outer = rng.range(0.05, 1.3);
            let range = random_range(&mut rng, diagonal);
            let light = probe_light(LightType::Spot, origin, direction, cone_outer, range);
            let label = || {
                format!(
                    "random spot {i} at {origin:?} dir {direction:?} outer {cone_outer:.3} range {range:.2}"
                )
            };
            probe.check_light(&label, &light, &mut random_spots);
        }
        let mut random_points = Tally::default();
        for i in 0..RANDOM_POINTS {
            let origin = random_origin(&mut rng, &map.bvh, min, max);
            let range = random_range(&mut rng, diagonal);
            let light = probe_light(LightType::Point, origin, Vec3::NEG_Z, 0.0, range);
            let label = || format!("random point {i} at {origin:?} range {range:.2}");
            probe.check_light(&label, &light, &mut random_points);
        }
        random_spots.report(&map, "random spots");
        random_points.report(&map, "random cube faces");

        let mut map_spots = Tally::default();
        let mut map_points = Tally::default();
        let mut directional = 0;
        for (i, light) in map.lights.iter().enumerate() {
            let label = || {
                format!(
                    "map light {i} ({:?}) at {:?}",
                    light.light_type, light.origin
                )
            };
            match light.light_type {
                LightType::Spot => {
                    probe.check_light(&label, light, &mut map_spots);
                }
                LightType::Point => {
                    probe.check_light(&label, light, &mut map_points);
                }
                LightType::Directional => directional += 1,
            }
        }
        map_spots.report(&map, "map spot lights");
        map_points.report(&map, "map point-light cube faces");
        if directional > 0 {
            println!("   {directional} directional lights: no pool region, not probed");
        }

        let mut carried = Tally::default();
        let lift_probed = probe_carried_lights(&mut probe, &mut carried);
        if carried.regions > 0 {
            carried.report(&map, "carried-light regions across travel");
        }
        if name == HALLWAY {
            assert!(
                lift_probed,
                "{HALLWAY}: no `{LIFT_MOVER}` mover carrying a light was found; the lift probe did not run"
            );
        }

        println!(
            "   {name}: {} failures, probe time {:.2}s",
            probe.failures.len(),
            started.elapsed().as_secs_f64()
        );
        failures.append(&mut probe.failures);
    }
    assert!(
        failures.is_empty(),
        "{} reach mismatches:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

// --- Cell-major contiguity --------------------------------------------------

/// D3 on-demand half: on each prebuilt map, face cuts included, every cell's
/// leaves form one contiguous index range, and consecutive non-empty cells
/// abut (cell-major order). Checked through the planner's per-cell grouping
/// and, independently, from the raw leaves. Every violation is listed by cell.
#[test]
#[ignore = "loads large prebuilt stress maps; on-demand only"]
fn stress_maps_cells_own_one_contiguous_index_range() {
    let mut failures = Vec::new();
    for name in MAPS {
        let Some(map) = probe_map(name) else {
            continue;
        };
        let index = ShadowReachIndex::new(&map.bvh.nodes, &map.bvh.leaves);

        let mut split_cells = Vec::new();
        let mut gaps = Vec::new();
        let mut non_empty = 0usize;
        let mut first_start = None;
        let mut previous: Option<(u32, u32)> = None;
        for cell in 0..index.cell_count() as u32 {
            let ranges = index.cell_ranges(cell);
            let Some(first) = ranges.first() else {
                continue;
            };
            non_empty += 1;
            first_start.get_or_insert(first.start);
            if ranges.len() > 1 {
                split_cells.push(format!("cell {cell}: {} ranges {ranges:?}", ranges.len()));
            }
            if let Some((previous_cell, previous_end)) = previous
                && previous_end != first.start
            {
                gaps.push(format!(
                    "cell {cell} starts at {} but previous non-empty cell {previous_cell} ends at {previous_end}",
                    first.start
                ));
            }
            previous = Some((cell, ranges.last().expect("non-empty").end));
        }

        // Independent of the planner: a cell is contiguous when its leaves'
        // index span equals the sum of their counts (no overlaps, no holes).
        let mut spans: Vec<Option<(u32, u32, u64)>> = vec![None; index.cell_count()];
        let mut empty_leaves = 0;
        for leaf in &map.bvh.leaves {
            if leaf.index_count == 0 {
                empty_leaves += 1;
                continue;
            }
            let end = leaf.index_offset + leaf.index_count;
            let span = &mut spans[leaf.cell_id as usize];
            *span = Some(match *span {
                None => (leaf.index_offset, end, u64::from(leaf.index_count)),
                Some((lo, hi, sum)) => (
                    lo.min(leaf.index_offset),
                    hi.max(end),
                    sum + u64::from(leaf.index_count),
                ),
            });
        }
        let raw_split: Vec<String> = spans
            .iter()
            .enumerate()
            .filter_map(|(cell, span)| {
                let (lo, hi, sum) = (*span)?;
                (u64::from(hi - lo) != sum)
                    .then(|| format!("cell {cell}: span {lo}..{hi} holds {sum} leaf indices"))
            })
            .collect();

        let last_end = previous.map(|(_, end)| end);
        println!(
            "== {name}: contiguity | {} loaded cells, {} reach cells ({} with geometry), {} leaves ({} empty), indices {:?}..{:?} of {} | split cells {}, raw split cells {}, cell-order gaps {}",
            map.cell_count,
            index.cell_count(),
            non_empty,
            map.bvh.leaves.len(),
            empty_leaves,
            first_start,
            last_end,
            map.index_count,
            split_cells.len(),
            raw_split.len(),
            gaps.len(),
        );
        for (kind, list) in [
            ("split cell", &split_cells),
            ("raw split cell", &raw_split),
            ("cell-order gap", &gaps),
        ] {
            failures.extend(list.iter().map(|line| format!("{name}: {kind}: {line}")));
        }
    }
    assert!(
        failures.is_empty(),
        "{} contiguity violations:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
