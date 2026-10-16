// Reflection-probe capture visibility: how much of its water a capture point sees, and the warning when splitting left a probe seeing under half.
// See: context/lib/resource_management.md §5.1 · rendering_pipeline.md §7.3a

use std::collections::BTreeMap;

use glam::{DVec2, DVec3, Vec3};
use postretro_level_format::environment_volumes::EnvironmentVolumesSection;
use postretro_level_format::octahedral::irradiance_tile_texel_coord;
use postretro_level_format::reflection_probes::ReflectionProbesSection;

use super::placement::{UpFacingFace, up_facing_faces};
use crate::bc6h::f16_bits_to_f32;

/// A bound face whose capture point sees less than this fraction of it
/// splits off (the sight trigger), and a probe still seeing less than this
/// fraction of its bound water after splitting warns: the runtime draws no
/// probe term on water its point cannot see.
pub(crate) const MIN_SEEN_FRACTION: f64 = 0.5;
/// Each fan triangle of a bound face splits into `SUBDIVISIONS²` equal
/// sub-triangles, one sample at each centroid: 128 samples on a quad.
pub(crate) const SUBDIVISIONS: u32 = 8;
/// An offset this short (L1, m) from the capture point is the point itself.
pub(crate) const AT_CAPTURE_POINT_M: f32 = 1.0e-6;

/// Warn, naming the probe and its source entity, for every probe of the
/// finished section whose capture point still sees less than
/// [`MIN_SEEN_FRACTION`] of the water it binds: splitting stopped short of
/// it, at the round limit, at a single face, or at the cap.
///
/// "Sees" is the runtime's own capture-shadow test read from the stored
/// distance tile: a water point P is seen when `|v|₁ · w(v) < 1`, with `v`
/// the offset from the capture point clamped to the horizon and `w` the
/// tile's bilinear inverse L1 distance. That is exactly the water the fluid
/// shader draws a probe term on, so the warning predicts the dark water and
/// nothing else. It runs from the section and id 52 alone, so a warm build
/// warns as a cold one does. Samples sit at fixed points of each face and
/// sum in fixed order, so the result is deterministic.
pub(crate) fn warn_hidden_water(
    section: &ReflectionProbesSection,
    volumes: &EnvironmentVolumesSection,
) {
    let faces = up_facing_faces(volumes);
    let mut bound: BTreeMap<u32, Vec<(u32, u32)>> = BTreeMap::new();
    for binding in &section.bindings {
        bound
            .entry(binding.probe)
            .or_default()
            .push((binding.volume, binding.face));
    }
    for (index, record) in section.probes.iter().enumerate() {
        let keys = bound.get(&(index as u32)).map_or(&[][..], Vec::as_slice);
        let tile = DistanceTile::decode(section, record.layer as usize);
        let seen = seen_fraction(&tile, Vec3::from(record.capture_point), keys, &faces);
        if seen < MIN_SEEN_FRACTION {
            log::warn!(
                "[ReflectionProbes] probe {index} of entity {}: its capture point {:?} sees \
                 {:.0}% of the water it binds after splitting, and water it cannot see draws no \
                 reflection; move the point with `_capture_offset`, or bound the hidden water \
                 with its own reflection_volume",
                record.source_entity,
                record.capture_point,
                seen * 100.0,
            );
        }
    }
}

/// The fraction of the water `keys` (bound faces, `(volume, face)`) a
/// capture point at `capture` sees, by area-weighted samples. No bound area
/// is all seen.
pub(crate) fn seen_fraction(
    tile: &DistanceTile,
    capture: Vec3,
    keys: &[(u32, u32)],
    faces: &[UpFacingFace],
) -> f64 {
    let (mut seen, mut total) = (0.0f64, 0.0f64);
    for key in keys {
        let Some(face) = face_by_key(faces, *key) else {
            continue;
        };
        face_samples(&face.outline, |point, area| {
            total += area;
            if tile.sees(capture, water_point(face, point)) {
                seen += area;
            }
        });
    }
    if total > 0.0 { seen / total } else { 1.0 }
}

/// The up-facing face `key` names in `faces` (id-52 order).
pub(crate) fn face_by_key(faces: &[UpFacingFace], key: (u32, u32)) -> Option<&UpFacingFace> {
    faces
        .binary_search_by_key(&key, |face| (face.volume, face.face))
        .ok()
        .map(|at| &faces[at])
}

