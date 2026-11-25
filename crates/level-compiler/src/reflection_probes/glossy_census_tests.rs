// Glossy material verdicts, world reflector faces and the census's plane and edge grouping.
// See: context/lib/resource_management.md §5.4 · testing_guide.md §3

use std::collections::HashMap;
use std::path::Path;

use glam::{DVec3, Vec3};
use postretro_level_format::prm::{
    PrmFile, PrmFormat, PrmHeader, PrmSlot, PrmSlots, STAGE_VERSION, cache_filename_for_key,
    expected_level_count,
};

use crate::bake_control::BakeControl;
use crate::geometry::GeometryResult;

use super::glossy_census::{coplanar_planes, edge_connected_groups, take_census};
use super::glossy_materials::GlossyMaterials;
use super::placement_test_fixtures::{PlacementFixture, entity, sealed_room, textured_box_brush};
use super::reflection_probes_test_fixtures::{GeometryBuilder, rect};
use super::world_reflectors::world_reflector_faces;

/// A 2×2 specular slot whose mip 0 is `texel` everywhere.
fn specular_slot(format: PrmFormat, texel: &[u8]) -> PrmSlot {
    let level0: Vec<u8> = texel.repeat(4);
    PrmSlot {
        format,
        width: 2,
        height: 2,
        level_count: expected_level_count(2, 2),
        payload: [level0, texel.to_vec()].concat(),
    }
}

fn write_specular(dir: &Path, key: [u8; 32], slot: PrmSlot) {
    write_specular_layers(dir, key, slot, 1);
}

/// A sidecar of `layer_count` layers; `slot`'s payload holds every layer.
fn write_specular_layers(dir: &Path, key: [u8; 32], slot: PrmSlot, layer_count: u16) {
    let prm = PrmFile {
        header: PrmHeader {
            stage_version: STAGE_VERSION,
            slot_mask: PrmSlots::SPECULAR,
            bundle_hash: [0; 32],
            total_body_bytes: 0,
            layer_count,
        },
        slots: [None, Some(slot), None, None],
    };
    std::fs::write(
        dir.join(format!("{}.prm", cache_filename_for_key(&key))),
        prm.to_bytes().unwrap(),
    )
    .unwrap();
}

/// Floor quads at `y`, one per `(x0, z0, x1, z1)` span, each with its texture
/// index.
fn floor_quads(quads: &[([f32; 4], u32)], y: f32) -> GeometryBuilder {
    let mut builder = GeometryBuilder::default();
    for &([x0, z0, x1, z1], texture) in quads {
        let corners = rect(Vec3::new(x0, y, z0), Vec3::new(x1, y, z1), 1, y);
        builder.push_face(Vec3::Y, &corners, texture, Vec3::ZERO, 0, [[0.0; 2]; 4]);
    }
    builder
}

#[test]
fn glossy_materials_read_reflects_only_a_four_channel_slot_past_both_conditions() {
    let dir = tempfile::tempdir().unwrap();
    write_specular(
        dir.path(),
        [1; 32],
        specular_slot(PrmFormat::Rgba8Unorm, &[200, 127, 200, 255]),
    );
    // No `_p`: B = 0.
    write_specular(
        dir.path(),
        [2; 32],
        specular_slot(PrmFormat::Rgba8Unorm, &[200, 127, 0, 255]),
    );
    // A one-channel slot, however bright.
    write_specular(
        dir.path(),
        [3; 32],
        specular_slot(PrmFormat::R8Unorm, &[255]),
    );
    // Glossy, but no face uses it.
    write_specular(
        dir.path(),
        [5; 32],
        specular_slot(PrmFormat::Rgba8Unorm, &[200, 127, 200, 255]),
    );
    let names: Vec<String> = ["marble", "rough", "plain", "untextured", "unused"]
        .map(String::from)
        .to_vec();
    let keys = HashMap::from([
        (names[0].clone(), [1u8; 32]),
        (names[1].clone(), [2u8; 32]),
        (names[2].clone(), [3u8; 32]),
        (names[3].clone(), [0u8; 32]),
        (names[4].clone(), [5u8; 32]),
    ]);
    let geometry = floor_quads(
        &[
            ([0.0, 0.0, 1.0, 1.0], 0),
            ([1.0, 0.0, 2.0, 1.0], 1),
            ([2.0, 0.0, 3.0, 1.0], 2),
            ([3.0, 0.0, 4.0, 1.0], 3),
        ],
        0.0,
    )
    .finish(names);
    let control = BakeControl::unrestricted();
    let materials = GlossyMaterials::read(&geometry, &keys, dir.path(), control.governor());
    assert_eq!(
        (0..5).map(|i| materials.reflects(i)).collect::<Vec<_>>(),
        vec![true, false, false, false, false]
    );
    assert_eq!(
        (materials.used, materials.glossy(), materials.unreadable),
        (4, 1, 0)
    );
}

