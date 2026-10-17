//! Reflection-probe light shares name animated lights by AnimatedBakedLights roster index, the
//! index every other animated section and the runtime descriptor buffer use.
//! See: context/lib/resource_management.md §5.2 · rendering_pipeline.md §7.3a (Live lighting)

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use postretro_level_format::alpha_lights::AlphaLightsSection;
use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::animated_light_chunks::AnimatedLightChunksSection;
use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;
use postretro_level_format::delta_sh_volumes::{AFFINITY_FACTOR, DeltaShVolumesSection};
use postretro_level_format::reflection_probes::{
    ReflectionProbeLightShare, ReflectionProbesSection,
};
use postretro_level_format::sh_volume::{
    ANIMATED_SLOT_NONE, AnimationDescriptor, OctahedralShVolumeSection,
};
use postretro_level_format::{SectionId, read_container, read_section_data};
use postretro_render_cpu::sh_compose::{AnimatedLightScaleDescriptor, animated_light_scale};

/// The fixture: a long sealed hall, a pool under one animated light. Its
/// header comment lists the entities.
const FIXTURE: &str = include_str!("fixtures/reflection_probe_lights.map");
/// Quake map units to engine meters.
const UNIT: f32 = 0.0254;
/// Every fixture light's `_falloff_range`, in meters.
const RANGE: f32 = 192.0 * UNIT;
/// The roster's lights by map origin: entity 3 (over the pool), entity 5
/// (bake-only, so no runtime record), entity 6 (sdf) and entity 7.
const ROSTER_MAP_ORIGINS: [[f32; 3]; 4] = [
    [-512.0, 0.0, 96.0],
    [256.0, 0.0, 96.0],
    [0.0, 0.0, 96.0],
    [512.0, 0.0, 96.0],
];
/// The roster index of the bake-only light: a runtime hole.
const BAKE_ONLY_SLOT: u32 = 1;

struct TempBuildDir(PathBuf);

impl TempBuildDir {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock must be after the Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "postretro-reflection-probes-compile-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(path.join("maps")).expect("create isolated compiler directory");
        Self(path)
    }
}

