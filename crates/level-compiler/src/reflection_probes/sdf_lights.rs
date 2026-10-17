// Reflection-probe SDF lights: the runtime's per-fragment top-four selection and its diffuse term, on the CPU.
// See: context/lib/rendering_pipeline.md §4 (Lighting architecture map) · shaders sdf_light_select.wgsl, forward.wgsl

use glam::Vec3;
use postretro_level_format::alpha_lights::{
    AlphaFalloffModel, AlphaLightType, AlphaLightsSection, AlphaShadowType,
};
use postretro_level_format::chunk_light_list::ChunkLightListSection;

/// `SDF_SELECT_K` in `sdf_light_select.wgsl`: at most this many sdf lights
/// shade a fragment. Pinned against the shader text and against
/// `chunk_light_list_bake::SDF_SHADOW_K` by tests.
pub(crate) const SDF_SELECT_K: usize = 4;

/// `spec_buffer.rs` light-type discriminants (`cone_dir_and_type.w`).
const SPEC_LIGHT_TYPE_DIRECTIONAL: u32 = 2;

/// One runtime `SpecLight`, as `postretro_lighting::spec_buffer::pack_spec_lights`
/// packs it from the loaded light: the record the forward pass's sdf diffuse
/// loop and `select_sdf_lights` read.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SpecLight {
    pub position: Vec3,
    /// `falloff_range`; 0 means unattenuated.
    pub range: f32,
    /// `color × intensity`, the static value the spec buffer uploads once.
    pub color: Vec3,
    pub sdf: bool,
    /// The packed aim: a spot's normalized cone axis, a directional light's
    /// direction, zero for a point light.
    pub aim: Vec3,
    pub light_type: u32,
    pub cos_inner: f32,
    pub cos_outer: f32,
    /// 0 Linear, 1 InverseDistance, 2 InverseSquared.
    pub falloff_model: u32,
}

impl SpecLight {
    fn from_record(r: &postretro_level_format::alpha_lights::AlphaLightRecord) -> Self {
        let (aim, light_type, cos_inner, cos_outer) = match r.light_type {
            AlphaLightType::Spot => (
                Vec3::from(r.cone_direction).normalize_or_zero(),
                1,
                r.cone_angle_inner.cos(),
                r.cone_angle_outer.cos(),
            ),
            AlphaLightType::Directional => (
                Vec3::from(r.cone_direction).normalize_or(Vec3::NEG_Y),
                SPEC_LIGHT_TYPE_DIRECTIONAL,
                1.0,
                -1.0,
            ),
            AlphaLightType::Point => (Vec3::ZERO, 0, 1.0, -1.0),
        };
        Self {
            position: Vec3::new(r.origin[0] as f32, r.origin[1] as f32, r.origin[2] as f32),
            range: r.falloff_range,
            color: Vec3::from(r.color) * r.intensity,
            sdf: r.shadow_type == AlphaShadowType::Sdf,
            aim,
            light_type,
            cos_inner,
            cos_outer,
            falloff_model: match r.falloff_model {
                AlphaFalloffModel::Linear => 0,
                AlphaFalloffModel::InverseDistance => 1,
                AlphaFalloffModel::InverseSquared => 2,
            },
        }
    }

    /// `positional_cone_aim`: the sdf loops place every light at its origin,
    /// so a directional light keeps a zero aim.
    fn positional_aim(&self) -> Vec3 {
        if self.light_type == SPEC_LIGHT_TYPE_DIRECTIONAL {
            Vec3::ZERO
        } else {
            self.aim
        }
    }
}

/// The level's static spec lights (every non-dynamic AlphaLights record, in
/// order: the index the chunk light list names) and the chunk grid that
/// windows the selection. `_bake_only` lights never reach AlphaLights, so they
/// are absent here as they are at runtime.
pub(crate) struct SdfLights {
    spec: Vec<SpecLight>,
    grid: Option<ChunkGrid>,
}

