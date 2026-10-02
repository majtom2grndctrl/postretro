// Real-map upload inventory inputs: SDF section and promoted-light receiver pose.
// See: context/lib/rendering_pipeline.md §4

use super::{Renderer, Vec3};

/// Keep the world mesh inside an authored promotion candidate's real
/// influence. A camera near that light gives it a strong score in the normal
/// shared-pool ranker; no promotion state or cache record is injected.
pub(super) fn promotion_receiver_pose(renderer: &Renderer) -> (usize, Vec3, Vec3) {
    use postretro_level_loader::{LightType, ShadowType};
    let full = renderer.full();
    let (candidate, light) = full
        .shadow_candidate_lights
        .iter()
        .enumerate()
        .filter(|(index, light)| {
            full.shadow_candidate_selection_indices[*index].is_some()
                && !light.is_dynamic
                && light.shadow_type == ShadowType::StaticLightMap
                && light.cell_index
                    != postretro_level_format::alpha_lights::ALPHA_LIGHT_LEAF_UNASSIGNED
                && light.falloff_range > 0.0
                && (light.light_type == LightType::Spot
                    || (light.light_type == LightType::Point && full.cube_shadow_pool.is_some()))
                && full
                    .shadow_candidate_influences
                    .get(*index)
                    .is_some_and(|influence| {
                        influence.center.is_finite() && influence.radius.is_finite()
                    })
        })
        .min_by_key(|(_, light)| u8::from(light.light_type != LightType::Spot))
        .expect("campaign needs a selected static light with a supported depth pool");
    let selection = full.shadow_candidate_selection_indices[candidate].unwrap();
    let receiver = full.shadow_candidate_influences[candidate].center;
    let eye = Vec3::from_array(light.origin.map(|coordinate| coordinate as f32))
        + Vec3::new(0.0, 0.125, 0.25);
    assert!(eye.is_finite() && eye.distance_squared(receiver) > 0.0);
    (selection, receiver, eye)
}

/// Campaign's generated PRL has no id-33 section. Supply one real apron'd
/// surface brick through the normal level-install input, using the same v2
/// shape as level-loader's SdfAtlasSection fixture. The field is a signed
/// distance to x=0 in a one-metre cube; no renderer flags are overridden.
pub(super) fn add_sdf_inventory_fixture(world: &mut postretro_level_loader::LevelWorld) {
    use postretro_level_format::sdf_atlas::SdfAtlasSection;
    if world.sdf_atlas.is_none() {
        let stored_edge = 6usize; // Four interior voxels plus the v2 apron.
        let atlas = (0..stored_edge.pow(3))
            .map(|index| ((index % stored_edge) as i16 * 256) - 640)
            .collect();
        let section = SdfAtlasSection {
            world_min: [-0.5; 3],
            world_max: [0.5; 3],
            voxel_size_m: 0.25,
            brick_size_voxels: 4,
            grid_dims: [1; 3],
            atlas_bricks_per_axis: [1; 3],
            surface_brick_count: 1,
            top_level: vec![0],
            atlas,
            coarse_distances: vec![0.0],
        };
        let decoded = SdfAtlasSection::from_bytes(&section.to_bytes())
            .expect("small SDF input must satisfy the real section schema");
        assert_eq!(decoded, section);
        world.sdf_atlas = Some(section);
    }
    // Reuse an authored static light so its original chunk-list membership
    // remains valid. An appended light would be absent from the baked lists.
    let light = world
        .lights
        .iter_mut()
        .find(|light| {
            !light.is_dynamic
                && light.intensity > 0.0
                && light.light_type != postretro_level_loader::LightType::Directional
        })
        .expect("campaign fixture must contain an eligible static SDF light");
    light.shadow_type = postretro_level_loader::ShadowType::Sdf;
}
