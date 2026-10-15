// Fluid reflection probes: the probe table's GPU layout, the per-vertex probe
// index, and the GPU-free CPU reference for `fluid_surface.wgsl`'s probe term,
// its live-lighting ratio included (the distance search is `reflection_search`).
// See: context/lib/rendering_pipeline.md §7.3a · context/lib/resource_management.md §5

use bytemuck::{Pod, Zeroable};
use glam::{Vec2, Vec3, Vec4};
use postretro_level_format::environment_volumes::EnvironmentVolumeRecord;
use postretro_level_format::octahedral::encode_unquantized_uv;
use postretro_level_format::reflection_probes::{
    MAX_REFLECTION_PROBES, REFLECTION_PROBE_LIGHTS_PER_PROBE, REFLECTION_PROBE_LOBE_COUNT,
    REFLECTION_PROBE_NO_LIGHT, ReflectionProbeLightShare, ReflectionProbeRecord,
    ReflectionProbesSection, reflection_probe_lobe_weights,
};

use super::FluidFaceDriftMap;
use super::reflection_search::reflection_search;
use super::reflection_style::{FluidTexelAxes, reflection_point};
use crate::frame_uniforms::LightTermMask;
use crate::sh_compose::{AnimatedLightScaleDescriptor, animated_light_scale};

// The shader mirrors each reference function below, `reflection_fragment`
// inlined as `fs_main`'s tail after the fog composite, and each constant but
// the packing-only ones; `render/fluid_surface_tests.rs` pins them.

/// Schlick base reflectance per unit of `surfaceReflectivity`: 1 is water's
/// 0.02, and [`FLUID_REFLECTIVITY_MAX`] tops at a mirror (F0 = 1).
pub const FLUID_REFLECTION_BASE_F0: f32 = 0.02;
/// Largest `surfaceReflectivity`; packing clamps to it. Packing-only.
pub const FLUID_REFLECTIVITY_MAX: f32 = 50.0;
/// Per-vertex probe index of a face no binding names: it reflects nothing.
pub const FLUID_REFLECTION_NO_PROBE: u32 = u32::MAX;
/// Lighting-mask bits the probe term needs all of, since the capture bakes
/// every term together: `LightTermMask::ALL`, read literally.
pub const FLUID_REFLECTION_ALL_TERMS: u32 = LightTermMask::ALL.bits();
/// Below this a reflected-ray component is treated as parallel to its slab,
/// and a lookup direction this short (squared, m²) has no direction.
pub const FLUID_REFLECTION_DIRECTION_EPSILON: f32 = 1.0e-6;
/// Floor on the union coverage before the composite divides by it.
pub const FLUID_REFLECTION_COVERAGE_EPSILON: f32 = 1.0e-4;
/// Floor on a stored reference channel before the live ratio divides by it;
/// a zero channel's term is dropped outright, so this only keeps the divide
/// off zero.
pub const FLUID_REFLECTION_REFERENCE_FLOOR: f32 = 1.0e-30;
/// Largest value of one unorm10 share field.
pub const FLUID_REFLECTION_UNORM10_MAX: u32 = 1023;

/// u32 words of one share entry's lobe shares, padded to whole `vec4<u32>`.
pub const FLUID_PROBE_SHARE_WORDS: usize = REFLECTION_PROBE_LOBE_COUNT.div_ceil(4) * 4;

/// One share entry as the shader's `FluidProbeLight` reads it: the RGB
/// reference beside the roster index, then one unorm10×3 word per lobe.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct FluidProbeLightGpu {
    pub reference: [f32; 3],
    /// Animated-light roster index; `REFLECTION_PROBE_NO_LIGHT` is unused.
    pub roster_index: u32,
    /// Per lobe, RGB share as unorm10×3 ([`pack_unorm10x3`]); the tail pads.
    pub shares: [u32; FLUID_PROBE_SHARE_WORDS],
}

