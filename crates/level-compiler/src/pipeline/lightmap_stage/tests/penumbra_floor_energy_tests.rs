//! On-demand measurement: where the penumbra floor moves per-light visibility and energy on a real map.
//! See: context/lib/build_pipeline.md §Compiler pipeline (step 12)
//
// Runs the production front end (parse → partition → visibility → geometry →
// BVH → atlas preparation, cut included) on `combat-demo` with the `--release`
// CLI at `--lightmap-density 0.025`, then bakes every layer light's partitions
// through the production chart walk twice, floor on and off, over the one
// prepared atlas, and once more floored with every texel's full set traced
// (no probe early-out), the floor's own reference. It prints per-light
// energy, a distance-to-edge histogram of
// changed texels, the top energy gains and the composited totals:
//   cargo test -p postretro-level-compiler --bin prl-build penumbra_floor_energy -- --ignored --nocapture
// `POSTRETRO_PENUMBRA_ENERGY_MAP` and `POSTRETRO_PENUMBRA_ENERGY_DENSITY`
// replace the map name and density. `POSTRETRO_PENUMBRA_ENERGY_PRLS=<on.prl>;<off.prl>`
// also sums the decoded id-22 irradiance of two CLI bakes of the same map.

use std::collections::VecDeque;
use std::collections::{BTreeMap, HashSet};

use glam::Vec3;

use crate::bake_control::BakeControl;
use crate::buried_lights::BuriedLights;
use crate::chart_raster::{CHART_PADDING_TEXELS, chart_grid};
use crate::light_namespaces::{AnimatedBakedLights, StaticBakedLights};
use crate::lightmap_bake::light_contribution_and_direction;
use crate::lightmap_layer::{
    IncrementalLayerAccumulator, LightmapLayer, SharedAtlas, bake_light_layer,
    bake_light_layer_full_sets, for_each_light_layer_chart_texel,
};
use crate::map_data::{MapLight, ShadowType};
use crate::pipeline::atlas_stage::{AtlasStageInputs, prepare_atlas_stage};
use crate::{geometry, parse, partition, portals, visibility};

/// A changed texel moved by more than this.
const CHANGE_EPSILON: f32 = 1.0e-4;
/// An edge texel has an axis neighbour on its parent grid whose floor-off
/// visibility differs by more than this.
const EDGE_DIFFERENCE: f32 = 0.05;
const BUCKETS: [&str; 7] = ["0", "1", "2", "3", "4-8", ">8", "no edge"];
const PRLS_ENV: &str = "POSTRETRO_PENUMBRA_ENERGY_PRLS";

/// `r + g + b` summed over every stored id-22 texel of a PRL (BC6H decoded),
/// and the texel count.
fn prl_irradiance(path: &str) -> (f64, usize) {
    use postretro_level_format::lightmap::{IRRADIANCE_FORMAT_RGBA16F, LightmapSection};
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let mut cursor = std::io::Cursor::new(&bytes);
    let meta = postretro_level_format::read_container(&mut cursor).expect("read_container");
    let id22 = postretro_level_format::read_section_data(
        &mut cursor,
        &meta,
        postretro_level_format::SectionId::Lightmap as u32,
    )
    .expect("read id 22")
    .expect("id 22 present");
    let section = LightmapSection::from_bytes(&id22).expect("id 22 decodes");
    let (mut sum, mut texels) = (0.0f64, 0usize);
    for block in &section.blocks {
        let w = usize::from(block.width);
        texels += w * usize::from(block.height);
        if section.irradiance_format == IRRADIANCE_FORMAT_RGBA16F {
            for bytes in block.irradiance.as_chunks::<8>().0 {
                for c in 0..3 {
                    sum += f64::from(crate::bc6h::f16_bits_to_f32(u16::from_le_bytes([
                        bytes[2 * c],
                        bytes[2 * c + 1],
                    ])));
                }
            }
        } else {
            for encoded in block.irradiance.as_chunks::<16>().0 {
                let decoded =
                    crate::bc6h::decode_bc6h_block(encoded).expect("the bake writes Mode 11");
                sum += decoded
                    .iter()
                    .map(|t| f64::from(t[0] + t[1] + t[2]))
                    .sum::<f64>();
            }
        }
    }
    (sum, texels)
}

