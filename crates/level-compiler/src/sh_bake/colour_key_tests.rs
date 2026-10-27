// PNG-derived material dependencies at the real SH group and delta cache boundaries.
// See: context/lib/build_pipeline.md §Compiler pipeline

use super::*;
use crate::cache::{CacheKey, StageCache};
use crate::delta_sh_bake::{INDIRECT_DELTA_SH_STAGE_ID, INDIRECT_DELTA_SH_STAGE_VERSION};
use crate::delta_sh_cache::{DeltaShEntryKeyInputs, delta_sh_entry_cache_key};
use crate::sh_group::{
    SH_GROUP_DIM, SH_GROUP_STAGE_VERSION, bounce_material_hash, geometry_content_hash,
    group_cache_key, partition_groups,
};

fn paint(room: &Room, name: &str, rgba: [u8; 4]) {
    let path = room.dir.path().join("textures").join(format!("{name}.png"));
    let mut encoder = png::Encoder::new(std::fs::File::create(path).unwrap(), 16, 16);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&rgba.repeat(256))
        .unwrap();
}

/// Two real group keys and two real delta-entry keys. This intentionally small
/// layout exercises spatially distinct entries without baking a second volume.
fn keys(
    room: &Room,
    geometry: &GeometryResult,
    materials: Option<&BounceMaterials>,
    delta_version: u32,
) -> Vec<CacheKey> {
    let dims = [SH_GROUP_DIM + 1, 1, 1];
    let layout = ProbeGridLayout {
        world_min: DVec3::ZERO,
        dims,
        cell_size: [1.0; 3],
        far_sentinel: 100.0,
        probe_positions: (0..dims[0])
            .map(|x| DVec3::new(f64::from(x), 0.0, 0.0))
            .collect(),
        validity: vec![1; dims[0] as usize],
    };
    let geometry_hash = geometry_content_hash(geometry);
    let hash = bounce_material_hash(materials, geometry);
    let reaching = [(0, room.static_light())];
    let mut keys: Vec<_> = partition_groups(dims)
        .iter()
        .map(|group| {
            group_cache_key(
                group,
                &reaching,
                &layout,
                1.0,
                &geometry_hash,
                hash.as_ref(),
            )
        })
        .collect();
    assert_eq!(keys.len(), 2, "test layout must cover two distinct groups");
    let animated = room
        .pipeline
        .lights
        .iter()
        .find(|light| light.animation.is_some())
        .unwrap();
    let unit = unit_radiance_light(animated);
    keys.extend((0..2).map(|cell| {
        delta_sh_entry_cache_key(&DeltaShEntryKeyInputs {
            bounce_hash: hash.as_ref(),
            stage_id: INDIRECT_DELTA_SH_STAGE_ID,
            stage_version: delta_version,
            geometry_hash: &geometry_hash,
            affinity_dims: [2, 1, 1],
            cell,
            probe_spacing: 1.0,
            valid_probe_mask: u64::MAX,
            seed_axis: indirect_light_axis(&unit),
            light: &unit,
        })
    }));
    keys
}

fn current_keys(room: &Room, materials: &BounceMaterials) -> Vec<CacheKey> {
    keys(
        room,
        &room.pipeline.geometry,
        Some(materials),
        INDIRECT_DELTA_SH_STAGE_VERSION,
    )
}

fn filenames(keys: &[CacheKey]) -> Vec<String> {
    keys.iter().map(CacheKey::as_filename).collect()
}

fn every_key_changes(before: &[CacheKey], after: &[CacheKey]) {
    assert_eq!(before.len(), after.len());
    for (index, (before, after)) in before.iter().zip(after).enumerate() {
        assert_ne!(
            before.as_filename(),
            after.as_filename(),
            "spatial cache entry {index} must re-key"
        );
    }
}

#[test]
fn saturation_and_mean_edits_rekey_every_entry_and_restoring_both_reuses_cache() {
    let room = Room::new();
    let cache = StageCache::new(room.dir.path().join("key-cache")).unwrap();
    let original = current_keys(&room, &room.materials(1.0));
    for key in &original {
        cache.put(key, b"original bake");
    }
    let saturation_only = current_keys(&room, &room.materials(0.5));
    every_key_changes(&original, &saturation_only);

    paint(&room, "bounce/red", [0, 0, 255, 255]);
    let mean_only = current_keys(&room, &room.materials(1.0));
    let combined = current_keys(&room, &room.materials(0.5));
    every_key_changes(&original, &mean_only);
    every_key_changes(&original, &combined);
    every_key_changes(&saturation_only, &combined);
    every_key_changes(&mean_only, &combined);
    for key in &combined {
        cache.put(key, b"combined edit bake");
    }
    // Reverting saturation alone is still the blue mean, so neither stored
    // original nor combined entries may answer the request.
    for key in &mean_only {
        assert!(cache.get(key).is_none());
    }
    paint(&room, "bounce/red", [255, 0, 0, 255]);
    let texture_reverted = current_keys(&room, &room.materials(0.5));
    assert_eq!(filenames(&texture_reverted), filenames(&saturation_only));
    for key in &texture_reverted {
        assert!(
            cache.get(key).is_none(),
            "reverting texture alone still misses"
        );
    }
    let both_reverted = current_keys(&room, &room.materials(1.0));
    assert_eq!(filenames(&both_reverted), filenames(&original));
    for key in &both_reverted {
        assert_eq!(cache.get(key).as_deref(), Some(b"original bake".as_slice()));
    }
}