/// The water point under horizontal sample `point` of `face`.
pub(crate) fn water_point(face: &UpFacingFace, point: DVec2) -> Vec3 {
    DVec3::new(point.x, face.geometry.plane_height(), point.y).as_vec3()
}

/// Every sample of a convex `outline` (horizontal `(x, z)`), with the area
/// it stands for: a fan from the first vertex, each triangle split into
/// equal sub-triangles sampled at their centroids.
pub(crate) fn face_samples(outline: &[DVec2], mut sample: impl FnMut(DVec2, f64)) {
    let Some(&first) = outline.first() else {
        return;
    };
    let n = f64::from(SUBDIVISIONS);
    for pair in outline[1..].windows(2) {
        let (ab, ac) = (pair[0] - first, pair[1] - first);
        let area = (ab.perp_dot(ac) * 0.5).abs() / (n * n);
        if area <= 0.0 {
            continue;
        }
        let at = |u: f64, v: f64| first + ab * (u / n) + ac * (v / n);
        for i in 0..SUBDIVISIONS {
            for j in 0..SUBDIVISIONS - i {
                let (u, v) = (f64::from(i), f64::from(j));
                sample(at(u + 1.0 / 3.0, v + 1.0 / 3.0), area);
                if i + j + 1 < SUBDIVISIONS {
                    sample(at(u + 2.0 / 3.0, v + 2.0 / 3.0), area);
                }
            }
        }
    }
}

/// One probe's distance tile decoded to f32, sampled as the fluid pass
/// samples it: bilinear in texel space, clamped at the tile edge.
pub(crate) struct DistanceTile {
    dimension: u32,
    border: u32,
    texels: Vec<f32>,
}

impl DistanceTile {
    pub(crate) fn decode(section: &ReflectionProbesSection, layer: usize) -> Self {
        Self::from_bytes(
            section.tile_dimension,
            section.tile_border,
            section.distance_tile(layer),
        )
    }

    /// A tile from its little-endian f16 payload, as the section stores it.
    pub(crate) fn from_bytes(dimension: u32, border: u32, bytes: &[u8]) -> Self {
        let (texels, _) = bytes.as_chunks::<2>();
        Self {
            dimension,
            border,
            texels: texels
                .iter()
                .map(|bits| f16_bits_to_f32(u16::from_le_bytes(*bits)))
                .collect(),
        }
    }

    /// Angular width of one interior texel, radians: a half-turn of horizon
    /// crosses the interior, as the fluid shader's capture-shadow fade reads.
    pub(crate) fn texel_angle(&self) -> f32 {
        std::f32::consts::PI / (self.dimension - 2 * self.border) as f32
    }

    /// The stored inverse L1 distance toward non-zero `direction`.
    pub(crate) fn sample(&self, direction: Vec3) -> f32 {
        let [x, y] = irradiance_tile_texel_coord(direction.to_array(), self.dimension, self.border);
        let last = self.dimension as i64 - 1;
        let (fx, fy) = (x - 0.5, y - 0.5);
        let (x0, y0) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - x0, fy - y0);
        let at = |x: f32, y: f32| {
            let x = (x as i64).clamp(0, last) as usize;
            let y = (y as i64).clamp(0, last) as usize;
            self.texels[y * self.dimension as usize + x]
        };
        let top = at(x0, y0) * (1.0 - tx) + at(x0 + 1.0, y0) * tx;
        let bottom = at(x0, y0 + 1.0) * (1.0 - tx) + at(x0 + 1.0, y0 + 1.0) * tx;
        top * (1.0 - ty) + bottom * ty
    }

    /// Whether the capture point sees `point`: the offset, clamped to the
    /// horizon as the shader clamps every tap, lies in front of the surface
    /// the capture stored in its direction. A miss stores 0, so open sky is
    /// always seen.
    pub(crate) fn sees(&self, capture: Vec3, point: Vec3) -> bool {
        let offset = point - capture;
        let level = Vec3::new(offset.x, offset.y.max(0.0), offset.z);
        let l1 = level.abs().element_sum();
        l1 < AT_CAPTURE_POINT_M || l1 * self.sample(level) < 1.0
    }
}