/// One probe as the shader's `FluidProbe` reads it: three `vec4` (box min,
/// box max, capture point with its array layer in `w`), then its K entries.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct FluidProbeGpu {
    pub box_min: [f32; 3],
    _pad0: u32,
    pub box_max: [f32; 3],
    _pad1: u32,
    pub capture_point: [f32; 3],
    /// Array layer of the probe's tile.
    pub layer: u32,
    pub lights: [FluidProbeLightGpu; REFLECTION_PROBE_LIGHTS_PER_PROBE],
}

/// The table's leading `vec4<u32>`: the tile layout every probe shares.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct FluidProbeTableHeader {
    pub tile_dimension: u32,
    pub tile_border: u32,
    pub probe_count: u32,
    _pad: u32,
}

/// Byte stride of one [`FluidProbeLightGpu`].
pub const FLUID_PROBE_LIGHT_STRIDE: usize = 48;
/// Byte stride of one [`FluidProbeGpu`].
pub const FLUID_PROBE_STRIDE: usize = 240;
/// Byte size of the table's header.
pub const FLUID_PROBE_TABLE_HEADER_BYTES: usize = 16;
/// The whole share table, laid out at the probe cap: one uniform binding,
/// written once at install.
pub const FLUID_PROBE_TABLE_BYTES: usize =
    FLUID_PROBE_TABLE_HEADER_BYTES + MAX_REFLECTION_PROBES * FLUID_PROBE_STRIDE;

const _: () = assert!(std::mem::size_of::<FluidProbeLightGpu>() == FLUID_PROBE_LIGHT_STRIDE);
const _: () = assert!(std::mem::size_of::<FluidProbeGpu>() == FLUID_PROBE_STRIDE);
const _: () =
    assert!(std::mem::size_of::<FluidProbeTableHeader>() == FLUID_PROBE_TABLE_HEADER_BYTES);
// Uniform-address-space array elements stride by whole vec4.
const _: () = assert!(FLUID_PROBE_STRIDE.is_multiple_of(16));
// The cap fits the portable array-layer floor, and the table at the cap fits
// one 64 KiB uniform binding (the WebGPU default and D3D's constant-buffer
// ceiling).
const _: () = assert!(MAX_REFLECTION_PROBES <= 256);
const _: () = assert!(FLUID_PROBE_TABLE_BYTES <= 64 * 1024);

/// RGB in [0, 1] as three 10-bit unorm fields, red lowest. Non-finite
/// channels pack as 0, so a NaN share never reaches the shader.
pub fn pack_unorm10x3(rgb: [f32; 3]) -> u32 {
    let quantize = |channel: f32| -> u32 {
        if channel.is_finite() {
            (channel.clamp(0.0, 1.0) * 1023.0 + 0.5) as u32
        } else {
            0
        }
    };
    quantize(rgb[0]) | (quantize(rgb[1]) << 10) | (quantize(rgb[2]) << 20)
}

impl FluidProbeLightGpu {
    pub fn pack(share: &ReflectionProbeLightShare) -> Self {
        let mut shares = [0; FLUID_PROBE_SHARE_WORDS];
        for (word, lobe) in shares.iter_mut().zip(&share.shares) {
            *word = pack_unorm10x3(*lobe);
        }
        Self {
            reference: share.reference,
            roster_index: share.roster_index,
            shares,
        }
    }
}

impl FluidProbeGpu {
    pub fn pack(probe: &ReflectionProbeRecord) -> Self {
        Self {
            box_min: probe.box_min,
            _pad0: 0,
            box_max: probe.box_max,
            _pad1: 0,
            capture_point: probe.capture_point,
            layer: probe.layer,
            lights: probe.lights.map(|share| FluidProbeLightGpu::pack(&share)),
        }
    }
}

/// Whether the renderer can install `section`'s probes: some probe, no more
/// than the cap. The loader already refuses anything else; a hand-built
/// section past it reflects nothing rather than overrunning the table.
pub fn usable_probe_section(
    section: Option<&ReflectionProbesSection>,
) -> Option<&ReflectionProbesSection> {
    section.filter(|section| {
        !section.probes.is_empty() && section.probes.len() <= MAX_REFLECTION_PROBES
    })
}