// The loader binds a layered world sidecar a placeholder, so its material
// draws matte at runtime; the compiler must not place probes for it either,
// whatever its layers hold.
#[test]
fn glossy_materials_read_never_reflects_a_layered_sidecar() {
    let dir = tempfile::tempdir().unwrap();
    let texel = [200, 127, 200, 255];
    write_specular(
        dir.path(),
        [1; 32],
        specular_slot(PrmFormat::Rgba8Unorm, &texel),
    );
    let mut layered = specular_slot(PrmFormat::Rgba8Unorm, &texel);
    layered.payload = layered.payload.repeat(2);
    write_specular_layers(dir.path(), [2; 32], layered, 2);
    let names: Vec<String> = ["single", "layered"].map(String::from).to_vec();
    let keys = HashMap::from([(names[0].clone(), [1u8; 32]), (names[1].clone(), [2u8; 32])]);
    let geometry =
        floor_quads(&[([0.0, 0.0, 1.0, 1.0], 0), ([1.0, 0.0, 2.0, 1.0], 1)], 0.0).finish(names);
    let control = BakeControl::unrestricted();
    let materials = GlossyMaterials::read(&geometry, &keys, dir.path(), control.governor());
    assert_eq!(
        (materials.reflects(0), materials.reflects(1)),
        (true, false)
    );
    assert_eq!(
        (materials.used, materials.glossy(), materials.unreadable),
        (2, 1, 0)
    );
}

#[test]
fn glossy_materials_read_counts_a_missing_sidecar_unreadable_and_not_glossy() {
    let dir = tempfile::tempdir().unwrap();
    let names = vec!["marble".to_string()];
    let keys = HashMap::from([(names[0].clone(), [9u8; 32])]);
    let geometry = floor_quads(&[([0.0, 0.0, 1.0, 1.0], 0)], 0.0).finish(names);
    let control = BakeControl::unrestricted();
    let materials = GlossyMaterials::read(&geometry, &keys, dir.path(), control.governor());
    assert!(!materials.reflects(0));
    assert_eq!((materials.used, materials.unreadable), (1, 1));
}

