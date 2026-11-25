// Reflection-probe capture visibility: warn when a capture point sees less than half of its probe's bound reflector.
// See: context/lib/resource_management.md §5.1 · rendering_pipeline.md §7.3a

use glam::{DVec2, Vec3};
use postretro_level_format::environment_volumes::EnvironmentVolumesSection;
use postretro_level_format::octahedral::irradiance_tile_texel_coord;
use postretro_level_format::reflection_probes::ReflectionProbesSection;

use super::facing::Facing;
use super::placement::{PlacedProbe, ReflectorFace, ReflectorSource, up_facing_faces};
use crate::bc6h::f16_bits_to_f32;

/// A probe whose capture point sees less than this fraction of its bound
/// water area warns: the runtime draws no probe term on the rest.
const MIN_SEEN_FRACTION: f64 = 0.5;
/// κ: the fluid shader's capture-shadow depth margin, in
/// `h = |v|₁ · w(v) − 1`. Water with h < κ draws a probe term (faded within
/// the margin and beside the shadow's edge); water at or past it draws none.
/// A parity test pins it to render-cpu's `FLUID_REFLECTION_SHADOW_MARGIN`.
pub(crate) const CAPTURE_SHADOW_MARGIN: f32 = 0.05;
/// δ: how far behind the capture point's plane, along the probe's facing
/// normal, a glossy fragment still tests its capture shadow at that plane
/// alone (m). Deeper, on a farther bound plane, the overlay also tests it
/// from its true position. A parity test pins it to render-cpu's
/// `FLUID_REFLECTION_SHADOW_PLANE_DEPTH`.
pub(crate) const CAPTURE_SHADOW_PLANE_DEPTH: f32 = 0.004;
/// Each fan triangle of a bound face splits into `SUBDIVISIONS²` equal
/// sub-triangles, one sample at each centroid: 128 samples on a quad.
const SUBDIVISIONS: u32 = 8;
/// An offset this short (L1, m) from the capture point is the point itself.
const AT_CAPTURE_POINT_M: f32 = 1.0e-6;

