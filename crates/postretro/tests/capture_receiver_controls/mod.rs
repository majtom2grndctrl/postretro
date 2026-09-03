// Same-bake receiver controls and CPU-projected rest-pose triangle masks.
// See: context/lib/testing_guide.md §3

use std::fs::File;
use std::path::Path;

use glam::{EulerRot, Mat4, Quat, Vec2, Vec3};
use postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection;
use postretro_level_format::sh_reconstruct::{Level, stored_delta_tiles};
use postretro_level_format::{SectionBlob, SectionId};

#[derive(Clone, Copy)]
pub(super) enum Receiver {
    Mover,
    Prop,
}

impl Receiver {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Mover => "closet mover",
            Self::Prop => "prop_mesh",
        }
    }
}

/// Keep the control under the same content root and copy every section byte
/// except the one receiver record. No rebake can change the lighting baseline.
pub(super) fn without_receiver(map: &Path, receiver: Receiver) -> tempfile::TempPath {
    let mut source = File::open(map).expect("open capture PRL");
    let meta = postretro_level_format::read_container(&mut source).expect("read PRL container");
    let mut sections = Vec::new();
    for entry in &meta.sections {
        let mut data =
            postretro_level_format::read_section_data(&mut source, &meta, entry.section_id)
                .expect("read PRL section")
                .expect("listed section exists");
        match receiver {
            Receiver::Mover if entry.section_id == SectionId::KinematicGeometry as u32 => {
                let mut section = postretro_level_format::kinematic_geometry::KinematicGeometrySection::from_bytes(&data)
                    .expect("decode movers");
                let before = section.movers.len();
                section
                    .movers
                    .retain(|mover| !mover.tags.iter().any(|tag| tag == "closet_door"));
                assert_eq!(
                    section.movers.len() + 1,
                    before,
                    "control removes exactly the closet door"
                );
                data = section.to_bytes();
            }
            Receiver::Prop if entry.section_id == SectionId::MapEntity as u32 => {
                let mut section =
                    postretro_level_format::map_entity::MapEntitySection::from_bytes(&data)
                        .expect("decode map entities");
                let before = section.entries.len();
                section
                    .entries
                    .retain(|entity| entity.classname != "prop_mesh");
                assert_eq!(
                    section.entries.len() + 1,
                    before,
                    "fixture has exactly one prop_mesh"
                );
                data = section.to_bytes();
            }
            _ => {}
        }
        sections.push(SectionBlob {
            section_id: entry.section_id,
            version: entry.version,
            data,
        });
    }
    let mut output = tempfile::Builder::new()
        .prefix(".capture-receiver-control-")
        .suffix(".prl")
        .tempfile_in(map.parent().expect("map parent"))
        .expect("reserve receiver control PRL");
    postretro_level_format::write_prl(output.as_file_mut(), &sections)
        .expect("write receiver control");
    output.into_temp_path()
}

/// Zero only this light's direct-SH payload. Keep its baked rest descriptor,
/// indirect SH, world-lighting payloads, and every receiver unchanged.
pub(super) fn without_animated_direct(map: &Path, slot: u32) -> tempfile::TempPath {
    let mut source = File::open(map).expect("open capture PRL");
    let meta = postretro_level_format::read_container(&mut source).expect("read PRL container");
    let mut sections = Vec::new();
    let mut found = false;
    for entry in &meta.sections {
        let mut data =
            postretro_level_format::read_section_data(&mut source, &meta, entry.section_id)
                .expect("read PRL section")
                .expect("listed section exists");
        if entry.section_id == SectionId::AnimatedDirectShDeltaVolumes as u32 {
            let mut section = AnimatedDirectShDeltaVolumesSection::from_bytes(&data)
                .expect("decode animated direct SH");
            zero_animated_direct_light(&mut section, slot);
            data = section.to_bytes();
            found = true;
        }
        sections.push(SectionBlob {
            section_id: entry.section_id,
            version: entry.version,
            data,
        });
    }
    assert!(found, "fixture must contain animated direct SH");
    let mut output = tempfile::Builder::new()
        .prefix(".capture-direct-control-")
        .suffix(".prl")
        .tempfile_in(map.parent().expect("map parent"))
        .expect("reserve direct control PRL");
    postretro_level_format::write_prl(output.as_file_mut(), &sections)
        .expect("write direct control");
    output.into_temp_path()
}