#[test]
fn world_reflector_faces_keep_glossy_faces_with_their_plane_and_outline() {
    let mut builder = floor_quads(&[([0.0, 0.0, 2.0, 1.0], 1), ([2.0, 0.0, 3.0, 1.0], 0)], 0.5);
    // A wall facing +X at x = 4.
    let wall = rect(Vec3::new(4.0, 0.0, 0.0), Vec3::new(4.0, 3.0, 2.0), 0, 4.0);
    builder.push_face(Vec3::X, &wall, 1, Vec3::ZERO, 0, [[0.0; 2]; 4]);
    let geometry = builder.finish(["matte", "glossy"].map(String::from).to_vec());
    let reflectors = world_reflector_faces(
        &geometry,
        &GlossyMaterials::from_verdicts(vec![false, true]),
    );
    assert_eq!(reflectors.unrecovered, 0);
    let faces = &reflectors.faces;
    assert_eq!(faces.iter().map(|f| f.face).collect::<Vec<_>>(), vec![0, 2]);

    let floor = &faces[0];
    assert!((floor.geometry.normal - DVec3::Y).length() < 1e-9);
    assert!((floor.plane_offset - 0.5).abs() < 1e-9);
    assert!((floor.geometry.area - 2.0).abs() < 1e-9);
    assert!((floor.geometry.centroid - DVec3::new(1.0, 0.5, 0.5)).length() < 1e-9);

    let wall_face = &faces[1];
    assert!((wall_face.geometry.normal - DVec3::X).length() < 1e-9);
    assert!((wall_face.plane_offset - 4.0).abs() < 1e-9);
    assert!((wall_face.geometry.area - 6.0).abs() < 1e-9);

    // The outline is the polygon in its own plane's frame, in winding order:
    // lifted back, it is the face's corners.
    for face in faces {
        let start = geometry.face_index_ranges[face.face as usize].index_offset as usize;
        let corners: Vec<DVec3> = [0, 1, 2, 5]
            .map(|i| geometry.geometry.indices[start + i])
            .iter()
            .map(|&v| {
                DVec3::from(
                    geometry.geometry.vertices[v as usize]
                        .position
                        .map(f64::from),
                )
            })
            .collect();
        let lifted: Vec<DVec3> = face.outline_points().collect();
        assert_eq!(lifted.len(), 4);
        for (a, b) in lifted.iter().zip(&corners) {
            assert!((*a - *b).length() < 1e-6, "{a} vs {b}");
        }
    }
}

#[test]
fn census_groups_coplanar_faces_by_shared_edges_including_t_junctions() {
    // Plane y = 0: a big quad with two half-length neighbours along its east
    // edge (a T-junction), a separate quad across a gap, and one touching the
    // big quad only at a corner. Plane y = 1: one quad. A down-facing quad at
    // y = 0, under the big one's footprint, is another plane.
    let mut builder = floor_quads(
        &[
            ([0.0, 0.0, 2.0, 2.0], 0),
            ([2.0, 0.0, 3.0, 1.0], 0),
            ([2.0, 1.0, 3.0, 2.0], 0),
            ([5.0, 0.0, 6.0, 1.0], 0),
            ([-1.0, -1.0, 0.0, 0.0], 0),
        ],
        0.0,
    );
    let raised = rect(Vec3::new(0.0, 1.0, 0.0), Vec3::new(1.0, 1.0, 1.0), 1, 1.0);
    builder.push_face(Vec3::Y, &raised, 0, Vec3::ZERO, 0, [[0.0; 2]; 4]);
    let under = rect(Vec3::new(0.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 2.0), 1, 0.0);
    builder.push_face(Vec3::NEG_Y, &under, 0, Vec3::ZERO, 0, [[0.0; 2]; 4]);
    let geometry = builder.finish(vec!["glossy".to_string()]);
    let faces = world_reflector_faces(&geometry, &GlossyMaterials::from_verdicts(vec![true])).faces;

    let planes = coplanar_planes(&faces);
    assert_eq!(planes, vec![vec![0, 1, 2, 3, 4], vec![5], vec![6]]);
    assert_eq!(
        edge_connected_groups(&faces, &planes[0]),
        vec![vec![0, 1, 2], vec![3], vec![4]],
        "T-junction joined; gap and corner apart"
    );
    assert_eq!(edge_connected_groups(&faces, &planes[1]), vec![vec![5]]);
    assert_eq!(edge_connected_groups(&faces, &planes[2]), vec![vec![6]]);
}

/// A unit floor quad spanning x0..x0 + 1, z 0..1, tilted `degrees` about its
/// own centre line (z axis) so its area centroid sits at height `y`.
fn tilted_quad(builder: &mut GeometryBuilder, x0: f32, y: f32, degrees: f32) {
    let half_rise = 0.5 * degrees.to_radians().tan();
    let corners = [
        Vec3::new(x0, y - half_rise, 0.0),
        Vec3::new(x0 + 1.0, y + half_rise, 0.0),
        Vec3::new(x0 + 1.0, y + half_rise, 1.0),
        Vec3::new(x0, y - half_rise, 1.0),
    ];
    let normal = (corners[3] - corners[0])
        .cross(corners[1] - corners[0])
        .normalize();
    builder.push_face(normal, &corners, 0, Vec3::ZERO, 0, [[0.0; 2]; 4]);
}