/// The share table's bytes, [`FLUID_PROBE_TABLE_BYTES`] long: header, then
/// every probe in record order, zeros past the last. No section packs all
/// zeros, which no fragment reads, since every probe index is then
/// [`FLUID_REFLECTION_NO_PROBE`].
pub fn pack_probe_table(section: Option<&ReflectionProbesSection>) -> Vec<u8> {
    let mut bytes = vec![0u8; FLUID_PROBE_TABLE_BYTES];
    let Some(section) = usable_probe_section(section) else {
        return bytes;
    };
    let header = FluidProbeTableHeader {
        tile_dimension: section.tile_dimension,
        tile_border: section.tile_border,
        probe_count: section.probes.len() as u32,
        _pad: 0,
    };
    bytes[..FLUID_PROBE_TABLE_HEADER_BYTES].copy_from_slice(bytemuck::bytes_of(&header));
    for (index, probe) in section.probes.iter().enumerate() {
        let start = FLUID_PROBE_TABLE_HEADER_BYTES + index * FLUID_PROBE_STRIDE;
        bytes[start..start + FLUID_PROBE_STRIDE]
            .copy_from_slice(bytemuck::bytes_of(&FluidProbeGpu::pack(probe)));
    }
    bytes
}

/// Per-vertex probe indices for the mesh `face_maps` describe (one map per
/// surface face of every volume, in record order, as
/// [`super::FluidSurfaceMesh::face_drift_maps`]). Every vertex a bound face
/// owns, any added centroid included, takes its binding's probe; every other
/// vertex takes [`FLUID_REFLECTION_NO_PROBE`]. A binding naming a face or
/// probe that does not exist binds nothing.
pub fn fill_probe_indices(
    volumes: &[EnvironmentVolumeRecord],
    face_maps: &[Option<FluidFaceDriftMap>],
    section: Option<&ReflectionProbesSection>,
    indices: &mut [u32],
) {
    indices.fill(FLUID_REFLECTION_NO_PROBE);
    let Some(section) = usable_probe_section(section) else {
        return;
    };
    let mut face_start = Vec::with_capacity(volumes.len());
    let mut faces = 0usize;
    for volume in volumes {
        face_start.push(faces);
        faces += volume.surface_faces.len();
    }
    for binding in &section.bindings {
        let volume = binding.volume as usize;
        let face = binding.face as usize;
        let (Some(&start), Some(record)) = (face_start.get(volume), volumes.get(volume)) else {
            continue;
        };
        if face >= record.surface_faces.len() || binding.probe as usize >= section.probes.len() {
            continue;
        }
        let Some(Some(map)) = face_maps.get(start + face) else {
            continue;
        };
        let first = map.vertex_start as usize;
        if let Some(vertices) = indices.get_mut(first..first + map.vertex_count as usize) {
            vertices.fill(binding.probe);
        }
    }
}

/// Whether any surface face of installed volume `volume` binds a probe.
/// Bindings ascend by (volume, face), so this is one binary search.
pub fn volume_has_bound_face(section: Option<&ReflectionProbesSection>, volume: usize) -> bool {
    let Some(section) = usable_probe_section(section) else {
        return false;
    };
    let first = section
        .bindings
        .partition_point(|binding| (binding.volume as usize) < volume);
    section
        .bindings
        .get(first)
        .is_some_and(|binding| binding.volume as usize == volume)
}

/// Whether the frame's lighting mask draws the probe term: every lighting
/// bit on, since the capture bakes every term together.
pub fn reflection_mask_on(mask: u32) -> bool {
    (mask & FLUID_REFLECTION_ALL_TERMS) == FLUID_REFLECTION_ALL_TERMS
}

/// Whether the view ray reaches the face from its up side. Reads the
/// interpolated outward normal, never the viewer-flipped shading normal.
pub fn reflection_seen_from_above(outward: Vec3, view: Vec3) -> bool {
    outward.dot(view) > 0.0
}

