//! On-demand measurement: how the penumbra floor moves each animated light's weight-map energy on a real map.
//! See: context/lib/build_pipeline.md §Compiler pipeline (step 12)
//
// Runs the static measurement's front end (`penumbra_floor_energy_tests`:
// production parse → partition → visibility → geometry → BVH → atlas
// preparation, cut included), then the production chunk builder and the
// weight-map bake three ways over the one prepared atlas: floor off, floor on
// (each light's embedded-emitter flag tested as the stage tests it), and
// floored with every texel's full set traced (no probe early-out), the
// floor's own reference. Per animated light it prints the texels, weight
// sums and energies, the ratios, the changed texels by distance to a
// floor-off edge, and the emitter flag:
//   cargo test -p postretro-level-compiler --bin prl-build penumbra_floor_animated_energy -- --ignored --nocapture
// `POSTRETRO_PENUMBRA_ENERGY_MAP` and `POSTRETRO_PENUMBRA_ENERGY_DENSITY`
// replace the map and density (default `kinematic-platform` at its own
// 0.04 m). `POSTRETRO_PENUMBRA_ENERGY_PRLS=<on.prl>;<off.prl>` also decodes
// both CLI bakes' id-25 sections: the per-light weight sums that ship, the
// in-process pools they must equal, and where the section bytes differ.
//
// Energy. An entry stores Lambert × falloff × cone × visibility with the
// light's colour and intensity divided out (`contribution_to_weight`), so a
// light's energy on a texel is `weight × intensity × (r + g + b)`: the static
// measurement's `visibility × lum` for the same light. The factor is constant
// per light, as is the animation curve that scales every texel of a light at
// runtime, so a light's ratios equal those of its plain weight sums.

use std::collections::BTreeMap;

use postretro_level_format::animated_light_weight_maps::{
    AnimatedLightWeightMapsSection, TexelLight,
};

use super::penumbra_floor_energy_tests::{
    BUCKETS, CHANGE_EPSILON, Front, Histogram, PRLS_ENV, Record, edge_distances, face_grids,
    front_end,
};
use crate::animated_light_chunks::build_placed_animated_light_chunks;
use crate::animated_light_weight_maps::{
    WEIGHT_EPSILON, WeightMapBake, WeightMapInputs, bake_animated_light_weight_maps_full_sets,
    bake_animated_light_weight_maps_with, contribution_to_weight, cull_unlit_chunks,
    embedded_emitters,
};
use crate::bake_control::BakeControl;
use crate::chart_raster::{CHART_PADDING_TEXELS, chart_interior_dims, chart_texel_world_position};
use crate::light_namespaces::AnimatedBakedLights;
use crate::lightmap_bake::light_contribution_and_direction;
use crate::lightmap_layer::SharedAtlas;
use crate::map_data::ShadowType;
use crate::texel_footprint::EmbeddedEmitters;

const SETTINGS: [&str; 3] = ["off", "on", "full set"];

/// One texel of one animated light's bake domain: the texels the chunks
/// listing it cover, inside the chart, where its unshadowed weight passes
/// the weight epsilon (the bake's own grid).
struct Texel {
    face: u32,
    tx: i32,
    ty: i32,
    /// Unshadowed weight (`contribution_to_weight` at the texel centre).
    base: f32,
    /// Stored weight per setting (off, on, full set); 0 where no entry.
    weight: [f32; 3],
}

impl Texel {
    fn visibility(&self, setting: usize) -> f32 {
        self.weight[setting] / self.base
    }
}