/// One covered texel of one light, both settings.
#[derive(Clone, Copy)]
struct Record {
    face: u32,
    parent: u32,
    /// Parent-grid texel.
    px: u32,
    py: u32,
    off: f32,
    on: f32,
    /// Unshadowed contribution, `r + g + b` (the bake's luminance).
    lum: f32,
    world: Vec3,
    normal: Vec3,
}

impl Record {
    fn delta(&self) -> f32 {
        self.on - self.off
    }
}

/// Each face's parent grid: a dense id, the face's origin on it, its size.
#[derive(Clone, Copy)]
struct FaceGrid {
    parent: u32,
    origin: [u32; 2],
    dims: [u32; 2],
}

fn face_grids(
    shared: &SharedAtlas<'_>,
    cut_parents: &[crate::lightmap_bake::CutParent],
) -> Vec<FaceGrid> {
    let mut parent_of: Vec<Option<usize>> = vec![None; shared.charts.len()];
    for (cut, parent) in cut_parents.iter().enumerate() {
        for face in parent.faces.clone() {
            parent_of[face] = Some(cut);
        }
    }
    shared
        .charts
        .iter()
        .enumerate()
        .map(|(face, chart)| {
            let grid = chart_grid(chart);
            let parent = match parent_of[face] {
                Some(cut) => (shared.charts.len() + cut) as u32,
                None => face as u32,
            };
            FaceGrid {
                parent,
                origin: [grid.origin[0] as u32, grid.origin[1] as u32],
                dims: [grid.interior[0] as u32, grid.interior[1] as u32],
            }
        })
        .collect()
}

/// Per bake layer, the face owning each atlas texel (`u32::MAX` for none).
fn texel_faces(shared: &SharedAtlas<'_>, layer_count: u32) -> Vec<Vec<u32>> {
    (0..layer_count)
        .map(|layer| {
            let mut faces =
                vec![u32::MAX; shared.atlas_width as usize * shared.atlas_height as usize];
            for (face, placement) in shared.placements.iter().enumerate() {
                if placement.layer == layer {
                    for_each_light_layer_chart_texel(shared, face, |sample| {
                        faces[sample.idx as usize] = face as u32;
                    });
                }
            }
            faces
        })
        .collect()
}

/// Joined records of one light; also counts texels present in only one bake.
fn join(
    light: &MapLight,
    shared: &SharedAtlas<'_>,
    grids: &[FaceGrid],
    faces: &[Vec<u32>],
    off: &[LightmapLayer],
    on: &[LightmapLayer],
) -> (Vec<Record>, usize, usize) {
    let mut records = Vec::new();
    let (mut only_off, mut only_on) = (0, 0);
    for (a, b) in off.iter().zip(on) {
        assert_eq!(a.target_layer, b.target_layer);
        let layer_faces = &faces[a.target_layer as usize];
        let (mut i, mut j) = (0, 0);
        while i < a.texels.len() || j < b.texels.len() {
            let (x, y) = (a.texels.get(i), b.texels.get(j));
            match (x, y) {
                (Some(x), Some(y)) if x.idx == y.idx => {
                    let idx = x.idx;
                    let face = layer_faces[idx as usize];
                    assert_ne!(face, u32::MAX, "a record off every chart");
                    let chart = &shared.charts[face as usize];
                    let placement = &shared.placements[face as usize];
                    let tx = idx % shared.atlas_width - placement.x - CHART_PADDING_TEXELS;
                    let ty = idx / shared.atlas_width - placement.y - CHART_PADDING_TEXELS;
                    let world = crate::chart_raster::chart_texel_world_position(
                        chart, tx as i32, ty as i32,
                    );
                    let (contribution, _) =
                        light_contribution_and_direction(light, world, chart.normal);
                    let grid = grids[face as usize];
                    records.push(Record {
                        face,
                        parent: grid.parent,
                        px: grid.origin[0] + tx,
                        py: grid.origin[1] + ty,
                        off: x.raw_visibility,
                        on: y.raw_visibility,
                        lum: contribution.x + contribution.y + contribution.z,
                        world,
                        normal: chart.normal,
                    });
                    i += 1;
                    j += 1;
                }
                (Some(x), Some(y)) if x.idx < y.idx => {
                    only_off += 1;
                    i += 1;
                }
                (Some(_), Some(_)) => {
                    only_on += 1;
                    j += 1;
                }
                (Some(_), None) => {
                    only_off += 1;
                    i += 1;
                }
                (None, Some(_)) => {
                    only_on += 1;
                    j += 1;
                }
                (None, None) => unreachable!(),
            }
        }
    }
    (records, only_off, only_on)
}

