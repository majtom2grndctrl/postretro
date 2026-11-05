// UV normalize: texel-space world and mover UVs to `[0,1]` by each
// material's diffuse dimensions, readable from prepared textures alone.
// See: context/lib/rendering_pipeline.md §3 · context/lib/boot_sequence.md §3

use crate::texture_prep::{PLACEHOLDER_DIFFUSE_SIZE, PreparedTexture, PreparedWorldTextures};

/// Each bound world material slot's diffuse `(width, height)` as the GPU
/// pass will upload it, parallel to `prepared.entries`: a bundle whose plan
/// consumes a readable diffuse slot uploads that slot's mip 0; every other
/// entry binds the checkerboard placeholder. A level with no materials still
/// binds one placeholder slot, so its dims are that one placeholder's: UV
/// normalize and the fluid pack read the slots the level binds.
pub fn diffuse_dims(prepared: &PreparedWorldTextures) -> Vec<(u32, u32)> {
    if prepared.entries.is_empty() {
        return vec![(PLACEHOLDER_DIFFUSE_SIZE, PLACEHOLDER_DIFFUSE_SIZE)];
    }
    prepared.entries.iter().map(prepared_diffuse_dims).collect()
}

fn prepared_diffuse_dims(entry: &PreparedTexture) -> (u32, u32) {
    match entry {
        PreparedTexture::Bundle { slots, plan, .. } if plan.consume[0] => match &slots[0] {
            Ok(slot) => (u32::from(slot.width), u32::from(slot.height)),
            Err(_) => (PLACEHOLDER_DIFFUSE_SIZE, PLACEHOLDER_DIFFUSE_SIZE),
        },
        _ => (PLACEHOLDER_DIFFUSE_SIZE, PLACEHOLDER_DIFFUSE_SIZE),
    }
}

/// Normalize the level's static world and kinematic mover UVs in place by
/// `dims` (one entry per world material; a missing or zero entry leaves that
/// material's UVs raw). Runs once, before geometry upload.
pub fn normalize_world_uvs_with_dims(
    world: &mut postretro_level_loader::LevelWorld,
    dims: &[(u32, u32)],
) {
    normalize_static_world_uvs(&mut world.vertices, &world.indices, &world.bvh.leaves, dims);
    normalize_kinematic_mover_uvs(&mut world.kinematic_geometry.movers, dims);
}

fn texture_dimensions_for_index(
    texture_dimensions: &[(u32, u32)],
    texture_index: u32,
) -> Option<(f32, f32)> {
    let dimensions = texture_dimensions.get(texture_index as usize)?;
    let (width, height) = *dimensions;
    if width == 0 || height == 0 {
        return None;
    }
    Some((width as f32, height as f32))
}

fn normalize_static_world_uvs(
    vertices: &mut [postretro_render_data::geometry::WorldVertex],
    indices: &[u32],
    leaves: &[postretro_render_data::geometry::BvhLeaf],
    texture_dimensions: &[(u32, u32)],
) {
    let mut normalized = vec![false; vertices.len()];
    for leaf in leaves {
        let Some((width, height)) =
            texture_dimensions_for_index(texture_dimensions, leaf.material_bucket_id)
        else {
            continue;
        };
        let start = leaf.index_offset as usize;
        let end = start.saturating_add(leaf.index_count as usize);
        for i in start..end {
            if let Some(&idx) = indices.get(i) {
                let vertex_index = idx as usize;
                if vertex_index < normalized.len()
                    && !normalized[vertex_index]
                    && let Some(vertex) = vertices.get_mut(vertex_index)
                {
                    vertex.base_uv[0] /= width;
                    vertex.base_uv[1] /= height;
                    normalized[vertex_index] = true;
                }
            }
        }
    }
}

