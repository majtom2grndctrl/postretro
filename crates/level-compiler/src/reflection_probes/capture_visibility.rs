// Reflection-probe capture visibility: warn when a capture point sees less than half of its probe's bound water.
// See: context/lib/resource_management.md §5.1 · rendering_pipeline.md §7.3a

use glam::{DVec2, DVec3, Vec3};
use postretro_level_format::environment_volumes::EnvironmentVolumesSection;
use postretro_level_format::octahedral::irradiance_tile_texel_coord;
use postretro_level_format::reflection_probes::ReflectionProbesSection;

use super::placement::{PlacedProbe, UpFacingFace, up_facing_faces};
use crate::bc6h::f16_bits_to_f32;

/// A probe whose capture point sees less than this fraction of its bound
/// water area warns: the runtime draws no probe term on the rest.
const MIN_SEEN_FRACTION: f64 = 0.5;
/// κ: the fluid shader's capture-shadow depth margin, in
/// `h = |v|₁ · w(v) − 1`. Water with h < κ draws a probe term (faded within
/// the margin and beside the shadow's edge); water at or past it draws none.
/// A parity test pins it to render-cpu's `FLUID_REFLECTION_SHADOW_MARGIN`.
pub(crate) const CAPTURE_SHADOW_MARGIN: f32 = 0.05;
/// Each fan triangle of a bound face splits into `SUBDIVISIONS²` equal
/// sub-triangles, one sample at each centroid: 128 samples on a quad.
const SUBDIVISIONS: u32 = 8;
/// An offset this short (L1, m) from the capture point is the point itself.
const AT_CAPTURE_POINT_M: f32 = 1.0e-6;

/// Warn, naming the probe and its source entity, for every probe whose
/// capture point sees less than [`MIN_SEEN_FRACTION`] of the water it binds.
///
/// "Sees" is [`DistanceTile::sees`], the runtime's capture-shadow cutoff read
/// from the stored distance tile, so the seen water matches the water the
/// fluid shader draws a probe term on (faded within κ of the cutoff), and
/// the warning predicts the dark water. It traces no rays (a direct C-to-P
/// visibility test would cost a BVH query per sample and could disagree
/// with what the shader reads), and it runs from the section alone, so a
/// warm build warns as a cold one does. Samples sit at fixed points of each
/// face and sum in fixed order, so the result is deterministic.
///
/// The remedy it names follows the probe's source: an authored box can move
/// its point with `_capture_offset` or be split; a derived probe's entity is
/// its fluid, which has no such key, so the hidden water needs its own
/// `reflection_volume`.
pub(crate) fn warn_hidden_water(
    section: &ReflectionProbesSection,
    probes: &[PlacedProbe],
    volumes: &EnvironmentVolumesSection,
) {
    let faces = up_facing_faces(volumes);
    for (index, (probe, record)) in probes.iter().zip(&section.probes).enumerate() {
        let tile = DistanceTile::decode(section, record.layer as usize);
        let seen = seen_fraction(&tile, probe, &faces);
        if seen < MIN_SEEN_FRACTION {
            let remedy = if probe.authored {
                "move the point with `_capture_offset`, or split the box"
            } else {
                "bound the hidden water with its own reflection_volume"
            };
            log::warn!(
                "[ReflectionProbes] probe {index} of entity {}: its capture point {:?} sees \
                 {:.0}% of the water it binds, and water it cannot see draws no reflection; \
                 {remedy}",
                probe.source_entity,
                probe.capture_point,
                seen * 100.0,
            );
        }
    }
}

/// The fraction of `probe`'s bound water area its capture point sees, by
/// area-weighted samples. A probe binding no area sees all of it.
fn seen_fraction(tile: &DistanceTile, probe: &PlacedProbe, faces: &[UpFacingFace]) -> f64 {
    let capture = Vec3::from(probe.capture_point);
    let (mut seen, mut total) = (0.0f64, 0.0f64);
    for key in &probe.faces {
        let Ok(at) = faces.binary_search_by_key(key, |face| (face.volume, face.face)) else {
            continue;
        };
        let face = &faces[at];
        let height = face.geometry.plane_height();
        face_samples(&face.outline, |point, area| {
            total += area;
            let water = DVec3::new(point.x, height, point.y).as_vec3();
            if tile.sees(capture, water) {
                seen += area;
            }
        });
    }
    if total > 0.0 { seen / total } else { 1.0 }
}

/// Every sample of a convex `outline` (horizontal `(x, z)`), with the area
/// it stands for: a fan from the first vertex, each triangle split into
/// equal sub-triangles sampled at their centroids.
fn face_samples(outline: &[DVec2], mut sample: impl FnMut(DVec2, f64)) {
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

    /// The stored inverse L1 distance toward non-zero `direction`.
    fn sample(&self, direction: Vec3) -> f32 {
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

    /// Whether the capture point sees `point`, meaning the fluid shader draws
    /// a probe term there: with `v` the offset clamped to the horizon as the
    /// shader clamps every tap, `h = |v|₁ · w(v) − 1 < κ`
    /// ([`CAPTURE_SHADOW_MARGIN`]). The shader fades that term within κ and
    /// beside the shadow's edge; past κ it draws none. A miss stores 0, so
    /// open sky is always seen.
    pub(crate) fn sees(&self, capture: Vec3, point: Vec3) -> bool {
        let offset = point - capture;
        let level = Vec3::new(offset.x, offset.y.max(0.0), offset.z);
        let l1 = level.abs().element_sum();
        l1 < AT_CAPTURE_POINT_M || l1 * self.sample(level) < 1.0 + CAPTURE_SHADOW_MARGIN
    }
}
