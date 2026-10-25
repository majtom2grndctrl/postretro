// EnvironmentVolumes PRL section (ID 52): the level default gravity plus every
// static regional volume (gravity, push, fluid) as convex brush leaves.
// See: context/lib/build_pipeline.md §PRL section IDs

use crate::convex_leaf_codec::{
    SectionReader, brushes_byte_len, write_brushes, write_count, write_f32, write_string,
    write_vec3,
};

/// Every decode error names this section.
const SECTION: &str = "environment volumes";

pub const ENVIRONMENT_VOLUMES_VERSION: u16 = 1;

/// Level default gravity when worldspawn `initialGravity` is absent, m/s².
/// Negative values accelerate downward in engine space. Quake's 800 u/s² at
/// the map scale of 0.0254 m per unit — about twice Earth's, so jumps land in
/// the genre's ~0.7 s instead of hanging.
pub const DEFAULT_WORLD_GRAVITY_MPS2: f32 = -20.32_f32;

/// Fixed bytes of one volume record with zero brushes, zero faces and an empty
/// fluid name: entity, priority, gravity flag + value, push flag + vector,
/// fluid length, brush count, face count.
const MIN_VOLUME_SIZE: usize = 4 + 4 + 1 + 4 + 1 + 12 + 4 + 4 + 4;
/// One surface face with zero vertices: texture index, vertex count.
const MIN_FACE_SIZE: usize = 4 + 4;
/// One surface vertex: position and UV.
const VERTEX_SIZE: usize = 12 + 8;

/// Fewest planes that bound a closed convex leaf (a tetrahedron).
pub const MIN_PLANES_PER_BRUSH: usize = 4;
/// Fewest vertices of a surface polygon.
pub const MIN_VERTICES_PER_FACE: usize = 3;

/// The always-present environment section. A level with no volumes still
/// carries its default gravity here.
#[derive(Debug, Clone, PartialEq)]
pub struct EnvironmentVolumesSection {
    /// Level default gravity, m/s², signed vertical (negative = downward).
    pub level_gravity: f32,
    /// Volumes in source map entity order; that order is the overlap rule's
    /// final tiebreak, so it is persistent.
    pub volumes: Vec<EnvironmentVolumeRecord>,
}

/// One environment volume. Each field a volume does not set is absent, so
/// per-field resolution falls through to other volumes or the level default.
/// Field order is persistent wire layout. Do not reorder.
#[derive(Debug, Clone, PartialEq)]
pub struct EnvironmentVolumeRecord {
    /// Source map entity index, for diagnostics.
    pub source_entity: u32,
    pub priority: i32,
    /// Gravity override, m/s², signed vertical.
    pub gravity: Option<f32>,
    /// Push acceleration, m/s², engine axes (Y-up).
    pub push: Option<[f32; 3]>,
    /// Fluid name the mod manifest resolves; empty = dry.
    pub fluid: String,
    /// One convex leaf per authored brush.
    pub brushes: Vec<EnvironmentBrush>,
    /// Visible fluid surface polygons. A dry volume has none; decoding rejects
    /// one that does, and a volume that sets no field at all.
    pub surface_faces: Vec<EnvironmentSurfaceFace>,
}