/// Per record, its 4-connected distance in texels to the nearest floor-off
/// edge texel on its parent grid (`None`: no edge on that grid), and to the
/// grid's border. Computed only for grids holding a changed texel.
fn edge_distances(records: &[Record], grids: &[FaceGrid]) -> (Vec<Option<u32>>, Vec<u32>) {
    let mut by_parent: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (k, r) in records.iter().enumerate() {
        by_parent.entry(r.parent).or_default().push(k);
    }
    let mut distance = vec![None; records.len()];
    let mut border = vec![0; records.len()];
    for members in by_parent.values() {
        let dims = grids[records[members[0]].face as usize].dims;
        let (w, h) = (dims[0] as usize, dims[1] as usize);
        for &k in members {
            let r = &records[k];
            border[k] =
                r.px.min(r.py)
                    .min(dims[0] - 1 - r.px)
                    .min(dims[1] - 1 - r.py);
        }
        if !members
            .iter()
            .any(|&k| records[k].delta().abs() > CHANGE_EPSILON)
        {
            continue;
        }
        let mut grid = vec![f32::NAN; w * h];
        for &k in members {
            let r = &records[k];
            grid[r.py as usize * w + r.px as usize] = r.off;
        }
        let mut dist = vec![u32::MAX; w * h];
        let mut queue = VecDeque::new();
        for y in 0..h {
            for x in 0..w {
                let v = grid[y * w + x];
                if v.is_nan() {
                    continue;
                }
                let differs = |nx: usize, ny: usize| {
                    let n = grid[ny * w + nx];
                    !n.is_nan() && (n - v).abs() > EDGE_DIFFERENCE
                };
                if (x > 0 && differs(x - 1, y))
                    || (x + 1 < w && differs(x + 1, y))
                    || (y > 0 && differs(x, y - 1))
                    || (y + 1 < h && differs(x, y + 1))
                {
                    dist[y * w + x] = 0;
                    queue.push_back((x, y));
                }
            }
        }
        if queue.is_empty() {
            continue;
        }
        while let Some((x, y)) = queue.pop_front() {
            let d = dist[y * w + x] + 1;
            let mut visit = |nx: usize, ny: usize| {
                if dist[ny * w + nx] > d {
                    dist[ny * w + nx] = d;
                    queue.push_back((nx, ny));
                }
            };
            if x > 0 {
                visit(x - 1, y);
            }
            if x + 1 < w {
                visit(x + 1, y);
            }
            if y > 0 {
                visit(x, y - 1);
            }
            if y + 1 < h {
                visit(x, y + 1);
            }
        }
        for &k in members {
            let r = &records[k];
            distance[k] = Some(dist[r.py as usize * w + r.px as usize]);
        }
    }
    (distance, border)
}

fn bucket(distance: Option<u32>) -> usize {
    match distance {
        None => 6,
        Some(d @ 0..=3) => d as usize,
        Some(4..=8) => 4,
        Some(_) => 5,
    }
}