fn zero_animated_direct_light(section: &mut AnimatedDirectShDeltaVolumesSection, slot: u32) {
    let mut cursor = 0;
    let mut changed = false;
    for cell in 0..section.affinity_cell_count() {
        let level = Level::from_u8(section.cell_levels[cell]).expect("valid cell level");
        let stride = stored_delta_tiles(level, section.valid_probe_masks[cell])
            * section.delta_probe_f16_stride();
        for entry in section.affinity_offsets[cell]..section.affinity_offsets[cell + 1] {
            let light = section.affinity_lights[entry as usize] as usize;
            let payload = &mut section.delta_subblocks[cursor..cursor + stride];
            if section.animation_descriptor_indices[light] == slot {
                changed |= payload.iter().any(|&half| half != 0);
                payload.fill(0);
            }
            cursor += stride;
        }
    }
    assert_eq!(cursor, section.delta_subblocks.len());
    assert!(
        changed,
        "fixture alarm must have nonzero animated direct SH"
    );
}

#[test]
fn animated_direct_control_preserves_other_lights_and_section_layout() {
    let mut section = AnimatedDirectShDeltaVolumesSection {
        affinity_factor: 4,
        affinity_dims: [2, 1, 1],
        tile_dimension: 6,
        tile_border: 1,
        animation_descriptor_indices: vec![7, 9],
        valid_probe_masks: vec![1, 3],
        cell_levels: vec![0, 0],
        affinity_offsets: vec![0, 2, 3],
        affinity_lights: vec![0, 1, 0],
        delta_subblocks: vec![0x3c00; 4 * 6 * 6 * 4],
    };
    let mut expected = section.clone();
    let stride = section.delta_probe_f16_stride();
    expected.delta_subblocks[..stride].fill(0);
    expected.delta_subblocks[2 * stride..].fill(0);
    zero_animated_direct_light(&mut section, 7);
    assert_eq!(section, expected);
    assert_eq!(
        AnimatedDirectShDeltaVolumesSection::from_bytes(&section.to_bytes()).unwrap(),
        expected
    );
}

pub(super) fn receiver_mask(map: &Path, workspace: &Path, receiver: Receiver) -> Vec<(u32, u32)> {
    let world =
        postretro_level_loader::load_prl(&map.to_string_lossy()).expect("load capture fixture");
    let (positions, indices) = match receiver {
        Receiver::Mover => {
            let mover = world
                .kinematic_geometry
                .movers
                .iter()
                .find(|mover| mover.tags.iter().any(|tag| tag == "closet_door"))
                .expect("fixture contains closet door");
            (
                mover
                    .vertices
                    .iter()
                    .map(|vertex| Vec3::from_array(vertex.position) + mover.origin)
                    .collect::<Vec<_>>(),
                mover.indices.clone(),
            )
        }
        Receiver::Prop => {
            let entity = world
                .map_entities
                .iter()
                .find(|entity| entity.classname == "prop_mesh")
                .expect("fixture contains prop_mesh");
            let model_path = &entity
                .key_values
                .iter()
                .find(|(key, _)| key == "model")
                .expect("prop_mesh declares a model")
                .1;
            let model = postretro_model::gltf_loader::load_model(
                &workspace.join("content/dev").join(model_path),
            )
            .expect("fixture prop model must load and contain drawable geometry");
            assert!(
                !model.mesh.indices.is_empty(),
                "prop model must have triangles"
            );
            let mut palette = Vec::new();
            postretro_model::anim::sample_rest_pose(&model.skeleton, &mut palette);
            let [pitch, yaw, roll] = entity.angles;
            let transform = Mat4::from_rotation_translation(
                Quat::from_euler(EulerRot::YXZ, yaw, pitch, roll),
                Vec3::from_array(entity.origin),
            );
            let positions = model
                .mesh
                .vertices
                .iter()
                .map(|vertex| {
                    let position = Vec3::from_array(vertex.position).extend(1.0);
                    let mut skinned = glam::Vec4::ZERO;
                    for (&joint, &weight) in vertex.joints.iter().zip(&vertex.weights) {
                        skinned += Mat4::from_cols_array_2d(&palette[joint as usize].matrix)
                            * position
                            * (f32::from(weight) / 255.0);
                    }
                    (transform * skinned).truncate()
                })
                .collect();
            (positions, model.mesh.indices)
        }
    };
    project_triangle_mask(&positions, &indices)
}