/// One convex leaf in engine space (Y-up, meters).
#[derive(Debug, Clone, PartialEq)]
pub struct EnvironmentBrush {
    pub aabb_min: [f32; 3],
    pub aabb_max: [f32; 3],
    /// A point `p` is inside when `dot(p, normal) - distance <= 0` for every plane.
    pub planes: Vec<EnvironmentPlane>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnvironmentPlane {
    pub normal: [f32; 3],
    pub distance: f32,
}

/// One surface polygon, textured from the brush face it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct EnvironmentSurfaceFace {
    /// Index into the level's TextureNames table, as KinematicGeometry face meta.
    pub texture_index: u32,
    pub vertices: Vec<EnvironmentSurfaceVertex>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnvironmentSurfaceVertex {
    pub position: [f32; 3],
    /// Texture coordinates in texel space, from the brush face's projection.
    /// The renderer normalises them by the texture's size at load.
    pub uv: [f32; 2],
}

impl Default for EnvironmentVolumesSection {
    /// The section a map with no volumes and no `initialGravity` compiles to.
    fn default() -> Self {
        Self {
            level_gravity: DEFAULT_WORLD_GRAVITY_MPS2,
            volumes: Vec::new(),
        }
    }
}

impl EnvironmentVolumesSection {
    pub fn byte_len(&self) -> usize {
        2 + 4
            + 4
            + self
                .volumes
                .iter()
                .map(|volume| {
                    // The brush count is inside MIN_VOLUME_SIZE.
                    MIN_VOLUME_SIZE - 4
                        + volume.fluid.len()
                        + brushes_byte_len(&volume.brushes)
                        + volume
                            .surface_faces
                            .iter()
                            .map(|face| MIN_FACE_SIZE + face.vertices.len() * VERTEX_SIZE)
                            .sum::<usize>()
                })
                .sum::<usize>()
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.byte_len());
        out.extend_from_slice(&ENVIRONMENT_VOLUMES_VERSION.to_le_bytes());
        write_f32(&mut out, self.level_gravity);
        write_count(&mut out, self.volumes.len());
        for volume in &self.volumes {
            out.extend_from_slice(&volume.source_entity.to_le_bytes());
            out.extend_from_slice(&volume.priority.to_le_bytes());
            out.push(u8::from(volume.gravity.is_some()));
            write_f32(&mut out, volume.gravity.unwrap_or(0.0));
            out.push(u8::from(volume.push.is_some()));
            write_vec3(&mut out, volume.push.unwrap_or([0.0; 3]));
            write_string(&mut out, &volume.fluid);
            write_brushes(&mut out, &volume.brushes);
            write_count(&mut out, volume.surface_faces.len());
            for face in &volume.surface_faces {
                out.extend_from_slice(&face.texture_index.to_le_bytes());
                write_count(&mut out, face.vertices.len());
                for vertex in &face.vertices {
                    write_vec3(&mut out, vertex.position);
                    write_f32(&mut out, vertex.uv[0]);
                    write_f32(&mut out, vertex.uv[1]);
                }
            }
        }
        out
    }

    /// Decode untrusted section bytes. Every count is bounded by the bytes
    /// left before anything is allocated, and every float must be finite.
    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        let mut r = SectionReader::new(data, SECTION);
        let version = r.u16("version")?;
        if version != ENVIRONMENT_VOLUMES_VERSION {
            return r.invalid(format!("unsupported version {version}"));
        }
        let level_gravity = r.finite_f32("level gravity")?;
        let volume_count = r.bounded_count("volume count", MIN_VOLUME_SIZE)?;
        let mut volumes = Vec::with_capacity(volume_count);
        for i in 0..volume_count {
            volumes.push(read_volume(&mut r, i)?);
        }
        r.finish()?;
        Ok(Self {
            level_gravity,
            volumes,
        })
    }
}