/// Whether a fragment draws the probe term at all; otherwise it renders
/// exactly as without reflection.
pub fn reflection_gate(
    probe: u32,
    outward: Vec3,
    view: Vec3,
    mask: u32,
    reflectivity: f32,
) -> bool {
    probe != FLUID_REFLECTION_NO_PROBE
        && reflectivity > 0.0
        && reflection_seen_from_above(outward, view)
        && reflection_mask_on(mask)
}

/// The view ray mirrored about the flat outward normal, for a unit `view`
/// (fragment to eye) and unit `outward`.
pub fn reflection_mirror(view: Vec3, outward: Vec3) -> Vec3 {
    outward * (2.0 * view.dot(outward)) - view
}

/// Where the reflected ray from `origin` leaves the projection box: the
/// runtime slab test, nearest exit over the three slabs. A component near
/// zero is nudged off it rather than divided by. A fragment outside the box
/// clamps to itself.
pub fn reflection_box_hit(origin: Vec3, ray: Vec3, box_min: Vec3, box_max: Vec3) -> Vec3 {
    let nudged = Vec3::select(
        ray.cmplt(Vec3::ZERO),
        Vec3::splat(-FLUID_REFLECTION_DIRECTION_EPSILON),
        Vec3::splat(FLUID_REFLECTION_DIRECTION_EPSILON),
    );
    let safe = Vec3::select(
        ray.abs()
            .cmplt(Vec3::splat(FLUID_REFLECTION_DIRECTION_EPSILON)),
        nudged,
        ray,
    );
    let t0 = (box_min - origin) / safe;
    let t1 = (box_max - origin) / safe;
    let far = t0.max(t1);
    let t = far.x.min(far.y.min(far.z)).max(0.0);
    origin + ray * t
}

/// The tile lookup direction: from the capture point to the reflected
/// point (the distance search's hit, or the box hit), clamped to the horizon
/// when it lies below the capture point. A point on the capture point itself
/// has no direction and takes the reflected ray's.
pub fn reflection_lookup(hit: Vec3, capture_point: Vec3, ray: Vec3) -> Vec3 {
    let toward = hit - capture_point;
    let level = Vec3::new(toward.x, toward.y.max(0.0), toward.z);
    if level.dot(level) < FLUID_REFLECTION_DIRECTION_EPSILON {
        Vec3::new(ray.x, ray.y.max(0.0), ray.z)
    } else {
        level
    }
}

/// Normalized tile UV for `direction`: the octahedral texel coordinate
/// (`octahedral::irradiance_tile_texel_coord`) over the tile dimension.
pub fn reflection_tile_uv(direction: Vec3, tile_dimension: u32, tile_border: u32) -> Vec2 {
    let dimension = tile_dimension as f32;
    let border = tile_border as f32;
    let interior = dimension - 2.0 * border;
    let oct = Vec2::from(encode_unquantized_uv(direction.x, direction.y, direction.z));
    (Vec2::splat(border) + oct * interior) / dimension
}

/// Schlick Fresnel with base reflectance 0.02 × `reflectivity`, topping at
/// 1. `cos_theta` is between the view-to-eye vector and the outward normal.
pub fn reflection_fresnel(cos_theta: f32, reflectivity: f32) -> f32 {
    let f0 = (FLUID_REFLECTION_BASE_F0 * reflectivity).min(1.0);
    let m = 1.0 - cos_theta.clamp(0.0, 1.0);
    let m2 = m * m;
    f0 + (1.0 - f0) * (m2 * m2 * m)
}

/// The reflection over the straight-alpha `surface` (surface over fog, or the
/// surface alone) as one straight-alpha output: coverage `F + (1 − F)·a` and
/// colour `(F·R + (1 − F)·a·c) / coverage`. Blended over a scene it equals
/// `F·R + (1 − F)·(surface over scene)`, so opacity never dims the reflection.
pub fn reflection_composite(surface: Vec4, reflection: Vec3, fresnel: f32) -> Vec4 {
    let coverage = fresnel + (1.0 - fresnel) * surface.w;
    let rgb = (reflection * fresnel + surface.truncate() * ((1.0 - fresnel) * surface.w))
        / coverage.max(FLUID_REFLECTION_COVERAGE_EPSILON);
    rgb.extend(coverage)
}