/// Every animated light's domain texels, keyed by position in `lights`,
/// joined across the three bakes, which share one chunk structure.
fn join_bakes(
    front: &Front,
    lights: &[crate::map_data::MapLight],
    chunk_section: &postretro_level_format::animated_light_chunks::AnimatedLightChunksSection,
    bakes: &[&AnimatedLightWeightMapsSection; 3],
) -> BTreeMap<u32, Vec<Texel>> {
    for bake in &bakes[1..] {
        assert_eq!(
            bake.chunk_rects, bakes[0].chunk_rects,
            "the three bakes share one chunk layout"
        );
        assert_eq!(bake.offset_counts.len(), bakes[0].offset_counts.len());
    }
    let charts = &front.prepared.charts;
    let placements = &front.prepared.placements;
    let padding = CHART_PADDING_TEXELS as i32;
    let mut by_light: BTreeMap<u32, Vec<Texel>> = BTreeMap::new();
    let mut matched = [0usize; 3];
    for (index, chunk) in chunk_section.chunks.iter().enumerate() {
        let rect = bakes[0].chunk_rects[index];
        let face = chunk.face_index as usize;
        let chart = &charts[face];
        let placement = placements[face];
        let usable = chart.uv_extent[0] > 0.0 && chart.uv_extent[1] > 0.0;
        let (interior_w, interior_h) = chart_interior_dims(chart);
        // The identity layout keeps every chunk at its bake-layer rect.
        let origin = [
            rect.compact_x as i32 - placement.x as i32 - padding,
            rect.compact_y as i32 - placement.y as i32 - padding,
        ];
        let start = chunk.index_offset as usize;
        let listed = &chunk_section.light_indices[start..start + chunk.index_count as usize];
        for ry in 0..rect.height {
            for rx in 0..rect.width {
                let (tx, ty) = (origin[0] + rx as i32, origin[1] + ry as i32);
                let at = (rect.texel_offset + ry * rect.width + rx) as usize;
                let entries: [&[TexelLight]; 3] = std::array::from_fn(|setting| {
                    let entry = bakes[setting].offset_counts[at];
                    let start = entry.offset as usize;
                    &bakes[setting].texel_lights[start..start + entry.count as usize]
                });
                let inside = usable && tx >= 0 && ty >= 0 && tx < interior_w && ty < interior_h;
                if !inside {
                    assert!(
                        entries.iter().all(|e| e.is_empty()),
                        "an entry off the chart"
                    );
                    continue;
                }
                let world = chart_texel_world_position(chart, tx, ty);
                for &light_index in listed {
                    let light = &lights[light_index as usize];
                    let weight: [f32; 3] = std::array::from_fn(|setting| {
                        entries[setting]
                            .iter()
                            .find(|e| e.light_index == light_index)
                            .map_or(0.0, |e| e.weight)
                    });
                    for (setting, &w) in weight.iter().enumerate() {
                        matched[setting] += usize::from(w > 0.0);
                    }
                    if light.shadow_type == ShadowType::Sdf {
                        assert!(weight.iter().all(|&w| w == 0.0), "an sdf light baked");
                        continue;
                    }
                    let (contribution, _) =
                        light_contribution_and_direction(light, world, chart.normal);
                    let base = contribution_to_weight(contribution, light.color, light.intensity);
                    if base <= WEIGHT_EPSILON {
                        assert!(weight.iter().all(|&w| w == 0.0), "an entry under epsilon");
                        continue;
                    }
                    for &w in &weight {
                        assert!(
                            w / base <= 1.0 + 1.0e-4,
                            "visibility {} > 1: the texel walk is misaligned",
                            w / base
                        );
                    }
                    by_light.entry(light_index).or_default().push(Texel {
                        face: face as u32,
                        tx,
                        ty,
                        base,
                        weight,
                    });
                }
            }
        }
    }
    for (setting, bake) in bakes.iter().enumerate() {
        assert_eq!(
            matched[setting],
            bake.texel_lights.len(),
            "every {} entry lies on a domain texel of a light its chunk lists",
            SETTINGS[setting]
        );
    }
    by_light
}

/// The id-25 section of a PRL, raw and decoded.
fn prl_weight_maps(path: &str) -> (Vec<u8>, AnimatedLightWeightMapsSection) {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let mut cursor = std::io::Cursor::new(&bytes);
    let meta = postretro_level_format::read_container(&mut cursor).expect("read_container");
    let raw = postretro_level_format::read_section_data(
        &mut cursor,
        &meta,
        postretro_level_format::SectionId::AnimatedLightWeightMaps as u32,
    )
    .expect("read id 25")
    .expect("id 25 present");
    let section = AnimatedLightWeightMapsSection::from_bytes(&raw).expect("id 25 decodes");
    (raw, section)
}