/// Floor-off value class: exact 0, exact 1, or a fraction.
fn class(v: f32) -> usize {
    if v <= 0.0 {
        0
    } else if v >= 1.0 {
        2
    } else {
        1
    }
}

#[derive(Default, Clone, Copy)]
struct Histogram {
    count: [usize; 7],
    abs_energy: [f64; 7],
    net_energy: [f64; 7],
    /// Changed texels per bucket whose floor-off value is 0 / fraction / 1.
    by_class: [[usize; 3]; 7],
}

impl Histogram {
    fn add(&mut self, r: &Record, distance: Option<u32>) {
        let b = bucket(distance);
        self.count[b] += 1;
        self.abs_energy[b] += f64::from(r.delta().abs() * r.lum);
        self.net_energy[b] += f64::from(r.delta() * r.lum);
        self.by_class[b][class(r.off)] += 1;
    }

    fn print(&self, title: &str) {
        println!("  {title}");
        println!(
            "    {:>8} {:>9} {:>14} {:>14}   off=0 / frac / off=1",
            "dist", "texels", "sum|dv|*lum", "sum dv*lum"
        );
        for (b, name) in BUCKETS.iter().enumerate() {
            println!(
                "    {:>8} {:>9} {:>14.4} {:>14.4}   {} / {} / {}",
                name,
                self.count[b],
                self.abs_energy[b],
                self.net_energy[b],
                self.by_class[b][0],
                self.by_class[b][1],
                self.by_class[b][2],
            );
        }
    }
}

struct Gain {
    light: usize,
    record: Record,
    distance: Option<u32>,
    border: u32,
}

/// The production front end's products up to the lightmap stage.
struct Front {
    args: crate::Args,
    map_data: crate::map_data::MapData,
    config: crate::lightmap_bake::LightmapConfig,
    geo: crate::geometry::GeometryResult,
    bvh: bvh::bvh::Bvh<f32, 3>,
    primitives: Vec<crate::bvh_build::BvhPrimitive>,
    prepared: crate::lightmap_bake::PreparedAtlas,
    /// Source indices of the lights the stage bakes layers for, in its order.
    layer_lights: Vec<usize>,
}

/// Parse `map` with the `--release` CLI at `density` and run the front end
/// as `pipeline::run_after_parsing` runs it up to the lightmap stage.
/// combat-demo's data script names no light, so its membership manifest
/// leaves the light set alone and is skipped.
fn front_end(map: &str, density: &str, baked: &std::path::Path) -> Front {
    let map_path = crate::fixture_pipeline::fixture_path(map);
    let args = crate::parse_args_from(
        [
            map_path.to_str().expect("UTF-8 path").to_owned(),
            "--release".to_owned(),
            "--lightmap-density".to_owned(),
            density.to_owned(),
            "--baked-root".to_owned(),
            baked.to_str().expect("UTF-8 path").to_owned(),
        ]
        .into_iter(),
    )
    .expect("arguments parse");
    let map_data = parse::parse_map_file(&args.input, args.format).expect("map parses");
    let config = crate::lightmap_config(&args, map_data.lightmap_density);
    let result = partition::partition(&map_data.brush_volumes).expect("partition");
    let buried = BuriedLights::classify(&result.tree, &map_data.lights);
    let static_lights = StaticBakedLights::from_lights_excluding(&map_data.lights, &buried);
    let animated_lights = AnimatedBakedLights::from_lights_excluding(&map_data.lights, &buried);
    let generated_portals = portals::generate_portals(&result.tree);
    let exterior_leaves = visibility::find_exterior_leaves(&result.tree, &generated_portals);
    let mut vis_result = visibility::encode_vis(&result.tree, &exterior_leaves);
    let (mut geo, mut face_brushes) =
        geometry::extract_geometry_with_brushes(&result.faces, &result.tree, &exterior_leaves);
    let (mut bvh, mut primitives, bvh_section) =
        crate::bvh_build::build_bvh(&geo).expect("BVH builds");
    let control = BakeControl::unrestricted();
    let atlas = prepare_atlas_stage(
        AtlasStageInputs {
            map_data: &map_data,
            static_lights: &static_lights,
            animated_lights: &animated_lights,
            config: &config,
            working_set_max_bytes: args.working_set_max_bytes,
            generated_portals: &generated_portals,
            exterior_leaves: &exterior_leaves,
            control: &control,
        },
        &mut geo,
        &mut face_brushes,
        &mut vis_result.leaves_section,
        &bvh_section,
    )
    .expect("atlas prepares");
    if let Some(rebuilt) = atlas.rebuilt {
        bvh = rebuilt.bvh;
        primitives = rebuilt.primitives;
    }
    let layer_lights = static_lights
        .entries()
        .iter()
        .filter(|entry| entry.light.shadow_type != ShadowType::Sdf)
        .map(|entry| entry.source_index)
        .collect();
    drop((static_lights, animated_lights));
    Front {
        args,
        map_data,
        config,
        geo,
        bvh,
        primitives,
        prepared: atlas.prepared,
        layer_lights,
    }
}

