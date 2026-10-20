//! Burial, flush faces, siblings and the shared texture table.

use super::*;

#[test]
fn floating_fluid_block_emits_all_six_faces_wound_outward() {
    let (_, section) = compile(
        &distant_world(),
        &[fluid(&[box_brush([0, -32, 32], [64, 32, 96], "water_tex")])],
    );
    let record = &section.volumes[0];
    assert_areas(
        record,
        &[
            ("+x", 64.0 * 64.0),
            ("-x", 64.0 * 64.0),
            ("+y", 64.0 * 64.0),
            ("-y", 64.0 * 64.0),
            ("+z", 64.0 * 64.0),
            ("-z", 64.0 * 64.0),
        ],
    );
    // Texel UVs come from the side's own projection, as for world faces: a
    // Standard-projected floor/ceiling face maps Quake (x, y) to (x, -y).
    for face in record.surface_faces.iter().filter(|f| facing(f) == "+z") {
        for vertex in &face.vertices {
            let q = quake(vertex.position);
            assert!((f64::from(vertex.uv[0]) - q.x).abs() < 1e-2, "{vertex:?}");
            assert!((f64::from(vertex.uv[1]) + q.y).abs() < 1e-2, "{vertex:?}");
        }
    }
}

#[test]
fn gravity_and_push_volumes_emit_no_surface() {
    let brush = box_brush([0, -32, 32], [64, 32, 96], "volume_tex");
    let (_, section) = compile(
        &distant_world(),
        &[
            entity(
                "gravity_volume",
                "\"gravity\" \"-3\"",
                std::slice::from_ref(&brush),
            ),
            entity(
                "push_volume",
                "\"push\" \"0 0 15\"",
                std::slice::from_ref(&brush),
            ),
        ],
    );
    assert_eq!(section.volumes.len(), 2);
    assert!(section.volumes.iter().all(|v| v.surface_faces.is_empty()));
}

/// One face flush against a solid wall and one shared with a sibling
/// brush of the same entity draw nothing; every other face draws whole.
#[test]
fn fluid_faces_flush_against_solid_or_shared_with_a_sibling_emit_no_surface() {
    let wall = box_brush([-32, -128, -128], [0, 128, 256], "world_tex");
    let (_, section) = compile(
        &[wall],
        &[fluid(&[
            box_brush([0, -32, 32], [64, 32, 96], "water_tex"),
            box_brush([64, -32, 32], [128, 32, 96], "water_tex"),
        ])],
    );
    let record = &section.volumes[0];
    // Brush A's -x is on the wall; A's +x and B's -x are the shared face. The
    // union is 128 x 64 x 64 with its -x face buried.
    assert_areas(
        record,
        &[
            ("+x", 64.0 * 64.0),
            ("+y", 128.0 * 64.0),
            ("-y", 128.0 * 64.0),
            ("+z", 128.0 * 64.0),
            ("-z", 128.0 * 64.0),
        ],
    );
    assert_eq!(record.surface_faces.len(), 9, "A draws 4 faces, B draws 5");
    for face in &record.surface_faces {
        let on_shared_plane = face
            .vertices
            .iter()
            .all(|v| (quake(v.position).x - 64.0).abs() < 1e-2);
        assert!(!on_shared_plane, "no face on the sibling-shared plane");
    }
}

/// A face partly flush against solid draws only its exposed part, and no
/// piece reaches into the buried region.
#[test]
fn fluid_face_partly_flush_against_solid_emits_only_its_exposed_part() {
    // The wall covers the fluid's -x face only below z = 64.
    let wall = box_brush([-32, -128, -128], [0, 128, 64], "world_tex");
    let (_, section) = compile(
        &[wall],
        &[fluid(&[box_brush([0, -32, 32], [64, 32, 96], "water_tex")])],
    );
    let record = &section.volumes[0];
    assert_areas(
        record,
        &[
            ("+x", 64.0 * 64.0),
            ("-x", 64.0 * 32.0),
            ("+y", 64.0 * 64.0),
            ("-y", 64.0 * 64.0),
            ("+z", 64.0 * 64.0),
            ("-z", 64.0 * 64.0),
        ],
    );
    for face in record.surface_faces.iter().filter(|f| facing(f) == "-x") {
        for vertex in &face.vertices {
            let q = quake(vertex.position);
            assert!(q.z >= 64.0 - 1e-2, "buried region leaked: {q:?}");
            let expected = engine([0.0, q.y, q.z]);
            assert!(
                (DVec3::from(vertex.position.map(f64::from)) - expected).length() < EPSILON,
                "exposed piece stays on the face plane"
            );
        }
    }
}