fn project_triangle_mask(positions: &[Vec3], indices: &[u32]) -> Vec<(u32, u32)> {
    let width = super::RECEIVER_CAPTURE_WIDTH;
    let height = super::RECEIVER_CAPTURE_HEIGHT;
    let eye = Vec3::from_array(super::RECEIVER_EYE);
    let yaw = super::RECEIVER_YAW_DEG.to_radians();
    let pitch = super::RECEIVER_PITCH_DEG.to_radians();
    let direction = Vec3::new(
        -yaw.sin() * pitch.cos(),
        pitch.sin(),
        -yaw.cos() * pitch.cos(),
    );
    let aspect = width as f32 / height as f32;
    let vfov = 2.0 * ((super::RECEIVER_FOV_DEG.to_radians() / 2.0).tan() / aspect).atan();
    let view_proj = Mat4::perspective_rh(vfov, aspect, 0.01, 1000.0)
        * Mat4::look_at_rh(eye, eye + direction, Vec3::Y);
    let screen: Vec<_> = positions
        .iter()
        .map(|&position| {
            let clip = view_proj * position.extend(1.0);
            assert!(
                clip.w > 0.0,
                "capture fixture receiver must be in front of camera"
            );
            let ndc = clip.truncate() / clip.w;
            Vec2::new(
                (ndc.x + 1.0) * 0.5 * width as f32,
                (1.0 - ndc.y) * 0.5 * height as f32,
            )
        })
        .collect();
    let mut mask = vec![false; (width * height) as usize];
    for triangle in indices.chunks_exact(3) {
        let [a, b, c] = [
            screen[triangle[0] as usize],
            screen[triangle[1] as usize],
            screen[triangle[2] as usize],
        ];
        let area = (b - a).perp_dot(c - a);
        if area.abs() < 1.0e-5 {
            continue;
        }
        let minimum = a.min(b).min(c).floor().max(Vec2::ZERO);
        let maximum = a
            .max(b)
            .max(c)
            .ceil()
            .min(Vec2::new(width as f32, height as f32));
        for y in minimum.y as u32..maximum.y as u32 {
            for x in minimum.x as u32..maximum.x as u32 {
                let pixel = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                let u = (b - pixel).perp_dot(c - pixel) / area;
                let v = (c - pixel).perp_dot(a - pixel) / area;
                let w = 1.0 - u - v;
                if u >= 0.0 && v >= 0.0 && w >= 0.0 {
                    mask[(y * width + x) as usize] = true;
                }
            }
        }
    }
    // Erode the union by one pixel so rasterization edge rounding or bloom
    // outside a silhouette cannot count as a receiver sample.
    let mut pixels = Vec::new();
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            if (y - 1..=y + 1).all(|sy| (x - 1..=x + 1).all(|sx| mask[(sy * width + sx) as usize]))
            {
                pixels.push((x, y));
            }
        }
    }
    assert!(
        pixels.len() >= 64,
        "fixture receiver must project to a substantial interior"
    );
    pixels
}