/// The animated lights' compose descriptors as the fluid pass binds them
/// (group 3 binding 11, `anim_descriptors`), roster-indexed, and the frame's
/// animation clock (`uniforms.time`).
#[derive(Debug, Clone, Copy)]
pub struct FluidReflectionLights<'a> {
    pub descriptors: &'a [AnimatedLightScaleDescriptor<'a>],
    pub time: f32,
}

impl FluidReflectionLights<'_> {
    /// No animated lights: every share entry indexes nothing.
    pub const NONE: FluidReflectionLights<'static> = FluidReflectionLights {
        descriptors: &[],
        time: 0.0,
    };
}

/// Whether a share entry's roster index names a descriptor: the reserved
/// value, and any index past the bound descriptors, indexes nothing, so the
/// entry adds no term (P28).
pub fn reflection_light_present(lights: &FluidReflectionLights<'_>, roster_index: u32) -> bool {
    roster_index != REFLECTION_PROBE_NO_LIGHT && (roster_index as usize) < lights.descriptors.len()
}

/// A present light's live RGB value, as SH compose evaluates it:
/// `sh_compose.wgsl`'s `animated_light_scale` read straight from descriptor
/// `roster_index`, the roster→descriptor table being identity whenever
/// section 54 loads (P10). Clamped colour × clamped brightness, 0 while
/// inactive; no promotion weight, lighting mask or debug override.
pub fn reflection_light_value(lights: &FluidReflectionLights<'_>, roster_index: u32) -> Vec3 {
    Vec3::from(animated_light_scale(
        Some(lights.descriptors[roster_index as usize]),
        lights.time,
    ))
}

/// One lobe word's RGB share: three 10-bit unorm fields, red lowest.
pub fn reflection_unpack_unorm10x3(word: u32) -> Vec3 {
    let field = FLUID_REFLECTION_UNORM10_MAX;
    Vec3::new(
        (word & field) as f32,
        ((word >> 10) & field) as f32,
        ((word >> 20) & field) as f32,
    ) / field as f32
}

/// A stored light's RGB share toward a lookup direction: its lobe shares
/// blended by that direction's lobe weights (`reflection_probe_lobe_weights`,
/// the function the compiler split the capture's light by).
pub fn reflection_light_share(
    shares: &[u32; FLUID_PROBE_SHARE_WORDS],
    lobes: &[f32; REFLECTION_PROBE_LOBE_COUNT],
) -> Vec3 {
    let mut share = Vec3::ZERO;
    for (word, weight) in shares.iter().zip(lobes) {
        share += reflection_unpack_unorm10x3(*word) * *weight;
    }
    share
}

/// A light's live change against the reference its share entry stores, per
/// channel: `current / reference − 1`. A channel the reference holds no
/// light in adds no term, so a pure-red light's green never divides by zero.
pub fn reflection_light_change(current: Vec3, reference: Vec3) -> Vec3 {
    Vec3::select(
        reference.cmpgt(Vec3::ZERO),
        current / reference.max(Vec3::splat(FLUID_REFLECTION_REFERENCE_FLOOR)) - Vec3::ONE,
        Vec3::ZERO,
    )
}

/// The capture's live-lighting scale toward `direction`, the tile lookup
/// direction, per RGB channel: `1 + Σ share(direction) × change`, clamped at
/// 0, over the probe's stored lights. A light past the stored K has no entry,
/// so it holds its reference. Reads no SH and no promotion weight: a function
/// of the probe, the direction and the live descriptors alone.
pub fn reflection_ratio(
    probe_lights: &[FluidProbeLightGpu; REFLECTION_PROBE_LIGHTS_PER_PROBE],
    direction: Vec3,
    lights: &FluidReflectionLights<'_>,
) -> Vec3 {
    let lobes = reflection_probe_lobe_weights(direction.to_array());
    let mut ratio = Vec3::ONE;
    for light in probe_lights {
        if !reflection_light_present(lights, light.roster_index) {
            continue;
        }
        let current = reflection_light_value(lights, light.roster_index);
        ratio += reflection_light_share(&light.shares, &lobes)
            * reflection_light_change(current, Vec3::from(light.reference));
    }
    ratio.max(Vec3::ZERO)
}