impl Drop for TempBuildDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn compile(temp: &TempBuildDir) -> PathBuf {
    let input = temp.0.join("maps").join("reflection_probe_lights.map");
    let output_path = temp.0.join("reflection_probe_lights.prl");
    std::fs::write(&input, FIXTURE).expect("write fixture map");
    let output = Command::new(env!("CARGO_BIN_EXE_prl-build"))
        .env("RUST_LOG", "warn")
        .arg(&input)
        .arg("-o")
        .arg(&output_path)
        .arg("--no-cache")
        .arg("--no-tui")
        .arg("--lightmap-density")
        .arg("0.25")
        .output()
        .expect("spawn prl-build");
    assert!(
        output.status.success(),
        "prl-build failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output_path
}

fn sections(prl: &Path) -> BTreeMap<u32, Vec<u8>> {
    let bytes = std::fs::read(prl).expect("read compiled PRL");
    let mut cursor = Cursor::new(bytes);
    let metadata = read_container(&mut cursor).expect("read PRL container");
    let ids: Vec<u32> = metadata.sections.iter().map(|e| e.section_id).collect();
    ids.into_iter()
        .map(|id| {
            let payload = read_section_data(&mut cursor, &metadata, id)
                .unwrap_or_else(|error| panic!("read section {id}: {error}"))
                .unwrap_or_else(|| panic!("section {id} is listed"));
            (id, payload)
        })
        .collect()
}

fn section(sections: &BTreeMap<u32, Vec<u8>>, id: SectionId) -> &[u8] {
    sections
        .get(&(id as u32))
        .unwrap_or_else(|| panic!("{id:?} is written; ids present: {:?}", sections.keys()))
}

fn engine(map: [f32; 3]) -> [f32; 3] {
    [map[1] * UNIT, map[2] * UNIT, -map[0] * UNIT]
}

/// Distance from `point` to the box `[min, max]`.
fn box_distance(point: [f32; 3], min: [f32; 3], max: [f32; 3]) -> f32 {
    (0..3)
        .map(|a| (min[a] - point[a]).max(point[a] - max[a]).max(0.0).powi(2))
        .sum::<f32>()
        .sqrt()
}

/// The runtime clamped evaluation's mean over one cycle, sampled where the
/// compiler samples it.
fn runtime_cycle_mean(descriptor: &AnimationDescriptor) -> [f32; 3] {
    const SAMPLES: u32 = 4096;
    let mut sum = [0.0f64; 3];
    for i in 0..SAMPLES {
        let value = animated_light_scale(
            Some(AnimatedLightScaleDescriptor {
                period: 1.0,
                phase: 0.0,
                base_color: descriptor.base_color,
                brightness: &descriptor.brightness,
                color: &descriptor.color,
                is_active: true,
            }),
            (i as f32 + 0.5) / SAMPLES as f32,
        );
        for (acc, v) in sum.iter_mut().zip(value) {
            *acc += f64::from(v);
        }
    }
    sum.map(|s| (s / f64::from(SAMPLES)) as f32)
}

// P10: on a map whose roster has a runtime hole (a bake-only light) and
// whose lights include static and dynamic lights the roster leaves out,
// share index r, weight-map light r, delta affinity light r and descriptor r
// all name the same light, and the descriptor-index tables are identity.
#[test]
fn share_weight_map_delta_and_descriptor_indices_name_the_same_light() {
    let temp = TempBuildDir::new();
    let prl = compile(&temp);
    let sections = sections(&prl);
    let alpha = AlphaLightsSection::from_bytes(section(&sections, SectionId::AlphaLights)).unwrap();
    let sh =
        OctahedralShVolumeSection::from_bytes(section(&sections, SectionId::OctahedralShVolume))
            .unwrap();
    let probes =
        ReflectionProbesSection::from_bytes(section(&sections, SectionId::ReflectionProbes))
            .unwrap();
    let roster = sh.animation_descriptors.len();
    assert_eq!(roster, ROSTER_MAP_ORIGINS.len());
    let origins = ROSTER_MAP_ORIGINS.map(engine);

    // The runtime lights: static, animated, dynamic, sdf animated, animated;
    // the bake-only light has no record, so no record maps to its slot.
    assert_eq!(sh.slot_for_map_light.len(), alpha.lights.len());
    assert_eq!(
        sh.slot_for_map_light,
        [ANIMATED_SLOT_NONE, 0, ANIMATED_SLOT_NONE, 2, 3]
    );
    assert!(!sh.slot_for_map_light.contains(&BAKE_ONLY_SLOT));
    for (record, &slot) in alpha.lights.iter().zip(&sh.slot_for_map_light) {
        if slot == ANIMATED_SLOT_NONE {
            continue;
        }
        // Descriptor `slot` is this light: its origin and its colour.
        let origin = record.origin.map(|v| v as f32);
        let expected = origins[slot as usize];
        assert!(
            box_distance(origin, expected, expected) < 1e-3,
            "slot {slot}"
        );
        let base = sh.animation_descriptors[slot as usize].base_color;
        for (b, color) in base.iter().zip(record.color) {
            assert!((b - color * record.intensity).abs() < 1e-5, "slot {slot}");
        }
    }

    // Weight-map light r lights only chunks within its reach of light r.
    let maps = AnimatedLightWeightMapsSection::from_bytes(section(
        &sections,
        SectionId::AnimatedLightWeightMaps,
    ))
    .unwrap();
    let chunks =
        AnimatedLightChunksSection::from_bytes(section(&sections, SectionId::AnimatedLightChunks))
            .unwrap();
    assert_eq!(maps.chunk_rects.len(), chunks.chunks.len());
    let mut weighted = [false; 4];
    for (rect, chunk) in maps.chunk_rects.iter().zip(&chunks.chunks) {
        let records = &maps.offset_counts
            [rect.texel_offset as usize..(rect.texel_offset + rect.width * rect.height) as usize];
        for entry in records {
            for light in
                &maps.texel_lights[entry.offset as usize..(entry.offset + entry.count) as usize]
            {
                let r = light.light_index as usize;
                assert!(r < roster);
                weighted[r] = true;
                let d = box_distance(origins[r], chunk.aabb_min, chunk.aabb_max);
                assert!(
                    d <= RANGE + 0.01,
                    "weight-map light {r} lights a chunk {d} m away"
                );
            }
        }
    }
    // The sdf light has no weight map (its direct resolves at runtime); the
    // others light the floor beneath them.
    assert_eq!(weighted, [true, true, false, true]);

    // Delta light r sits only in affinity cells within its reach.
    let delta =
        DeltaShVolumesSection::from_bytes(section(&sections, SectionId::DeltaShVolumes)).unwrap();
    assert_eq!(
        delta.animation_descriptor_indices,
        (0..roster as u32).collect::<Vec<_>>(),
        "id 27's descriptor table is the identity"
    );
    if let Some(bytes) = sections.get(&(SectionId::AnimatedDirectShDeltaVolumes as u32)) {
        let animated = AnimatedDirectShDeltaVolumesSection::from_bytes(bytes).unwrap();
        assert_eq!(
            animated.animation_descriptor_indices,
            (0..animated.animation_descriptor_indices.len() as u32).collect::<Vec<_>>(),
            "id 45's descriptor table is the identity"
        );
    }
    let edge = sh.cell_size.map(|c| c * f32::from(AFFINITY_FACTOR));
    let [ax, ay, _] = delta.affinity_dims;
    let mut delta_lights = [false; 4];
    for cell in 0..delta.affinity_cell_count() {
        let coord = [
            cell as u32 % ax,
            (cell as u32 / ax) % ay,
            cell as u32 / (ax * ay),
        ];
        let min: [f32; 3] = std::array::from_fn(|a| sh.grid_origin[a] + coord[a] as f32 * edge[a]);
        let max: [f32; 3] = std::array::from_fn(|a| min[a] + edge[a] - sh.cell_size[a]);
        for &light in &delta.affinity_lights
            [delta.affinity_offsets[cell] as usize..delta.affinity_offsets[cell + 1] as usize]
        {
            let r = light as usize;
            delta_lights[r] = true;
            let d = box_distance(origins[r], min, max);
            assert!(
                d <= RANGE + 0.01,
                "delta light {r} reaches a cell {d} m away"
            );
        }
    }
    assert_eq!(
        delta_lights, [true; 4],
        "every roster light bounces, the hole included"
    );

    // Share entry r carries descriptor r's reference, and the pool's own
    // light, roster 0, holds the largest share.
    assert_eq!(probes.probes.len(), 1);
    let lights: Vec<_> = probes.probes[0]
        .lights
        .iter()
        .filter(|l| l.is_used())
        .collect();
    assert!(!lights.is_empty());
    for light in &lights {
        let r = light.roster_index as usize;
        assert!(r < roster);
        let mean = runtime_cycle_mean(&sh.animation_descriptors[r]);
        for c in 0..3 {
            assert!(
                (light.reference[c] - mean[c]).abs() < 1e-5,
                "entry {r}: {light:?} vs {mean:?}"
            );
        }
    }
    let share_sum = |l: &ReflectionProbeLightShare| l.shares.iter().flatten().sum::<f32>();
    let largest = lights
        .iter()
        .copied()
        .max_by(|a, b| share_sum(a).total_cmp(&share_sum(b)))
        .unwrap();
    assert_eq!(largest.roster_index, 0, "{lights:?}");
}