struct ChunkGrid {
    origin: Vec3,
    cell_size: f32,
    dims: [u32; 3],
    offsets: Vec<(u32, u32)>,
    indices: Vec<u32>,
}

/// The selected lights, strongest first, as spec indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct SdfSelection {
    pub indices: [u32; SDF_SELECT_K],
    pub count: usize,
}

impl SdfSelection {
    pub(crate) fn lights(&self) -> &[u32] {
        &self.indices[..self.count]
    }
}

impl SdfLights {
    pub(crate) fn new(
        alpha_lights: &AlphaLightsSection,
        chunk_list: &ChunkLightListSection,
    ) -> Self {
        let spec: Vec<SpecLight> = alpha_lights
            .lights
            .iter()
            .filter(|r| !r.is_dynamic)
            .map(SpecLight::from_record)
            .collect();
        // `ChunkGrid::from_section` uploads the section whole; `has_grid == 0`
        // (the placeholder) selects the full-buffer scan.
        let grid = (chunk_list.has_grid != 0).then(|| ChunkGrid {
            origin: Vec3::from(chunk_list.grid_origin),
            cell_size: chunk_list.cell_size.max(1.0e-6),
            dims: chunk_list.grid_dimensions,
            offsets: chunk_list
                .offsets
                .iter()
                .map(|e| (e.offset, e.count))
                .collect(),
            indices: chunk_list.light_indices.clone(),
        });
        Self { spec, grid }
    }

    #[cfg(test)]
    pub(crate) fn from_parts(spec: Vec<SpecLight>, chunk_list: &ChunkLightListSection) -> Self {
        let mut lights = Self::new(&AlphaLightsSection::default(), chunk_list);
        lights.spec = spec;
        lights
    }

    /// No sdf light in the level: the capture skips the selection.
    pub(crate) fn is_empty(&self) -> bool {
        !self.spec.iter().any(|l| l.sdf)
    }

    pub(crate) fn spec(&self, index: u32) -> &SpecLight {
        &self.spec[index as usize]
    }