/// Everything one fluid fragment's probe term reads besides its surface.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FluidReflectionFragment {
    /// The face's flat per-vertex probe index.
    pub probe: u32,
    pub camera: Vec3,
    pub world_position: Vec3,
    /// The interpolated outward face normal, any length; normalized as the
    /// shader does.
    pub outward: Vec3,
    pub light_term_mask: u32,
    /// The row's packed reflectivity (`surface.z`).
    pub reflectivity: f32,
    /// The scene's reflection style (`FluidReflectionStyle::uniform_value`).
    pub style: u32,
    /// The unwarped texel UV: the normalized UV times the face texture's
    /// dimensions, before drift, wave or cell offsets.
    pub texel_uv: Vec2,
    /// The face's texel axes (`reflection_texel_axes` of the fragment's
    /// derivatives).
    pub axes: FluidTexelAxes,
    /// The animation clock and the fade footprint the warp reads, so the
    /// cell's offset is the one its albedo displays.
    pub time: f32,
    pub texels_per_pixel: f32,
}

/// The fragment's output as `fs_main`'s tail computes it from `surface`
/// (the fog reference's output): unchanged when the gate is off, else the
/// probe's capture, sampled toward the distance search's lookup and scaled
/// by the live-lighting ratio toward that same direction, composited over it
/// at the Fresnel weight times the search's (0 deep in the capture shadow).
/// The reflection starts from the style's point (the fragment, or its texel
/// cell's centre about the cell's tilted normal) and sees the eye from
/// there, so every fragment of a pixel-style cell reads one search, one
/// sample and one weight. `sample(layer, uv)` reads the colour tile and
/// `distance(layer, uv)` the distance tile, both at level 0. The layer comes
/// from the face's probe index alone.
pub fn reflection_fragment(
    surface: Vec4,
    fragment: &FluidReflectionFragment,
    probes: &[ReflectionProbeRecord],
    tile: [u32; 2],
    lights: &FluidReflectionLights<'_>,
    sample: impl Fn(u32, Vec2) -> Vec3,
    distance: impl Fn(u32, Vec2) -> f32,
) -> Vec4 {
    let view = (fragment.camera - fragment.world_position).normalize_or_zero();
    let outward = fragment.outward.normalize();
    if !reflection_gate(
        fragment.probe,
        outward,
        view,
        fragment.light_term_mask,
        fragment.reflectivity,
    ) {
        return surface;
    }
    let point = reflection_point(
        fragment.style,
        fragment.texel_uv,
        fragment.world_position,
        outward,
        fragment.axes,
        fragment.time,
        fragment.texels_per_pixel,
    );
    let view = (fragment.camera - point.position).normalize_or_zero();
    let probe = &probes[fragment.probe as usize];
    let ray = reflection_mirror(view, point.normal);
    let search = reflection_search(
        point.position,
        ray,
        Vec3::from(probe.box_min),
        Vec3::from(probe.box_max),
        Vec3::from(probe.capture_point),
        tile,
        |uv| distance(probe.layer, uv),
    );
    let capture = sample(
        probe.layer,
        reflection_tile_uv(search.lookup, tile[0], tile[1]),
    ) * reflection_ratio(&FluidProbeGpu::pack(probe).lights, search.lookup, lights);
    let fresnel = reflection_fresnel(view.dot(outward), fragment.reflectivity) * search.weight;
    reflection_composite(surface, capture, fresnel)
}

#[cfg(test)]
#[path = "reflection_tests.rs"]
mod tests;