#[test]
fn census_counts_planes_within_a_millimetre_and_a_degree_as_one() {
    // A flat quad; beside it, sharing its edge, one 0.5 mm higher and tilted
    // 0.05°: one plane, one group. One 2 mm higher, and one tilted 2°, are
    // planes of their own.
    let mut builder = floor_quads(&[([0.0, 0.0, 1.0, 1.0], 0)], 0.0);
    tilted_quad(&mut builder, 1.0, 0.0005, 0.05);
    tilted_quad(&mut builder, 3.0, 0.002, 0.0);
    tilted_quad(&mut builder, 5.0, 0.0, 2.0);
    let geometry = builder.finish(vec!["glossy".to_string()]);
    let faces = world_reflector_faces(&geometry, &GlossyMaterials::from_verdicts(vec![true])).faces;
    let planes = coplanar_planes(&faces);
    assert_eq!(planes, vec![vec![0, 1], vec![2], vec![3]]);
    assert_eq!(edge_connected_groups(&faces, &planes[0]), vec![vec![0, 1]]);
}

#[test]
fn census_of_a_level_without_glossy_materials_reads_no_face() {
    let dir = tempfile::tempdir().unwrap();
    let geometry: GeometryResult =
        floor_quads(&[([0.0, 0.0, 1.0, 1.0], 0)], 0.0).finish(vec!["matte".to_string()]);
    let control = BakeControl::unrestricted();
    let materials =
        GlossyMaterials::read(&geometry, &HashMap::new(), dir.path(), control.governor());
    let census = take_census(&geometry, &materials);
    assert_eq!(
        (
            census.used_materials,
            census.glossy_materials,
            census.faces,
            census.groups
        ),
        (1, 0, 0, 0)
    );
}

/// On a compiled map, reflector faces front their stored vertex normals, so
/// the winding normal is the face's front, and fluid surfaces never reach
/// id 17: every glossy verdict on the fluid's texture finds no face.
#[test]
fn compiled_world_reflectors_front_their_vertex_normals_and_exclude_fluid_surfaces() {
    let mut world = sealed_room([0.0, 0.0, 0.0], [256.0, 256.0, 128.0]);
    world.push(textured_box_brush(
        [64.0, 64.0, 0.0],
        [128.0, 128.0, 48.0],
        "test/glossy",
    ));
    let water = entity(
        "fluid_volume",
        "\"fluid\" \"water\"",
        &[textured_box_brush(
            [160.0, 64.0, 0.0],
            [224.0, 192.0, 32.0],
            "test/water",
        )],
    );
    let fixture = PlacementFixture::compile(&world, &[water]);
    assert!(
        !fixture.up_faces().is_empty(),
        "the pool has an id-52 surface"
    );
    let names = &fixture.geometry.texture_names.names;
    let verdicts: Vec<bool> = names
        .iter()
        .map(|name| name == "test/glossy" || name == "test/water")
        .collect();
    let reflectors =
        world_reflector_faces(&fixture.geometry, &GlossyMaterials::from_verdicts(verdicts));
    assert_eq!(reflectors.unrecovered, 0);
    assert!(!reflectors.faces.is_empty());
    let section = &fixture.geometry.geometry;
    for face in &reflectors.faces {
        let meta = &section.faces[face.face as usize];
        assert_eq!(
            names[meta.texture_index as usize], "test/glossy",
            "face {}",
            face.face
        );
        let range = fixture.geometry.face_index_ranges[face.face as usize];
        let first = section.indices[range.index_offset as usize] as usize;
        let stored = DVec3::from(section.vertices[first].decode_normal().map(f64::from));
        assert!(
            face.geometry.normal.dot(stored) > 0.999,
            "face {}: winding {} against stored {stored}",
            face.face,
            face.geometry.normal
        );
    }
}