/// A pool in a pit carved into the floor draws its top; its bottom and sides
/// rest flush on solid.
#[test]
fn pool_sunk_into_a_floor_emits_its_top_but_not_its_buried_bottom_or_sides() {
    let world = vec![
        box_brush([-64, -64, -64], [192, 192, -48], "world_tex"),
        box_brush([-64, -64, -48], [0, 192, 0], "world_tex"),
        box_brush([128, -64, -48], [192, 192, 0], "world_tex"),
        box_brush([0, -64, -48], [128, 0, 0], "world_tex"),
        box_brush([0, 128, -48], [128, 192, 0], "world_tex"),
    ];
    let (_, section) = compile(
        &world,
        &[fluid(&[box_brush(
            [0, 0, -48],
            [128, 128, -8],
            "water_tex",
        )])],
    );
    assert_areas(&section.volumes[0], &[("+z", 128.0 * 128.0)]);
}

/// A fluid block sunk into an uncarved slab keeps only what stands above it:
/// the whole top, each side clipped at the slab's top, and no bottom.
#[test]
fn fluid_block_sunk_into_solid_keeps_only_its_parts_above_the_solid() {
    let slab = box_brush([-128, -128, -64], [192, 192, 0], "world_tex");
    let (_, section) = compile(
        &[slab],
        &[fluid(&[box_brush([0, 0, -32], [64, 64, 32], "water_tex")])],
    );
    let record = &section.volumes[0];
    assert_areas(
        record,
        &[
            ("+x", 64.0 * 32.0),
            ("-x", 64.0 * 32.0),
            ("+y", 64.0 * 32.0),
            ("-y", 64.0 * 32.0),
            ("+z", 64.0 * 64.0),
        ],
    );
    for face in &record.surface_faces {
        for vertex in &face.vertices {
            assert!(
                quake(vertex.position).z >= -1e-2,
                "no piece inside the slab"
            );
        }
    }
}

/// Overlapping sibling brushes share a top plane: the overlap draws once, with
/// no hole and no double layer, and no face lies inside the union.
#[test]
fn overlapping_sibling_brushes_draw_their_union_boundary_once() {
    let (_, section) = compile(
        &distant_world(),
        &[fluid(&[
            box_brush([0, 0, 0], [96, 64, 64], "water_tex"),
            box_brush([64, 0, 0], [160, 64, 64], "water_tex"),
        ])],
    );
    assert_areas(
        &section.volumes[0],
        &[
            ("+x", 64.0 * 64.0),
            ("-x", 64.0 * 64.0),
            ("+y", 160.0 * 64.0),
            ("-y", 160.0 * 64.0),
            ("+z", 160.0 * 64.0),
            ("-z", 160.0 * 64.0),
        ],
    );
}

/// Known limit: two fluid entities do not cull each other, so each draws the
/// face they share.
#[test]
fn adjacent_fluid_entities_each_draw_their_shared_face() {
    let (_, section) = compile(
        &distant_world(),
        &[
            fluid(&[box_brush([0, 0, 0], [64, 64, 64], "water_tex")]),
            fluid(&[box_brush([64, 0, 0], [128, 64, 64], "water_tex")]),
        ],
    );
    assert_eq!(section.volumes[0].surface_faces.len(), 6);
    assert_eq!(section.volumes[1].surface_faces.len(), 6);
}

/// The shared level texture table: a texture the world already uses keeps its
/// index, and a fluid-only texture appends after every world entry.
#[test]
fn fluid_textures_reuse_world_indices_and_append_new_ones_after_the_world() {
    let (map, _) = compile(
        &distant_world(),
        &[
            fluid(&[box_brush([0, 0, 0], [64, 64, 64], "world_tex")]),
            fluid(&[box_brush([200, 0, 0], [264, 64, 64], "water_tex")]),
        ],
    );
    let mut texture_names = TextureNamesSection {
        names: vec!["mover_tex".to_string(), "world_tex".to_string()],
    };
    let section = encode_environment_volumes_section(
        &map.environment_volumes,
        -9.81,
        &map.brush_volumes,
        &mut texture_names,
    );
    assert_eq!(texture_names.names, ["mover_tex", "world_tex", "water_tex"]);
    assert!(
        section.volumes[0]
            .surface_faces
            .iter()
            .all(|face| face.texture_index == 1)
    );
    assert!(
        section.volumes[1]
            .surface_faces
            .iter()
            .all(|face| face.texture_index == 2)
    );
}