fn normalize_kinematic_mover_uvs(
    movers: &mut [postretro_level_loader::LoadedKinematicMover],
    texture_dimensions: &[(u32, u32)],
) {
    for mover in movers {
        let mut normalized = vec![false; mover.vertices.len()];
        let mut offset = 0usize;
        for face in &mover.face_meta {
            if offset + 2 >= mover.indices.len() {
                break;
            }
            let face_base = mover.indices[offset];
            let start = offset;
            while offset + 2 < mover.indices.len() && mover.indices[offset] == face_base {
                offset += 3;
            }
            normalize_kinematic_face_uvs(
                &mut mover.vertices,
                &mover.indices[start..offset],
                face.texture_index,
                texture_dimensions,
                &mut normalized,
            );
        }
    }
}

fn normalize_kinematic_face_uvs(
    vertices: &mut [postretro_level_format::geometry::Vertex],
    indices: &[u32],
    texture_index: u32,
    texture_dimensions: &[(u32, u32)],
    normalized: &mut [bool],
) {
    let Some((width, height)) = texture_dimensions_for_index(texture_dimensions, texture_index)
    else {
        return;
    };
    for &index in indices {
        let vertex_index = index as usize;
        if vertex_index < normalized.len()
            && !normalized[vertex_index]
            && let Some(vertex) = vertices.get_mut(vertex_index)
        {
            vertex.uv[0] /= width;
            vertex.uv[1] /= height;
            normalized[vertex_index] = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use postretro_level_format::geometry::{FaceMeta, Vertex};
    use postretro_render_data::geometry::{BvhLeaf, WorldVertex};

    fn world_vertex(base_uv: [f32; 2]) -> WorldVertex {
        WorldVertex {
            position: [0.0, 0.0, 0.0],
            base_uv,
            normal_oct: [0, 0],
            tangent_packed: [0, 0],
            lightmap_uv: [0, 0],
            lightmap_block: 0,
            animated_block: 0,
        }
    }

    fn kinematic_vertex(uv: [f32; 2]) -> Vertex {
        Vertex::new(
            [0.0, 0.0, 0.0],
            uv,
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            [0.0, 0.0],
            0,
        )
    }

    fn loaded_mover(
        vertices: Vec<Vertex>,
        indices: Vec<u32>,
        face_meta: Vec<FaceMeta>,
    ) -> postretro_level_loader::LoadedKinematicMover {
        postretro_level_loader::LoadedKinematicMover {
            mover_id: 7,
            name: "lift".to_string(),
            tags: Vec::new(),
            origin: Vec3::ZERO,
            path: "a".to_string(),
            speed_mps: 1.0,
            wait_ms: 0.0,
            move_mode: 0,
            start_on_spawn: true,
            vertices,
            indices,
            face_meta,
            spin_axis: Vec3::ZERO,
            spin_speed_deg_s: 0.0,
            spin_accel_deg_s2: 0.0,
            carry_yaw: false,
            block_policy: "displace".to_string(),
            crush_damage: 0.0,
            crush_interval_ms: 0.0,
            auto_close_ms: None,
            open_event: None,
            close_event: None,
            blocked_event: None,
            crush_event: None,
            open_sound: None,
            close_sound: None,
            blocked_sound: None,
            crush_sound: None,
            sealed_portal_ids: Vec::new(),
            carried_lights: Vec::new(),
            crush_damage_type: None,
        }
    }

    fn assert_uv_approx(actual: [f32; 2], expected: [f32; 2]) {
        const EPSILON: f32 = 0.000_001;
        assert!(
            (actual[0] - expected[0]).abs() <= EPSILON
                && (actual[1] - expected[1]).abs() <= EPSILON,
            "actual={actual:?} expected={expected:?}",
        );
    }

    #[test]
    fn static_world_uvs_normalize_once_per_bvh_vertex() {
        let mut vertices = vec![
            world_vertex([64.0, 16.0]),
            world_vertex([32.0, 32.0]),
            world_vertex([16.0, 8.0]),
        ];
        let indices = vec![0, 1, 2, 0, 2, 1];
        let leaves = vec![BvhLeaf {
            aabb_min: [0.0, 0.0, 0.0],
            material_bucket_id: 0,
            aabb_max: [1.0, 1.0, 1.0],
            index_offset: 0,
            index_count: 6,
            cell_id: 0,
            chunk_range_start: 0,
            chunk_range_count: 0,
        }];

        normalize_static_world_uvs(&mut vertices, &indices, &leaves, &[(64, 32)]);

        assert_uv_approx(vertices[0].base_uv, [1.0, 0.5]);
        assert_uv_approx(vertices[1].base_uv, [0.5, 1.0]);
        assert_uv_approx(vertices[2].base_uv, [0.25, 0.25]);
    }

    #[test]
    fn kinematic_mover_uvs_normalize_by_face_texture_dimensions() {
        let mut movers = vec![loaded_mover(
            vec![
                kinematic_vertex([64.0, 32.0]),
                kinematic_vertex([32.0, 16.0]),
                kinematic_vertex([128.0, 64.0]),
                kinematic_vertex([16.0, 8.0]),
                kinematic_vertex([128.0, 64.0]),
                kinematic_vertex([64.0, 32.0]),
                kinematic_vertex([32.0, 16.0]),
            ],
            vec![0, 1, 2, 0, 2, 3, 4, 5, 6],
            vec![
                FaceMeta {
                    leaf_index: 0,
                    texture_index: 1,
                },
                FaceMeta {
                    leaf_index: 0,
                    texture_index: 2,
                },
            ],
        )];

        normalize_kinematic_mover_uvs(&mut movers, &[(32, 16), (128, 64), (256, 128)]);

        let vertices = &movers[0].vertices;
        assert_uv_approx(vertices[0].uv, [0.5, 0.5]);
        assert_uv_approx(vertices[1].uv, [0.25, 0.25]);
        assert_uv_approx(vertices[2].uv, [1.0, 1.0]);
        assert_uv_approx(vertices[3].uv, [0.125, 0.125]);
        assert_uv_approx(vertices[4].uv, [0.5, 0.5]);
        assert_uv_approx(vertices[5].uv, [0.25, 0.25]);
        assert_uv_approx(vertices[6].uv, [0.125, 0.125]);
    }

    #[test]
    fn diffuse_dims_read_consumed_diffuse_slots_and_placeholders() {
        use crate::loaded_texture::TextureSlotPlan;
        use crate::texture_prep::{PreparedTexture, PreparedWorldTextures};
        use postretro_level_format::prm::{PrmFormat, PrmReadError, PrmSlot};

        let slot = |width, height| PrmSlot {
            format: PrmFormat::Rgba8UnormSrgb,
            width,
            height,
            level_count: 1,
            payload: vec![0; usize::from(width) * usize::from(height) * 4],
        };
        let bundle =
            |diffuse: Result<PrmSlot, PrmReadError>, consume_diffuse| PreparedTexture::Bundle {
                name: "t".to_owned(),
                slots: [
                    diffuse,
                    Err(PrmReadError::NotPresent),
                    Err(PrmReadError::NotPresent),
                    Err(PrmReadError::NotPresent),
                ],
                plan: TextureSlotPlan {
                    consume: [consume_diffuse, true, true, true],
                    mip_count: 1,
                },
            };
        let prepared = PreparedWorldTextures {
            entries: vec![
                bundle(Ok(slot(32, 8)), true),
                PreparedTexture::Placeholder,
                bundle(Err(PrmReadError::NotPresent), true),
                bundle(Ok(slot(16, 16)), false),
            ],
            timing: Default::default(),
        };
        assert_eq!(
            diffuse_dims(&prepared),
            [(32, 8), (64, 64), (64, 64), (64, 64)]
        );
        // No materials still binds the one placeholder slot.
        let empty = PreparedWorldTextures {
            entries: Vec::new(),
            timing: Default::default(),
        };
        assert_eq!(diffuse_dims(&empty), [(64, 64)]);
    }
}