fn read_volume(r: &mut SectionReader<'_>, i: usize) -> crate::Result<EnvironmentVolumeRecord> {
    let source_entity = r.u32(&format!("volume {i} source entity"))?;
    let priority = r.u32(&format!("volume {i} priority"))? as i32;
    let gravity_present = r.flag(&format!("volume {i} gravity flag"))?;
    let gravity = r.finite_f32(&format!("volume {i} gravity"))?;
    let gravity = optional(
        r,
        gravity_present,
        gravity,
        0.0,
        &format!("volume {i} gravity"),
    )?;
    let push_present = r.flag(&format!("volume {i} push flag"))?;
    let push = r.finite_vec3(&format!("volume {i} push"))?;
    let push = optional(r, push_present, push, [0.0; 3], &format!("volume {i} push"))?;
    let fluid = r.string(&format!("volume {i} fluid"))?;
    if gravity.is_none() && push.is_none() && fluid.is_empty() {
        return r.invalid(format!(
            "volume {i} sets no field; a volume sets gravity, push or a fluid"
        ));
    }

    let brushes = r.brushes(&format!("volume {i}"))?;

    let face_count = r.bounded_count(&format!("volume {i} surface face count"), MIN_FACE_SIZE)?;
    if fluid.is_empty() && face_count > 0 {
        return r.invalid(format!(
            "volume {i} is dry but carries {face_count} surface face(s); a dry volume has none"
        ));
    }
    let mut surface_faces = Vec::with_capacity(face_count);
    for f in 0..face_count {
        let what = format!("volume {i} surface face {f}");
        let texture_index = r.u32(&format!("{what} texture index"))?;
        let vertex_count = r.bounded_count(&format!("{what} vertex count"), VERTEX_SIZE)?;
        if vertex_count < MIN_VERTICES_PER_FACE {
            return r.invalid(format!(
                "{what} has {vertex_count} vertices; a polygon needs at least {MIN_VERTICES_PER_FACE}"
            ));
        }
        let mut vertices = Vec::with_capacity(vertex_count);
        for v in 0..vertex_count {
            let position = r.finite_vec3(&format!("{what} vertex {v} position"))?;
            let u = r.finite_f32(&format!("{what} vertex {v} u"))?;
            let v_coord = r.finite_f32(&format!("{what} vertex {v} v"))?;
            vertices.push(EnvironmentSurfaceVertex {
                position,
                uv: [u, v_coord],
            });
        }
        surface_faces.push(EnvironmentSurfaceFace {
            texture_index,
            vertices,
        });
    }

    Ok(EnvironmentVolumeRecord {
        source_entity,
        priority,
        gravity,
        push,
        fluid,
        brushes,
        surface_faces,
    })
}