    /// The bytes the capture reads from these lights, for the stage key: each
    /// sdf light with its spec index, the spec count and the chunk grid.
    pub(crate) fn key_bytes(&self, chunk_list: &ChunkLightListSection) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(self.spec.len() as u64).to_le_bytes());
        for (index, light) in self.spec.iter().enumerate().filter(|(_, l)| l.sdf) {
            out.extend_from_slice(&(index as u32).to_le_bytes());
            for v in light
                .position
                .to_array()
                .into_iter()
                .chain([light.range])
                .chain(light.color.to_array())
                .chain(light.aim.to_array())
                .chain([light.cos_inner, light.cos_outer])
            {
                out.extend_from_slice(&v.to_le_bytes());
            }
            out.extend_from_slice(&light.light_type.to_le_bytes());
            out.extend_from_slice(&light.falloff_model.to_le_bytes());
        }
        out.extend_from_slice(&chunk_list.to_bytes());
        out
    }

    /// `select_sdf_lights`: up to [`SDF_SELECT_K`] sdf lights influencing
    /// `world`, by influence descending, then spec index ascending, from the
    /// fragment's chunk-grid window (the whole spec buffer without a grid).
    pub(crate) fn select(&self, world: Vec3) -> SdfSelection {
        let mut selection = SdfSelection {
            indices: [u32::MAX; SDF_SELECT_K],
            count: 0,
        };
        let mut influences = [0.0f32; SDF_SELECT_K];
        let mut consider = |index: u32| {
            let Some(light) = self.spec.get(index as usize) else {
                return;
            };
            if !light.sdf {
                return;
            }
            let influence = select_influence(light, world);
            if influence <= 0.0 || influence.is_nan() {
                return;
            }
            let Some(at) = (0..SDF_SELECT_K).find(|&s| {
                s >= selection.count
                    || influence > influences[s]
                    || (influence == influences[s] && index < selection.indices[s])
            }) else {
                return;
            };
            for s in (at + 1..SDF_SELECT_K).rev() {
                selection.indices[s] = selection.indices[s - 1];
                influences[s] = influences[s - 1];
            }
            selection.indices[at] = index;
            influences[at] = influence;
            selection.count = (selection.count + 1).min(SDF_SELECT_K);
        };
        match &self.grid {
            None => (0..self.spec.len() as u32).for_each(&mut consider),
            Some(grid) => {
                let cell = ((world - grid.origin) / grid.cell_size).floor();
                let dims = grid.dims.map(|d| d as f32);
                if (0..3).all(|a| cell[a] >= 0.0 && cell[a] < dims[a]) {
                    let [x, y, z] = [cell.x as u32, cell.y as u32, cell.z as u32];
                    let ci = (z * grid.dims[0] * grid.dims[1] + y * grid.dims[0] + x) as usize;
                    if let Some(&(offset, count)) = grid.offsets.get(ci) {
                        for j in 0..count {
                            if let Some(&index) = grid.indices.get((offset + j) as usize) {
                                consider(index);
                            }
                        }
                    }
                }
            }
        }
        selection
    }

    /// The selected lights' diffuse at a capture hit, `forward.wgsl`'s sdf
    /// loop with the shading normal at the face normal: each light at its
    /// static spec colour × N·L × falloff × cone × visibility. `visible`
    /// answers the shadow ray toward the light's origin, where the loop and
    /// the SDF visibility trace place every light.
    pub(crate) fn direct(
        &self,
        selection: &SdfSelection,
        world: Vec3,
        normal: Vec3,
        visible: impl Fn(Vec3) -> bool,
    ) -> Vec3 {
        let mut sum = Vec3::ZERO;
        for &index in selection.lights() {
            let light = self.spec(index);
            let to_light = light.position - world;
            let dist = to_light.length();
            if light.range > 0.0 && dist > light.range {
                continue;
            }
            let l = to_light / dist.max(0.0001);
            let n_dot_l = normal.dot(l);
            if n_dot_l <= 0.0 {
                continue;
            }
            let atten = if light.range > 0.0 {
                light_eval_falloff(dist, light.range, light.falloff_model)
            } else {
                1.0
            };
            let cone =
                cone_attenuation_cos(l, light.positional_aim(), light.cos_inner, light.cos_outer);
            if atten * cone <= 0.0 || !visible(light.position) {
                continue;
            }
            sum += light.color * (n_dot_l * atten * cone);
        }
        sum
    }
}

/// `sdf_select_influence`: the shading loop's falloff (none at range 0) times
/// the light's brightest channel.
fn select_influence(light: &SpecLight, world: Vec3) -> f32 {
    let dist = (light.position - world).length();
    let atten = if light.range > 0.0 {
        light_eval_falloff(dist, light.range, light.falloff_model)
    } else {
        1.0
    };
    atten * light.color.max_element()
}

/// `light_falloff.wgsl` `light_eval_falloff`.
pub(crate) fn light_eval_falloff(distance: f32, range: f32, model: u32) -> f32 {
    match model {
        0 => (1.0 - distance / range.max(0.001)).max(0.0),
        1 => {
            if distance > range.max(0.0001) {
                0.0
            } else {
                1.0 / distance.max(0.0001)
            }
        }
        2 => {
            if distance > range.max(0.0001) {
                0.0
            } else {
                1.0 / (distance * distance).max(0.0001)
            }
        }
        _ => 0.0,
    }
}