/// Section byte ranges: header, chunk rects, blocks, offset table, pool.
fn regions(section: &AnimatedLightWeightMapsSection) -> [(&'static str, usize, usize); 5] {
    let mut at = 28;
    let mut next = |len: usize| {
        let range = (at, at + len);
        at += len;
        range
    };
    let rects = next(section.chunk_rects.len() * 24);
    let blocks = next(section.blocks.len() * 28);
    let offsets = next(section.offset_counts.len() * 8);
    let pool = next(section.texel_lights.len() * 12);
    [
        ("header", 0, 28),
        ("chunk rects", rects.0, rects.1),
        ("blocks", blocks.0, blocks.1),
        ("offset table", offsets.0, offsets.1),
        ("light pool", pool.0, pool.1),
    ]
}

fn differing(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b).filter(|(x, y)| x != y).count() + a.len().abs_diff(b.len())
}

/// Weight sum and entry count per light index.
fn sums_by_light(section: &AnimatedLightWeightMapsSection) -> BTreeMap<u32, (f64, usize)> {
    let mut sums: BTreeMap<u32, (f64, usize)> = BTreeMap::new();
    for entry in &section.texel_lights {
        let sum = sums.entry(entry.light_index).or_default();
        sum.0 += f64::from(entry.weight);
        sum.1 += 1;
    }
    sums
}

/// Where two id-25 sections of one map differ, region by region and texel
/// by texel, and why.
fn explain_section_difference(
    off: (&[u8], &AnimatedLightWeightMapsSection),
    on: (&[u8], &AnimatedLightWeightMapsSection),
) {
    let (off_raw, off) = off;
    let (on_raw, on) = on;
    println!(
        "  byte length off {} -> on {} ({:+}); positional differing bytes {}",
        off_raw.len(),
        on_raw.len(),
        on_raw.len() as i64 - off_raw.len() as i64,
        differing(off_raw, on_raw)
    );
    println!(
        "  {:>12} {:>12} {:>12} {:>12} {:>12} {:>9} {:>14}",
        "", "chunks", "blocks", "offsets", "pool", "pages", "page size"
    );
    for (name, s) in [("off", off), ("on", on)] {
        println!(
            "  {name:>12} {:>12} {:>12} {:>12} {:>12} {:>9} {:>14}",
            s.chunk_rects.len(),
            s.blocks.len(),
            s.offset_counts.len(),
            s.texel_lights.len(),
            s.compact_layers,
            s.page_size
        );
    }
    let (off_regions, on_regions) = (regions(off), regions(on));
    println!("  differing bytes by region (positional within the region):");
    let mut region_total = 0;
    for (a, b) in off_regions.iter().zip(&on_regions) {
        let bytes = differing(&off_raw[a.1..a.2], &on_raw[b.1..b.2]);
        region_total += bytes;
        println!(
            "    {:>12}: {:>10} of {:>10} -> {:>10} bytes",
            a.0,
            bytes,
            a.2 - a.1,
            b.2 - b.1
        );
    }
    println!("    region sum {region_total}");
    println!(
        "  chunk rects equal: {}; blocks equal: {}",
        off.chunk_rects == on.chunk_rects,
        off.blocks == on.blocks
    );
    if off.offset_counts.len() != on.offset_counts.len() {
        println!("  offset tables differ in length: no per-texel walk");
        return;
    }
    // Per texel: counts, offsets and the entry lists themselves.
    let (mut count_differs, mut offset_differs) = (0usize, 0usize);
    let mut first_offset_shift = None;
    let (mut added, mut removed, mut weight_changed, mut same) = (0usize, 0usize, 0usize, 0usize);
    let mut direction_changed = 0usize;
    let mut shift_hist: BTreeMap<i64, usize> = BTreeMap::new();
    for (t, (a, b)) in off.offset_counts.iter().zip(&on.offset_counts).enumerate() {
        if a.count != b.count {
            count_differs += 1;
        }
        if a.offset != b.offset {
            offset_differs += 1;
            first_offset_shift.get_or_insert(t);
        }
        let shift = i64::from(b.offset) - i64::from(a.offset);
        *shift_hist.entry(shift.signum()).or_default() += 1;
        let ea = &off.texel_lights[a.offset as usize..(a.offset + a.count) as usize];
        let eb = &on.texel_lights[b.offset as usize..(b.offset + b.count) as usize];
        for x in ea {
            match eb.iter().find(|y| y.light_index == x.light_index) {
                None => removed += 1,
                Some(y) => {
                    if y.direction_oct != x.direction_oct {
                        direction_changed += 1;
                    }
                    if y.weight.to_bits() == x.weight.to_bits() {
                        same += 1;
                    } else {
                        weight_changed += 1;
                    }
                }
            }
        }
        added += eb
            .iter()
            .filter(|y| !ea.iter().any(|x| x.light_index == y.light_index))
            .count();
    }
    println!(
        "  offset table: {count_differs} texels change their entry count, {offset_differs} of {} change their pool offset (first at texel {:?}); offset shift sign histogram {:?}",
        off.offset_counts.len(),
        first_offset_shift,
        shift_hist
    );
    println!(
        "  pool entries matched per texel: {same} bit-identical, {weight_changed} weight changed, {added} added (on only), {removed} removed (off only); {direction_changed} direction changed"
    );
    println!(
        "  pool growth {} entries x 12 bytes = {} bytes (section growth {})",
        on.texel_lights.len() as i64 - off.texel_lights.len() as i64,
        (on.texel_lights.len() as i64 - off.texel_lights.len() as i64) * 12,
        on_raw.len() as i64 - off_raw.len() as i64
    );
    println!(
        "  aligned (per-texel) content change: {} weight bytes changed at most ({} entries x 4) + {} added entries x 12",
        weight_changed * 4,
        weight_changed,
        added
    );
}