#[test]
#[ignore = "measurement: compiles a content/dev/maps level's lightmap in process, floor off and on"]
fn penumbra_floor_energy_on_a_real_map() {
    let map =
        std::env::var("POSTRETRO_PENUMBRA_ENERGY_MAP").unwrap_or_else(|_| "combat-demo".into());
    let density =
        std::env::var("POSTRETRO_PENUMBRA_ENERGY_DENSITY").unwrap_or_else(|_| "0.025".into());
    let baked =
        std::env::temp_dir().join(format!("postretro-penumbra-energy-{}", std::process::id()));
    std::fs::create_dir_all(baked.join("materials")).expect("baked root");
    let front = front_end(&map, &density, &baked);
    let Front {
        args,
        map_data,
        config,
        geo,
        bvh,
        primitives,
        prepared,
        layer_lights,
    } = &front;
    let layer_lights: Vec<(usize, &MapLight)> = layer_lights
        .iter()
        .map(|&source| (source, &map_data.lights[source]))
        .collect();
    let control = BakeControl::unrestricted();
    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };
    println!(
        "{map} at {} m, {} soft-shadow samples: {} charts ({} cut parents), {} bake layers of {}x{}, {} layer lights",
        config.lightmap_density,
        args.soft_shadow_samples,
        prepared.charts.len(),
        prepared.cut_parents.len(),
        prepared.layer_count,
        prepared.atlas_width,
        prepared.atlas_height,
        layer_lights.len()
    );

    let grids = face_grids(&shared, &prepared.cut_parents);
    let faces = texel_faces(&shared, prepared.layer_count);

    let mut partitions: Vec<[Vec<LightmapLayer>; 2]> = Vec::new();
    let mut total = Histogram::default();
    let (mut total_off, mut total_on, mut total_full) = (0.0f64, 0.0f64, 0.0f64);
    let mut gains: Vec<Gain> = Vec::new();
    // (face) -> (changed texels, net dv*lum, normal)
    let mut by_face: BTreeMap<u32, (usize, f64, f64, Vec3)> = BTreeMap::new();
    let mut ceiling = Histogram::default();
    let mut ceiling_border = [0usize; 7];
    let mut ceiling_border_energy = [0.0f64; 7];
    for (position, &(source_index, light)) in layer_lights.iter().enumerate() {
        let started = std::time::Instant::now();
        let bake = |floor: bool| {
            bake_light_layer(
                light,
                &shared,
                bvh,
                primitives,
                geo,
                args.soft_shadow_samples,
                floor,
                &control,
            )
        };
        let off = bake(false);
        let on = bake(true);
        let elapsed = started.elapsed();
        // Floored with every texel's full set traced: the floor's own
        // reference, which no probe early-out can bias.
        let full = bake_light_layer_full_sets(
            light,
            &shared,
            bvh,
            primitives,
            geo,
            args.soft_shadow_samples,
            &control,
        );
        let (full_records, _, _) = join(light, &shared, &grids, &faces, &off, &full);
        let e_full: f64 = full_records.iter().map(|r| f64::from(r.on * r.lum)).sum();
        let (records, only_off, only_on) = join(light, &shared, &grids, &faces, &off, &on);
        let (distance, border) = edge_distances(&records, &grids);

        let (mut e_off, mut e_on) = (0.0f64, 0.0f64);
        let mut changed = 0usize;
        let (mut sum_abs, mut max_abs) = (0.0f64, 0.0f32);
        let (mut pos, mut neg) = (0usize, 0usize);
        let (mut pos_e, mut neg_e) = (0.0f64, 0.0f64);
        let mut hist = Histogram::default();
        for (k, r) in records.iter().enumerate() {
            e_off += f64::from(r.off * r.lum);
            e_on += f64::from(r.on * r.lum);
            let d = r.delta();
            if d.abs() <= CHANGE_EPSILON {
                continue;
            }
            changed += 1;
            sum_abs += f64::from(d.abs());
            max_abs = max_abs.max(d.abs());
            if d > 0.0 {
                pos += 1;
                pos_e += f64::from(d * r.lum);
            } else {
                neg += 1;
                neg_e += f64::from(d * r.lum);
            }
            hist.add(r, distance[k]);
            total.add(r, distance[k]);
            let face = by_face.entry(r.face).or_insert((0, 0.0, 0.0, r.normal));
            face.0 += 1;
            face.1 += f64::from(d * r.lum);
            face.2 += f64::from(d.abs() * r.lum);
            if r.normal.y < -0.5 {
                ceiling.add(r, distance[k]);
                let b = bucket(Some(border[k]));
                ceiling_border[b] += 1;
                ceiling_border_energy[b] += f64::from(d * r.lum);
            }
            if d > 0.0 {
                gains.push(Gain {
                    light: position,
                    record: *r,
                    distance: distance[k],
                    border: border[k],
                });
            }
        }
        total_off += e_off;
        total_on += e_on;
        total_full += e_full;
        let label = map_data
            .light_source_labels
            .get(source_index)
            .cloned()
            .unwrap_or_default();
        println!(
            "\nlight {position} (source {}, {label}): {:?}, size {} m, origin [{:.3}, {:.3}, {:.3}] m, {} texels, baked off+on in {:.1} s",
            source_index,
            light.light_type,
            light.light_size,
            light.origin.x,
            light.origin.y,
            light.origin.z,
            records.len(),
            elapsed.as_secs_f64()
        );
        println!(
            "  energy off {e_off:.4}, on {e_on:.4}, ratio {:.5} (on - off {:+.4}); full set {e_full:.4}, on / full {:.5}",
            e_on / e_off.max(1e-30),
            e_on - e_off,
            e_on / e_full.max(1e-30),
        );
        println!(
            "  changed > {CHANGE_EPSILON}: {changed} ({:.3}%), mean |dv| {:.4}, max |dv| {max_abs:.4}; positive {pos} (+{pos_e:.4}), negative {neg} ({neg_e:.4}); only off {only_off}, only on {only_on}",
            100.0 * changed as f64 / records.len().max(1) as f64,
            sum_abs / changed.max(1) as f64,
        );
        hist.print("changed texels by distance to a floor-off edge (texels, 4-connected)");
        partitions.push([off, on]);
    }

    println!(
        "\nALL LIGHTS: energy off {total_off:.4}, on {total_on:.4}, ratio {:.5}; full set {total_full:.4}, on / full {:.5}",
        total_on / total_off.max(1e-30),
        total_on / total_full.max(1e-30)
    );
    total.print("all changed texels by distance to a floor-off edge");
    ceiling.print("ceiling (normal.y < -0.5) changed texels by distance to a floor-off edge");
    println!("  ceiling changed texels by distance to their parent grid's border:");
    for b in 0..6 {
        println!(
            "    {:>8} {:>9} net dv*lum {:+.4}",
            BUCKETS[b], ceiling_border[b], ceiling_border_energy[b]
        );
    }

    println!(
        "\nfaces by |dv|*lum (top 15): face, normal, changed texels, net dv*lum, sum |dv|*lum"
    );
    let mut face_rows: Vec<_> = by_face.into_iter().collect();
    face_rows.sort_by(|a, b| b.1.2.total_cmp(&a.1.2));
    for (face, (count, net, abs, normal)) in face_rows.iter().take(15) {
        println!(
            "  face {face:>5} n [{:+.2}, {:+.2}, {:+.2}] {count:>8} {net:+12.4} {abs:12.4}",
            normal.x, normal.y, normal.z
        );
    }

    gains.sort_by(|a, b| {
        (b.record.delta() * b.record.lum).total_cmp(&(a.record.delta() * a.record.lum))
    });
    println!("\ntop 20 energy gains (dv * lum):");
    for g in gains.iter().take(20) {
        let r = &g.record;
        let light = layer_lights[g.light].1;
        println!(
            "  light {} @[{:.2},{:.2},{:.2}] face {} n[{:+.2},{:+.2},{:+.2}] p[{:.3},{:.3},{:.3}] texel ({},{}) off {:.4} on {:.4} dv {:+.4} gain {:.4} edge-dist {} border {}",
            g.light,
            light.origin.x,
            light.origin.y,
            light.origin.z,
            r.face,
            r.normal.x,
            r.normal.y,
            r.normal.z,
            r.world.x,
            r.world.y,
            r.world.z,
            r.px,
            r.py,
            r.off,
            r.on,
            r.delta(),
            r.delta() * r.lum,
            g.distance.map_or("none".to_owned(), |d| d.to_string()),
            g.border,
        );
    }

    // Composited irradiance, in global light order per layer, as the stage folds.
    let mut composite = [0.0f64; 2];
    for (setting, sum) in composite.iter_mut().enumerate() {
        for layer in 0..prepared.layer_count {
            let mut accumulator = IncrementalLayerAccumulator::for_atlas_layer(&shared, layer);
            for (position, &(_, light)) in layer_lights.iter().enumerate() {
                for partition in &partitions[position][setting] {
                    if partition.target_layer == layer {
                        accumulator.fold_partition(light, partition, &shared);
                    }
                }
            }
            let plane = accumulator.finish();
            *sum += plane
                .irradiance
                .as_chunks::<4>()
                .0
                .iter()
                .zip(&plane.coverage)
                .filter(|(_, covered)| **covered)
                .map(|(t, _)| f64::from(t[0] + t[1] + t[2]))
                .sum::<f64>();
        }
    }
    println!(
        "\ncomposited irradiance (r+g+b over covered texels, pre-dilation): off {:.4}, on {:.4}, ratio {:.5}",
        composite[0],
        composite[1],
        composite[1] / composite[0].max(1e-30)
    );
    if let Ok(pair) = std::env::var(PRLS_ENV) {
        let (on_path, off_path) = pair.split_once(';').expect("<on.prl>;<off.prl>");
        let ((on, on_texels), (off, off_texels)) =
            (prl_irradiance(on_path), prl_irradiance(off_path));
        println!(
            "CLI bakes, decoded id 22 (r+g+b over {on_texels} / {off_texels} block texels, dilated): off {off:.4}, on {on:.4}, ratio {:.5}",
            on / off.max(1e-30)
        );
    }
    let unique_faces: HashSet<u32> = gains.iter().map(|g| g.record.face).collect();
    println!(
        "texels with a positive change lie on {} faces",
        unique_faces.len()
    );
    assert!(
        total_off > 0.0 && total_on > 0.0,
        "the lights reach something"
    );
    let _ = std::fs::remove_dir_all(&baked);
}