/// Warn, naming the probe and its source entity, for every probe whose
/// capture point sees less than [`MIN_SEEN_FRACTION`] of the reflector it
/// binds: its water, or its glossy world faces among `world_faces` (the
/// placed world faces, in id-17 order).
///
/// "Sees" is the runtime's capture-shadow cutoff read from the stored
/// distance tile: [`DistanceTile::sees`] for water, as the fluid shader
/// tests it, and [`DistanceTile::sees_glossy`] for glossy faces, as the
/// overlay tests them. So the seen surface matches the surface that draws a
/// probe term (faded within κ of the cutoff), and the warning predicts the
/// dark surface. It traces no rays (a direct C-to-P
/// visibility test would cost a BVH query per sample and could disagree
/// with what the shader reads), and it runs from the section alone, so a
/// warm build warns as a cold one does. Samples sit at fixed points of each
/// face and sum in fixed order, so the result is deterministic.
///
/// The remedy it names follows the probe's source: an authored box can move
/// its point with `_capture_offset` or be split; a derived probe's entity is
/// its fluid, which has no such key, so the hidden water needs its own
/// `reflection_volume`. A glossy face's probe names none.
pub(crate) fn warn_hidden_reflectors(
    section: &ReflectionProbesSection,
    probes: &[PlacedProbe],
    volumes: &EnvironmentVolumesSection,
    world_faces: &[ReflectorFace],
) {
    let mut faces = up_facing_faces(volumes);
    faces.extend(world_faces.iter().cloned());
    for (index, (probe, record)) in probes.iter().zip(&section.probes).enumerate() {
        let tile = DistanceTile::decode(section, record.layer as usize);
        let seen = seen_fraction(&tile, probe, &faces);
        if seen >= MIN_SEEN_FRACTION {
            continue;
        }
        if !probe.world_faces.is_empty() {
            let what = bound_reflector(probe);
            log::warn!(
                "[ReflectionProbes] probe {index} of entity {}: its capture point {:?} sees \
                 {:.0}% of the {what} it binds, and what it cannot see draws no reflection",
                probe.source_entity,
                probe.capture_point,
                seen * 100.0,
            );
            continue;
        }
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

/// What a probe binding world faces names in its warning: every reflector
/// [`seen_fraction`] measures, so the name and the percentage agree. Only an
/// up-facing probe binds water beside world faces.
fn bound_reflector(probe: &PlacedProbe) -> &'static str {
    match (probe.facing.is_up(), probe.fluid_faces.is_empty()) {
        (true, false) => "water and glossy floor",
        (true, true) => "glossy floor",
        (false, _) => "glossy faces",
    }
}

/// The fraction of `probe`'s bound reflector area its capture point sees, by
/// area-weighted samples; `faces` ascends by source. A probe binding no area
/// sees all of it.
fn seen_fraction(tile: &DistanceTile, probe: &PlacedProbe, faces: &[ReflectorFace]) -> f64 {
    let capture = Vec3::from(probe.capture_point);
    let (mut seen, mut total) = (0.0f64, 0.0f64);
    let sources = probe
        .fluid_faces
        .iter()
        .map(|&(volume, face)| ReflectorSource::Fluid { volume, face })
        .chain(
            probe
                .world_faces
                .iter()
                .map(|&face| ReflectorSource::World { face }),
        );
    for source in sources {
        let Ok(at) = faces.binary_search_by_key(&source, |face| face.source) else {
            continue;
        };
        let face = &faces[at];
        let height = face.plane_height();
        let glossy = matches!(source, ReflectorSource::World { .. });
        face_samples(&face.outline, |point, area| {
            total += area;
            let surface = face.facing.lift(point, height).as_vec3();
            let sees = if glossy {
                tile.sees_glossy(capture, surface, &probe.facing)
            } else {
                tile.sees(capture, surface, &probe.facing)
            };
            if sees {
                seen += area;
            }
        });
    }
    if total > 0.0 { seen / total } else { 1.0 }
}

/// Every sample of a convex `outline` (in-plane coordinates), with the area
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
    /// a probe term there: with `v` the offset clamped to `facing`'s
    /// hemisphere (the horizon, for water) as the shader clamps every tap,
    /// `h = |v|₁ · w(v) − 1 < κ` ([`CAPTURE_SHADOW_MARGIN`]). The shader
    /// fades that term within κ and beside the shadow's edge; past κ it draws
    /// none. A miss stores 0, so open sky is always seen.
    pub(crate) fn sees(&self, capture: Vec3, point: Vec3, facing: &Facing) -> bool {
        let offset = point - capture;
        let level = match facing {
            Facing::Up => Vec3::new(offset.x, offset.y.max(0.0), offset.z),
            Facing::Other(_) => {
                let normal = facing.normal().as_vec3();
                offset - normal * offset.dot(normal).min(0.0)
            }
        };
        let l1 = level.abs().element_sum();
        l1 < AT_CAPTURE_POINT_M || l1 * self.sample(level) < 1.0 + CAPTURE_SHADOW_MARGIN
    }

    /// Whether the glossy overlay draws a probe term at `point`, a glossy
    /// world face's fragment. Within [`CAPTURE_SHADOW_PLANE_DEPTH`] of the
    /// capture point's plane it tests as [`Self::sees`] does. Deeper, on a
    /// farther bound plane (a lower floor sharing the probe), it reads `h`
    /// twice: from its true position lifted by δ, which the full-sphere tile
    /// holds, and over the capture point's plane; the larger must stay under
    /// κ. So an object the capture point saw between it and the fragment
    /// withholds the term there, as the overlay's search does.
    pub(crate) fn sees_glossy(&self, capture: Vec3, point: Vec3, facing: &Facing) -> bool {
        let axis = facing.normal().as_vec3();
        let offset = point - capture;
        let along = offset.dot(axis);
        let h = |level: Vec3| {
            let l1 = level.abs().element_sum();
            if l1 < AT_CAPTURE_POINT_M {
                -1.0
            } else {
                l1 * self.sample(level) - 1.0
            }
        };
        let h_shadow = h(offset - axis * along.clamp(-CAPTURE_SHADOW_PLANE_DEPTH, 0.0));
        let h_depth = if along < -CAPTURE_SHADOW_PLANE_DEPTH {
            h_shadow.max(h(offset - axis * along.min(0.0)))
        } else {
            h_shadow
        };
        h_depth < CAPTURE_SHADOW_MARGIN
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(facing: Facing, fluid_faces: Vec<(u32, u32)>) -> PlacedProbe {
        PlacedProbe {
            source_entity: 2,
            authored: true,
            box_min: [-1.0; 3],
            box_max: [1.0; 3],
            capture_point: [0.0; 3],
            facing,
            bound_area: 1.0,
            fluid_faces,
            world_faces: vec![0],
        }
    }

    // The warning's percentage covers water and glossy floor together, so a
    // probe binding both names both.
    #[test]
    fn the_hidden_reflector_warning_names_every_reflector_it_measures() {
        assert_eq!(
            bound_reflector(&probe(Facing::Up, vec![(0, 0)])),
            "water and glossy floor"
        );
        assert_eq!(
            bound_reflector(&probe(Facing::Up, Vec::new())),
            "glossy floor"
        );
    }
}