/// `forward.wgsl` `cone_attenuation_cos`: WGSL `smoothstep(cos_outer,
/// cos_inner, dot(−L, aim))`. A point light packs a zero aim and the cutoffs
/// (1, −1), so it reads `smoothstep(−1, 1, 0)` = 0.5, as the runtime draws it.
/// A degenerate cone (equal cutoffs) reads 0 rather than NaN.
fn cone_attenuation_cos(l: Vec3, aim: Vec3, cos_inner: f32, cos_outer: f32) -> f32 {
    let x = (-l).dot(aim);
    let t = ((x - cos_outer) / (cos_inner - cos_outer)).clamp(0.0, 1.0);
    let value = t * t * (3.0 - 2.0 * t);
    if value.is_nan() { 0.0 } else { value }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::chunk_light_list::ChunkEntry;

    const SELECT_WGSL: &str = include_str!("../../../renderer/src/shaders/sdf_light_select.wgsl");
    const FALLOFF_WGSL: &str = include_str!("../../../renderer/src/shaders/light_falloff.wgsl");
    const FORWARD_WGSL: &str = include_str!("../../../renderer/src/shaders/forward.wgsl");

    fn squash(source: &str) -> String {
        source.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    fn wgsl_select_k() -> usize {
        let line = SELECT_WGSL
            .lines()
            .find(|l| l.trim_start().starts_with("const SDF_SELECT_K: u32 ="))
            .expect("sdf_light_select.wgsl declares SDF_SELECT_K");
        line.split('=')
            .nth(1)
            .and_then(|v| {
                v.trim()
                    .trim_end_matches(';')
                    .trim_end_matches('u')
                    .parse()
                    .ok()
            })
            .expect("SDF_SELECT_K is an integer literal")
    }

    #[test]
    fn select_k_matches_the_shader_and_the_chunk_list_budget() {
        assert_eq!(SDF_SELECT_K, wgsl_select_k());
        assert_eq!(crate::chunk_light_list_bake::SDF_SHADOW_K, wgsl_select_k());
    }

    /// The CPU selection mirrors these shader statements; a change to any of
    /// them must change the mirror with it.
    #[test]
    fn selection_mirror_pins_the_shader_ranking_window_and_influence() {
        let select = squash(SELECT_WGSL);
        for statement in [
            "return sl.color_and_pad.w > 0.5;",
            "let atten = select( 1.0, light_eval_falloff(dist, range, u32(round(sl.cone_cos.w))), range > 0.0, );",
            "let peak = max(sl.color_and_pad.x, max(sl.color_and_pad.y, sl.color_and_pad.z)); return atten * peak;",
            "let cell = vec3<i32>(floor(local / chunk_grid.cell_size));",
            "let ci = u32(cell.z) * chunk_grid.dims.x * chunk_grid.dims.y + u32(cell.y) * chunk_grid.dims.x + u32(cell.x);",
            "if (influence <= 0.0) { continue; }",
            "let outranks = influence > infl[s] || (influence == infl[s] && light_idx < indices[s]);",
        ] {
            assert!(
                select.contains(statement),
                "sdf_light_select.wgsl lost: {statement}"
            );
        }
        let falloff = squash(FALLOFF_WGSL);
        for statement in [
            "let r = max(range, 0.001); return max(1.0 - distance / r, 0.0);",
            "let r = max(range, 0.0001); if distance > r { return 0.0; } return 1.0 / max(distance, 0.0001);",
            "let d2 = max(distance * distance, 0.0001); return 1.0 / d2;",
        ] {
            assert!(
                falloff.contains(statement),
                "light_falloff.wgsl lost: {statement}"
            );
        }
    }

    /// The diffuse mirror's statements in the forward sdf loop.
    #[test]
    fn diffuse_mirror_pins_the_forward_sdf_loop() {
        let forward = squash(FORWARD_WGSL);
        for statement in [
            "if range > 0.0 && dist > range { continue; } let L = to_light / max(dist, 0.0001); let n_dot_l = dot(N_shade, L); if n_dot_l <= 0.0 { continue; }",
            "let cone = cone_attenuation_cos(L, positional_cone_aim(sl), sl.cone_cos.x, sl.cone_cos.y);",
            "static_direct = static_direct + sl.color_and_pad.xyz * (n_dot_l * atten * cone * visibility);",
            "let cos_angle = dot(-L, aim); return smoothstep(cos_outer, cos_inner, cos_angle);",
            "return select(sl.cone_dir_and_type.xyz, vec3<f32>(0.0), directional);",
        ] {
            assert!(
                forward.contains(statement),
                "forward.wgsl lost: {statement}"
            );
        }
    }

    fn sdf_point(position: [f32; 3], range: f32, brightness: f32) -> SpecLight {
        SpecLight {
            position: Vec3::from(position),
            range,
            color: Vec3::splat(brightness),
            sdf: true,
            aim: Vec3::ZERO,
            light_type: 0,
            cos_inner: 1.0,
            cos_outer: -1.0,
            falloff_model: 0,
        }
    }

    /// An independent reference: rank every eligible light by the same key
    /// with a full sort, keep the first four.
    fn reference_select(lights: &[SpecLight], window: &[u32], world: Vec3) -> Vec<u32> {
        let mut ranked: Vec<(f32, u32)> = window
            .iter()
            .filter(|&&i| lights[i as usize].sdf)
            .map(|&i| (select_influence(&lights[i as usize], world), i))
            .filter(|(influence, _)| *influence > 0.0)
            .collect();
        ranked.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        ranked
            .into_iter()
            .take(SDF_SELECT_K)
            .map(|(_, i)| i)
            .collect()
    }

    #[test]
    fn selection_keeps_the_four_strongest_with_ties_to_the_lower_index() {
        let mut lights: Vec<SpecLight> = (0..7)
            .map(|i| sdf_point([i as f32, 0.0, 0.0], 20.0, 1.0 + (i % 3) as f32))
            .collect();
        // A non-sdf light that would outrank them all.
        lights.push(SpecLight {
            sdf: false,
            ..sdf_point([0.0; 3], 20.0, 50.0)
        });
        // Two exact ties in influence: same distance, same brightness.
        lights.push(sdf_point([0.0, 3.0, 0.0], 20.0, 3.0));
        lights.push(sdf_point([0.0, -3.0, 0.0], 20.0, 3.0));
        let set = SdfLights::from_parts(lights.clone(), &ChunkLightListSection::placeholder());
        for world in [
            Vec3::ZERO,
            Vec3::new(3.0, 1.0, 0.5),
            Vec3::new(6.0, 0.0, 0.0),
        ] {
            let picked = set.select(world);
            let all: Vec<u32> = (0..lights.len() as u32).collect();
            assert_eq!(
                picked.lights(),
                reference_select(&lights, &all, world),
                "{world}"
            );
        }
        // At the origin: light 2, the tied pair 8 and 9 in index order, then
        // light 5; the brighter non-sdf light 7 never enters.
        assert_eq!(set.select(Vec3::ZERO).lights(), [2, 8, 9, 5]);
    }

    #[test]
    fn a_tie_at_the_fourth_slot_keeps_the_lower_index() {
        let lights: Vec<SpecLight> = [
            [1.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, -1.0],
        ]
        .map(|p| sdf_point(p, 10.0, 1.0))
        .to_vec();
        // Fed in reverse index order through a chunk window, the result is
        // still the four lowest indices.
        let grid = ChunkLightListSection {
            grid_origin: [-8.0; 3],
            cell_size: 16.0,
            grid_dimensions: [1, 1, 1],
            has_grid: 1,
            per_chunk_cap: 64,
            offsets: vec![ChunkEntry {
                offset: 0,
                count: 6,
            }],
            light_indices: vec![5, 4, 3, 2, 1, 0],
        };
        let set = SdfLights::from_parts(lights, &grid);
        assert_eq!(set.select(Vec3::ZERO).lights(), [0, 1, 2, 3]);
    }

    #[test]
    fn selection_reads_only_its_chunk_window() {
        let lights: Vec<SpecLight> = (0..3)
            .map(|i| sdf_point([i as f32, 0.0, 0.0], 50.0, 1.0))
            .collect();
        let grid = ChunkLightListSection {
            grid_origin: [-4.0, -4.0, -4.0],
            cell_size: 8.0,
            grid_dimensions: [2, 1, 1],
            has_grid: 1,
            per_chunk_cap: 64,
            offsets: vec![
                ChunkEntry {
                    offset: 0,
                    count: 2,
                },
                ChunkEntry {
                    offset: 2,
                    count: 1,
                },
            ],
            light_indices: vec![2, 0, 1],
        };
        let set = SdfLights::from_parts(lights, &grid);
        assert_eq!(set.select(Vec3::ZERO).lights(), [0, 2]);
        assert_eq!(set.select(Vec3::new(5.0, 0.0, 0.0)).lights(), [1]);
        // Outside the grid: nothing.
        assert_eq!(set.select(Vec3::new(50.0, 0.0, 0.0)).count, 0);
    }

    #[test]
    fn falloff_models_match_the_shared_helper() {
        assert!((light_eval_falloff(2.0, 8.0, 0) - 0.75).abs() < 1e-6);
        assert_eq!(light_eval_falloff(9.0, 8.0, 0), 0.0);
        assert!((light_eval_falloff(2.0, 8.0, 1) - 0.5).abs() < 1e-6);
        assert_eq!(light_eval_falloff(9.0, 8.0, 1), 0.0);
        assert!((light_eval_falloff(2.0, 8.0, 2) - 0.25).abs() < 1e-6);
        assert_eq!(light_eval_falloff(2.0, 8.0, 7), 0.0);
    }

    #[test]
    fn diffuse_is_lambert_falloff_and_cone_and_drops_occluded_lights() {
        let light = sdf_point([0.0, 2.0, 0.0], 4.0, 2.0);
        let set = SdfLights::from_parts(vec![light], &ChunkLightListSection::placeholder());
        let selection = set.select(Vec3::ZERO);
        let lit = set.direct(&selection, Vec3::ZERO, Vec3::Y, |_| true);
        // N·L 1, linear falloff 0.5, the point light's 0.5 cone.
        assert!(
            (lit - Vec3::splat(2.0 * 0.5 * 0.5)).length() < 1e-6,
            "{lit}"
        );
        assert_eq!(
            set.direct(&selection, Vec3::ZERO, Vec3::Y, |_| false),
            Vec3::ZERO
        );
        // Facing away: no light.
        assert_eq!(
            set.direct(&selection, Vec3::ZERO, Vec3::NEG_Y, |_| true),
            Vec3::ZERO
        );
    }

    #[test]
    fn spec_lights_follow_the_runtime_pack_and_skip_dynamic_lights() {
        use postretro_level_format::alpha_lights::AlphaLightRecord;
        let record = |light_type, is_dynamic, shadow_type| AlphaLightRecord {
            origin: [1.0, 2.0, 3.0],
            light_type,
            intensity: 2.0,
            color: [0.5, 0.25, 1.0],
            falloff_model: AlphaFalloffModel::InverseSquared,
            falloff_range: 6.0,
            cone_angle_inner: 0.3,
            cone_angle_outer: 0.6,
            cone_direction: [0.0, -2.0, 0.0],
            is_dynamic,
            casts_entity_shadows: false,
            leaf_index: 0,
            shadow_type,
        };
        let section = AlphaLightsSection {
            lights: vec![
                record(AlphaLightType::Point, true, AlphaShadowType::Sdf),
                record(AlphaLightType::Spot, false, AlphaShadowType::Sdf),
                record(
                    AlphaLightType::Directional,
                    false,
                    AlphaShadowType::StaticLightMap,
                ),
            ],
        };
        let set = SdfLights::new(&section, &ChunkLightListSection::placeholder());
        assert_eq!(set.spec.len(), 2, "the dynamic light has no spec slot");
        let spot = set.spec(0);
        assert!(spot.sdf);
        assert_eq!(spot.color, Vec3::new(1.0, 0.5, 2.0));
        assert_eq!(spot.aim, Vec3::NEG_Y);
        assert_eq!(
            (spot.cos_inner, spot.cos_outer),
            (0.3f32.cos(), 0.6f32.cos())
        );
        assert_eq!(spot.falloff_model, 2);
        let sun = set.spec(1);
        assert!(!sun.sdf);
        assert_eq!(sun.positional_aim(), Vec3::ZERO);
    }
}
