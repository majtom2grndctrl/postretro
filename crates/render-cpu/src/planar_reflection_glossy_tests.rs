// Planar candidates and selection beside glossy world faces: id 54's world
// bindings never form, widen or score a plane.
// See: context/lib/rendering_pipeline.md §7.3a (Planar layer, Glossy world faces)

use super::*;
use postretro_level_format::reflection_probes::ReflectionProbeWorldBinding;

/// `section` with `faces` world faces bound to its probe: a glossy floor's
/// faces, however many and however large.
fn with_glossy_faces(mut section: ReflectionProbesSection, faces: u32) -> ReflectionProbesSection {
    section.world_bindings = (0..faces)
        .map(|face| ReflectionProbeWorldBinding { face, probe: 0 })
        .collect();
    section
}

// Acceptance row 58 (candidate builder): the candidate builder reads fluid
// volumes and their fluid bindings alone, so a level whose glossy floor binds
// thousands of world faces, outsizing every pool, builds the same candidate
// planes as without them, and selects the same plane, with the same
// hysteresis, on every frame of a camera path.
#[test]
fn glossy_world_bindings_leave_candidates_and_selection_unchanged() {
    let volumes = [
        water(0.0, 0.0),
        water_rect(-2.0, Vec2::new(-30.0, -40.0), Vec2::new(30.0, -10.0)),
        water(1.0, 6.0),
    ];
    let all: Vec<u32> = (0..volumes.len() as u32).collect();
    let water_only = bind_every_face(&volumes);
    let glossy = with_glossy_faces(water_only.clone(), 4096);
    let before = PlanarCandidates::from_volumes(&volumes, &all, Some(&water_only));
    let after = PlanarCandidates::from_volumes(&volumes, &all, Some(&glossy));
    assert!(before.planes.len() >= 2, "the fixture forms several planes");
    assert_eq!(after.planes, before.planes);

    // A section binding world faces and no fluid face forms no plane.
    let world_only = with_glossy_faces(probe_section(), 4096);
    assert!(
        PlanarCandidates::from_volumes(&volumes, &all, Some(&world_only))
            .planes
            .is_empty()
    );

    let path = [
        Vec3::new(0.0, 3.0, 2.0),
        Vec3::new(-4.0, 2.5, 0.0),
        Vec3::new(5.0, 4.0, -2.0),
        Vec3::new(0.0, 0.5, -3.0),
        Vec3::new(0.0, 6.0, 4.0),
    ];
    let (mut held_before, mut held_after) = (None, None);
    let mut selected = 0;
    for eye in path {
        let vp = projection() * view(eye);
        let first = select_plane(&before, eye, vp, &SEEDED, held_before);
        let second = select_plane(&after, eye, vp, &SEEDED, held_after);
        assert_eq!(second, first, "the same plane from {eye}");
        selected += usize::from(first.is_some());
        (held_before, held_after) = (first, second);
    }
    assert!(selected >= 3, "the path selects planes: {selected}");
}