#[test]
fn every_bvh_texture_mean_rekeys_both_stages_but_unused_names_and_empty_faces_do_not() {
    let mut room = Room::new();
    room.pipeline.geometry.texture_names.names.extend([
        "bounce/mover_only".to_owned(),
        "bounce/fluid_only".to_owned(),
    ]);
    paint(&room, "bounce/mover_only", [64, 128, 32, 255]);
    paint(&room, "bounce/fluid_only", [32, 128, 64, 255]);
    room.assert_bvh_texture_indices();
    let used: std::collections::BTreeSet<u32> = room
        .pipeline
        .primitives
        .iter()
        .map(|primitive| primitive.material_bucket_id)
        .collect();
    let static_face_indices: std::collections::BTreeSet<u32> = room
        .pipeline
        .geometry
        .geometry
        .faces
        .iter()
        .map(|face| face.texture_index)
        .collect();
    assert_eq!(
        used, static_face_indices,
        "the folded static-face set covers every BVH material bucket"
    );
    let baseline = current_keys(&room, &room.materials(1.0));
    assert!(!used.is_empty());
    for index in used {
        let name = &room.pipeline.geometry.texture_names.names[index as usize];
        let path = room.dir.path().join("textures").join(format!("{name}.png"));
        let original_png = std::fs::read(&path).unwrap();
        paint(&room, name, [0, 0, 255, 255]);
        every_key_changes(&baseline, &current_keys(&room, &room.materials(1.0)));
        std::fs::write(path, original_png).unwrap();
        assert_eq!(
            filenames(&baseline),
            filenames(&current_keys(&room, &room.materials(1.0)))
        );
    }
    for name in ["bounce/mover_only", "bounce/fluid_only"] {
        paint(&room, name, [255, 0, 255, 255]);
        assert_eq!(
            filenames(&baseline),
            filenames(&current_keys(&room, &room.materials(1.0))),
            "unused {name} must not re-key SH"
        );
    }
    let mut no_faces = room.pipeline.geometry.clone();
    no_faces.geometry.faces.clear();
    no_faces.face_index_ranges.clear();
    let first = keys(
        &room,
        &no_faces,
        Some(&room.materials(1.0)),
        INDIRECT_DELTA_SH_STAGE_VERSION,
    );
    paint(&room, "bounce/red", [0, 255, 0, 255]);
    paint(&room, "bounce/white", [0, 0, 0, 255]);
    let edited = keys(
        &room,
        &no_faces,
        Some(&room.materials(1.0)),
        INDIRECT_DELTA_SH_STAGE_VERSION,
    );
    assert_eq!(
        filenames(&first),
        filenames(&edited),
        "empty static-face set ignores every texture edit"
    );
    let saturated = keys(
        &room,
        &no_faces,
        Some(&room.materials(0.5)),
        INDIRECT_DELTA_SH_STAGE_VERSION,
    );
    every_key_changes(&edited, &saturated);
}

#[test]
fn pre_colour_delta_epoch_misses_and_material_dependency_changes_group_keys() {
    let room = Room::new();
    let materials = room.materials(1.0);
    // These epochs are the actual pre-colour production constants captured by
    // the baseline; both current stages must have advanced beyond them.
    for (current, baseline) in [
        (SH_GROUP_STAGE_VERSION, 3),
        (INDIRECT_DELTA_SH_STAGE_VERSION, 4),
    ] {
        assert!(
            current > baseline,
            "colour-aware baker must reject pre-colour epoch {baseline}"
        );
    }
    let current = current_keys(&room, &materials);
    let without_materials = keys(
        &room,
        &room.pipeline.geometry,
        None,
        INDIRECT_DELTA_SH_STAGE_VERSION,
    );
    every_key_changes(&current, &without_materials);
    let old_delta_epoch = keys(&room, &room.pipeline.geometry, Some(&materials), 4);
    let cache = StageCache::new(room.dir.path().join("epoch-cache")).unwrap();
    // Group version is fixed by its production builder; delta exposes the
    // stage epoch as a real input, so exercise its old on-disk entry directly.
    for key in &old_delta_epoch[2..] {
        cache.put(key, b"pre-colour epoch");
    }
    for (old, new) in old_delta_epoch[2..].iter().zip(&current[2..]) {
        assert_ne!(old.as_filename(), new.as_filename());
        assert!(cache.get(new).is_none(), "old delta stage epoch must miss");
        assert_eq!(
            cache.get(old).as_deref(),
            Some(b"pre-colour epoch".as_slice())
        );
    }
}