/// An absent field's value is always written as zero; anything else is a
/// malformed section, not a value to ignore.
fn optional<T: PartialEq + Copy>(
    r: &SectionReader<'_>,
    present: bool,
    value: T,
    zero: T,
    what: &str,
) -> crate::Result<Option<T>> {
    match (present, value == zero) {
        (true, _) => Ok(Some(value)),
        (false, true) => Ok(None),
        (false, false) => r.invalid(format!("{what} is absent but its value is not zero")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SectionId;

    fn cube(min: f32, max: f32) -> EnvironmentBrush {
        let axis = |i: usize, sign: f32| {
            let mut normal = [0.0; 3];
            normal[i] = sign;
            EnvironmentPlane {
                normal,
                distance: if sign > 0.0 { max } else { -min },
            }
        };
        EnvironmentBrush {
            aabb_min: [min; 3],
            aabb_max: [max; 3],
            planes: (0..3).flat_map(|i| [axis(i, 1.0), axis(i, -1.0)]).collect(),
        }
    }

    /// A gravity room, a push shaft, and a two-brush pool with surface faces,
    /// so every presence flag and every nested count is exercised.
    fn sample() -> EnvironmentVolumesSection {
        EnvironmentVolumesSection {
            level_gravity: -9.81,
            volumes: vec![
                EnvironmentVolumeRecord {
                    source_entity: 3,
                    priority: 0,
                    gravity: Some(-3.0),
                    push: None,
                    fluid: String::new(),
                    brushes: vec![cube(-4.0, 4.0)],
                    surface_faces: Vec::new(),
                },
                EnvironmentVolumeRecord {
                    source_entity: 4,
                    priority: -2,
                    gravity: None,
                    push: Some([0.0, 15.0, -8.0]),
                    fluid: String::new(),
                    brushes: vec![cube(0.0, 1.0)],
                    surface_faces: Vec::new(),
                },
                EnvironmentVolumeRecord {
                    source_entity: 7,
                    priority: 5,
                    gravity: None,
                    push: None,
                    fluid: "water".into(),
                    brushes: vec![cube(-1.0, 0.0), cube(0.0, 1.0)],
                    surface_faces: vec![EnvironmentSurfaceFace {
                        texture_index: 2,
                        vertices: vec![
                            EnvironmentSurfaceVertex {
                                position: [-1.0, 0.0, -1.0],
                                uv: [0.0, 0.0],
                            },
                            EnvironmentSurfaceVertex {
                                position: [1.0, 0.0, -1.0],
                                uv: [2.0, 0.0],
                            },
                            EnvironmentSurfaceVertex {
                                position: [1.0, 0.0, 1.0],
                                uv: [2.0, 2.0],
                            },
                        ],
                    }],
                },
            ],
        }
    }

    /// Byte offset of volume 0's gravity flag: version, level gravity, count,
    /// source entity, priority.
    const FIRST_GRAVITY_FLAG: usize = 2 + 4 + 4 + 4 + 4;

    fn assert_rejected(bytes: &[u8], expected: &str) {
        let error = EnvironmentVolumesSection::from_bytes(bytes)
            .expect_err("malformed environment volumes must be rejected");
        assert!(
            error.to_string().contains(expected),
            "expected an error mentioning `{expected}`, got: {error}"
        );
    }

    #[test]
    fn section_id_is_52() {
        assert_eq!(SectionId::EnvironmentVolumes as u32, 52);
        assert_eq!(SectionId::from_u32(52), Some(SectionId::EnvironmentVolumes));
    }

    #[test]
    fn environment_volumes_round_trip_every_field_and_byte_len_matches() {
        let section = sample();
        let bytes = section.to_bytes();
        assert_eq!(section.byte_len(), bytes.len());
        assert_eq!(
            EnvironmentVolumesSection::from_bytes(&bytes).unwrap(),
            section
        );
    }

    #[test]
    fn a_level_with_no_volumes_still_carries_its_default_gravity() {
        let section = EnvironmentVolumesSection {
            level_gravity: 4.0,
            volumes: Vec::new(),
        };
        let bytes = section.to_bytes();
        assert_eq!(bytes.len(), 10, "version + gravity + zero count");
        let decoded = EnvironmentVolumesSection::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.level_gravity, 4.0);
        assert!(decoded.volumes.is_empty());
    }

    #[test]
    fn wire_layout_follows_the_documented_field_order() {
        let section = EnvironmentVolumesSection {
            level_gravity: -9.81,
            volumes: vec![sample().volumes.swap_remove(1)],
        };
        let bytes = section.to_bytes();
        let f32_at = |o: usize| f32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
        let u32_at = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
        assert_eq!(u16::from_le_bytes([bytes[0], bytes[1]]), 1);
        assert_eq!(f32_at(2), -9.81);
        assert_eq!(u32_at(6), 1, "volume count");
        assert_eq!(u32_at(10), 4, "source entity");
        assert_eq!(u32_at(14) as i32, -2, "priority");
        assert_eq!(bytes[18], 0, "gravity absent");
        assert_eq!(f32_at(19), 0.0, "absent gravity written as zero");
        assert_eq!(bytes[23], 1, "push present");
        assert_eq!([f32_at(24), f32_at(28), f32_at(32)], [0.0, 15.0, -8.0]);
        assert_eq!(u32_at(36), 0, "empty fluid name = dry");
        assert_eq!(u32_at(40), 1, "brush count");
    }

    #[test]
    fn rejects_unsupported_version() {
        let mut bytes = sample().to_bytes();
        bytes[..2].copy_from_slice(&2_u16.to_le_bytes());
        assert_rejected(&bytes, "unsupported version 2");
    }

    #[test]
    fn rejects_trailing_bytes() {
        let mut bytes = sample().to_bytes();
        bytes.push(0);
        assert_rejected(&bytes, "trailing bytes");
    }

    #[test]
    fn rejects_every_truncation() {
        let bytes = sample().to_bytes();
        for len in 0..bytes.len() {
            assert!(
                EnvironmentVolumesSection::from_bytes(&bytes[..len]).is_err(),
                "a section truncated to {len} of {} bytes must be rejected",
                bytes.len()
            );
        }
    }

    #[test]
    fn rejects_counts_the_remaining_bytes_cannot_hold_before_allocating() {
        let mut bytes = EnvironmentVolumesSection {
            level_gravity: -9.81,
            volumes: Vec::new(),
        }
        .to_bytes();
        bytes[6..10].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_rejected(&bytes, "volume count");

        let mut bytes = sample().to_bytes();
        // Volume 0's brush count follows its empty fluid name.
        let brush_count = FIRST_GRAVITY_FLAG + 1 + 4 + 1 + 12 + 4;
        bytes[brush_count..brush_count + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_rejected(&bytes, "brush count");
    }

    #[test]
    fn rejects_presence_flags_other_than_zero_or_one() {
        let mut bytes = sample().to_bytes();
        bytes[FIRST_GRAVITY_FLAG] = 2;
        assert_rejected(&bytes, "gravity flag must be 0 or 1");

        let mut bytes = sample().to_bytes();
        bytes[FIRST_GRAVITY_FLAG + 5] = 7;
        assert_rejected(&bytes, "push flag must be 0 or 1");
    }

    #[test]
    fn rejects_an_absent_field_with_a_nonzero_value() {
        let mut bytes = sample().to_bytes();
        // Volume 0 sets gravity, not push: write a push value under a 0 flag.
        let push_x = FIRST_GRAVITY_FLAG + 1 + 4 + 1;
        bytes[push_x..push_x + 4].copy_from_slice(&1.0_f32.to_le_bytes());
        assert_rejected(&bytes, "push is absent but its value is not zero");
    }

    #[test]
    fn rejects_non_finite_floats() {
        let mut bytes = sample().to_bytes();
        bytes[2..6].copy_from_slice(&f32::NAN.to_le_bytes());
        assert_rejected(&bytes, "level gravity is not finite");

        let mut bytes = sample().to_bytes();
        let gravity = FIRST_GRAVITY_FLAG + 1;
        bytes[gravity..gravity + 4].copy_from_slice(&f32::INFINITY.to_le_bytes());
        assert_rejected(&bytes, "volume 0 gravity is not finite");
    }

    #[test]
    fn rejects_inverted_aabbs_open_leaves_and_degenerate_faces() {
        let mut section = sample();
        section.volumes[0].brushes[0].aabb_min[1] = 10.0;
        assert_rejected(&section.to_bytes(), "AABB min exceeds max");

        let mut section = sample();
        section.volumes[0].brushes[0].planes.truncate(3);
        assert_rejected(&section.to_bytes(), "at least 4");

        let mut section = sample();
        section.volumes[0].brushes.clear();
        assert_rejected(&section.to_bytes(), "has no brushes");

        let mut section = sample();
        section.volumes[2].surface_faces[0].vertices.truncate(2);
        assert_rejected(&section.to_bytes(), "a polygon needs at least 3");
    }

    #[test]
    fn rejects_a_dry_volume_carrying_surface_faces() {
        let mut section = sample();
        let faces = section.volumes[2].surface_faces.clone();
        section.volumes[0].surface_faces = faces;
        assert_rejected(
            &section.to_bytes(),
            "volume 0 is dry but carries 1 surface face(s)",
        );
    }

    #[test]
    fn rejects_a_volume_that_sets_no_field() {
        let mut section = sample();
        section.volumes[0].gravity = None;
        assert_rejected(&section.to_bytes(), "volume 0 sets no field");
    }

    #[test]
    fn rejects_invalid_utf8_fluid_names() {
        let mut section = sample();
        section.volumes[2].fluid = "wat".into();
        let mut bytes = section.to_bytes();
        let at = bytes
            .windows(3)
            .position(|w| w == b"wat")
            .expect("fluid name is in the payload");
        bytes[at] = 0xFF;
        assert_rejected(&bytes, "invalid UTF-8");
    }
}