#[test]
#[ignore = "measurement: compiles a content/dev/maps level's animated weight maps in process, floor off, on and full set"]
fn penumbra_floor_animated_energy_on_a_real_map() {
    let map = std::env::var("POSTRETRO_PENUMBRA_ENERGY_MAP")
        .unwrap_or_else(|_| "kinematic-platform".into());
    let density =
        std::env::var("POSTRETRO_PENUMBRA_ENERGY_DENSITY").unwrap_or_else(|_| "0.04".into());
    let baked = std::env::temp_dir().join(format!(
        "postretro-penumbra-animated-energy-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(baked.join("materials")).expect("baked root");
    let front = front_end(&map, &density, &baked);
    let control = BakeControl::unrestricted();
    let prepared = &front.prepared;

    let animated =
        AnimatedBakedLights::from_lights_excluding(&front.map_data.lights, &front.buried);
    let source_of: Vec<usize> = animated.entries().iter().map(|e| e.source_index).collect();
    let (lights, _) = animated.to_parallel_vecs();
    let (chunk_section, leaf_ranges) = build_placed_animated_light_chunks(
        &front.bvh_section,
        &animated,
        &prepared.charts,
        &prepared.placements,
        &front.geo.face_index_ranges,
        front.config.lightmap_density,
    );
    println!(
        "{map} at {} m, {} soft-shadow samples: {} charts ({} cut parents), {} bake layers of {}x{}, {} animated lights, {} chunks",
        front.config.lightmap_density,
        front.args.soft_shadow_samples,
        prepared.charts.len(),
        prepared.cut_parents.len(),
        prepared.layer_count,
        prepared.atlas_width,
        prepared.atlas_height,
        lights.len(),
        chunk_section.chunks.len()
    );
    assert!(
        !chunk_section.chunks.is_empty(),
        "the map has animated chunks"
    );

    let inputs = |penumbra_floor| WeightMapInputs {
        bvh: &front.bvh,
        primitives: &front.primitives,
        geometry: &front.geo,
        chunk_section: &chunk_section,
        lights: &lights,
        face_charts: &prepared.charts,
        face_placements: &prepared.placements,
        layout: &prepared.layout,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        static_atlas_layer_count: prepared.layer_count,
        area_sample_count: front.args.soft_shadow_samples,
        cut_parents: &prepared.cut_parents,
        penumbra_floor,
        step_threshold: crate::texel_step_detector::TEXEL_STEP_THRESHOLD,
    };
    let (off_inputs, on_inputs) = (inputs(false), inputs(true));
    // As the stage tests them: floor on only.
    let flags = embedded_emitters(&on_inputs, &control);
    let timed = |name: &str, bake: &dyn Fn() -> WeightMapBake| {
        let started = std::time::Instant::now();
        let bake = bake();
        println!(
            "  {name}: {} chunks, {} offsets, {} pool entries, {} byte section, {} texel-step warnings, baked in {:.1} s",
            bake.section.chunk_rects.len(),
            bake.section.offset_counts.len(),
            bake.section.texel_lights.len(),
            bake.section.byte_len(),
            bake.texel_steps.warnings.len(),
            started.elapsed().as_secs_f64()
        );
        bake
    };
    let off = timed("off", &|| {
        bake_animated_light_weight_maps_with(&off_inputs, &EmbeddedEmitters::default(), &control)
            .expect("floor-off bake")
    });
    let on = timed("on", &|| {
        bake_animated_light_weight_maps_with(&on_inputs, &flags, &control).expect("floored bake")
    });
    let full = timed("full set", &|| {
        bake_animated_light_weight_maps_full_sets(&on_inputs, &control).expect("full-set bake")
    });
    let bakes = [&off.section, &on.section, &full.section];

    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };
    let grids = face_grids(&shared, &prepared.cut_parents);
    let by_light = join_bakes(&front, &lights, &chunk_section, &bakes);

    let mut totals = [0.0f64; 3];
    let mut total_hist = Histogram::default();
    for (position, light) in lights.iter().enumerate() {
        let source = source_of[position];
        let label = front
            .map_data
            .light_source_labels
            .get(source)
            .cloned()
            .unwrap_or_default();
        let fraction = flags.fraction(position);
        println!(
            "\nanimated light {position} (source {source}, {label}): {:?}, {:?} shadow, size {} m, falloff range {} m, intensity {}, colour {:?}, origin [{:.3}, {:.3}, {:.3}] m, animated {}",
            light.light_type,
            light.shadow_type,
            light.light_size,
            light.falloff_range,
            light.intensity,
            light.color,
            light.origin.x,
            light.origin.y,
            light.origin.z,
            light.animation.is_some()
        );
        println!(
            "  embedded emitter: {} (fraction {fraction:.4})",
            if flags.is_embedded(position) {
                "flagged, all-clear early-out skipped"
            } else {
                "not flagged"
            }
        );
        let Some(texels) = by_light.get(&(position as u32)) else {
            println!("  no domain texel");
            continue;
        };
        // energy = weight × intensity × (r + g + b): the static measurement's lum.
        let scale = f64::from(light.intensity) * f64::from(light.color.iter().sum::<f32>());
        let mut sums = [0.0f64; 3];
        let mut lit = [0usize; 3];
        for texel in texels {
            for setting in 0..3 {
                sums[setting] += f64::from(texel.weight[setting]);
                lit[setting] += usize::from(texel.weight[setting] > 0.0);
            }
        }
        for setting in 0..3 {
            totals[setting] += sums[setting] * scale;
        }
        let only_on = texels
            .iter()
            .filter(|t| t.weight[0] == 0.0 && t.weight[1] > 0.0)
            .count();
        let only_off = texels
            .iter()
            .filter(|t| t.weight[0] > 0.0 && t.weight[1] == 0.0)
            .count();
        println!(
            "  {} domain texels; entries off {} / on {} / full set {}; on only {only_on}, off only {only_off}",
            texels.len(),
            lit[0],
            lit[1],
            lit[2]
        );
        println!(
            "  weight sum off {:.4}, on {:.4}, full set {:.4}",
            sums[0], sums[1], sums[2]
        );
        println!(
            "  energy (weight x intensity x (r+g+b)) off {:.4}, on {:.4}, full set {:.4}; on / off {:.5} (on - off {:+.4}), on / full {:.5}, off / full {:.5}",
            sums[0] * scale,
            sums[1] * scale,
            sums[2] * scale,
            sums[1] / sums[0].max(1e-30),
            (sums[1] - sums[0]) * scale,
            sums[1] / sums[2].max(1e-30),
            sums[0] / sums[2].max(1e-30)
        );

        let records: Vec<Record> = texels
            .iter()
            .map(|t| {
                let grid = grids[t.face as usize];
                let chart = &prepared.charts[t.face as usize];
                Record {
                    face: t.face,
                    parent: grid.parent,
                    px: grid.origin[0] + t.tx as u32,
                    py: grid.origin[1] + t.ty as u32,
                    off: t.visibility(0),
                    on: t.visibility(1),
                    lum: t.base * scale as f32,
                    world: chart_texel_world_position(chart, t.tx, t.ty),
                    normal: chart.normal,
                }
            })
            .collect();
        let (distance, _) = edge_distances(&records, &grids);
        let mut hist = Histogram::default();
        let (mut changed, mut sum_abs, mut max_abs) = (0usize, 0.0f64, 0.0f32);
        let (mut pos, mut neg, mut pos_e, mut neg_e) = (0usize, 0usize, 0.0f64, 0.0f64);
        for (k, r) in records.iter().enumerate() {
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
            total_hist.add(r, distance[k]);
        }
        println!(
            "  changed visibility > {CHANGE_EPSILON}: {changed} ({:.3}%), mean |dv| {:.4}, max |dv| {max_abs:.4}; positive {pos} (+{pos_e:.4}), negative {neg} ({neg_e:.4})",
            100.0 * changed as f64 / records.len().max(1) as f64,
            sum_abs / changed.max(1) as f64,
        );
        hist.print("changed texels by distance to a floor-off edge (texels, 4-connected)");
        let (mut early, mut early_max, mut early_e) = (0usize, 0.0f32, 0.0f64);
        for t in texels {
            let d = t.visibility(1) - t.visibility(2);
            if d.abs() > CHANGE_EPSILON {
                early += 1;
                early_max = early_max.max(d.abs());
                early_e += f64::from(t.weight[1] - t.weight[2]) * scale;
            }
        }
        println!(
            "  on vs full set: {early} texels differ > {CHANGE_EPSILON} (max |dv| {early_max:.4}, net energy {early_e:+.4}) — the probe early-out's share"
        );
    }
    println!(
        "\nALL ANIMATED LIGHTS: energy off {:.4}, on {:.4}, full set {:.4}; on / off {:.5}, on / full {:.5}",
        totals[0],
        totals[1],
        totals[2],
        totals[1] / totals[0].max(1e-30),
        totals[1] / totals[2].max(1e-30)
    );
    total_hist.print("all changed texels by distance to a floor-off edge");
    println!("  (distance buckets: {})", BUCKETS.join(", "));

    // What ships: the stage culls chunks with no lit texel before the
    // compact repack; the pool and its order carry over unchanged.
    let culled: Vec<AnimatedLightWeightMapsSection> = [&off, &on]
        .iter()
        .map(|bake| {
            cull_unlit_chunks(
                &chunk_section,
                bake.section.clone(),
                &leaf_ranges,
                &prepared.layout,
                prepared.atlas_width,
            )
            .weight_maps
        })
        .collect();
    for (name, section) in ["off", "on"].iter().zip(&culled) {
        println!(
            "in-process {name} after the unlit-chunk cull: {} chunks, {} blocks, {} offsets, {} pool entries, {} bytes",
            section.chunk_rects.len(),
            section.blocks.len(),
            section.offset_counts.len(),
            section.texel_lights.len(),
            section.byte_len()
        );
    }
    println!("\nin-process sections, off vs on (culled, identity layout):");
    let (off_bytes, on_bytes) = (culled[0].to_bytes(), culled[1].to_bytes());
    explain_section_difference((&off_bytes, &culled[0]), (&on_bytes, &culled[1]));

    if let Ok(pair) = std::env::var(PRLS_ENV) {
        let (on_path, off_path) = pair.split_once(';').expect("<on.prl>;<off.prl>");
        let (off_raw, off_prl) = prl_weight_maps(off_path);
        let (on_raw, on_prl) = prl_weight_maps(on_path);
        println!("\nCLI bakes, id 25 (off {off_path}, on {on_path}):");
        let (off_sums, on_sums) = (sums_by_light(&off_prl), sums_by_light(&on_prl));
        for (&light_index, &(on_sum, on_count)) in &on_sums {
            let (off_sum, off_count) = off_sums.get(&light_index).copied().unwrap_or_default();
            let source = source_of.get(light_index as usize).copied();
            let label = source
                .and_then(|s| front.map_data.light_source_labels.get(s).cloned())
                .unwrap_or_default();
            println!(
                "  light index {light_index} ({label}): weight sum off {off_sum:.4} ({off_count} entries), on {on_sum:.4} ({on_count} entries), on / off {:.5}",
                on_sum / off_sum.max(1e-30)
            );
        }
        println!(
            "  shipped pools equal the in-process pools: off {}, on {}",
            off_prl.texel_lights == culled[0].texel_lights,
            on_prl.texel_lights == culled[1].texel_lights
        );
        explain_section_difference((&off_raw, &off_prl), (&on_raw, &on_prl));
    }
    let _ = std::fs::remove_dir_all(&baked);
}
