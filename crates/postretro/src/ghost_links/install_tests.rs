// Ghost links compiled by the real `prl-build`, loaded from the PRL, then digested
// and installed: the section stays off the level digest, and each link installs
// one engine entity whose companion is never a placement.
// See: context/lib/entity_model.md §7d · context/lib/networking.md §Level identity and level content digest

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use postretro_entities::components::map_ordinal::MapOrdinal;
use postretro_entities::provenance::DescriptorProvenance;
use postretro_entities::{ComponentKind, ComponentValue, GhostLinkComponent, Transform};
use postretro_level_format::ghost_links::GhostMembers;
use postretro_level_format::map_entity::MapEntityRecord;
use postretro_level_loader::LevelWorld;

use super::spawn_loaded_ghost_links;

const TEXTURE: &str = "50-free-textures/concrete_stone_021";

fn box_brush(min: [i32; 3], max: [i32; 3]) -> String {
    let ([x0, y0, z0], [x1, y1, z1]) = (min, max);
    format!(
        "{{\n\
         ( {x0} 0 0 ) ( {x0} 1 0 ) ( {x0} 0 1 ) {TEXTURE} 0 0 0 1 1\n\
         ( {x1} 0 0 ) ( {x1} 0 1 ) ( {x1} 1 0 ) {TEXTURE} 0 0 0 1 1\n\
         ( 0 {y0} 0 ) ( 0 {y0} 1 ) ( 1 {y0} 0 ) {TEXTURE} 0 0 0 1 1\n\
         ( 0 {y1} 0 ) ( 1 {y1} 0 ) ( 0 {y1} 1 ) {TEXTURE} 0 0 0 1 1\n\
         ( 0 0 {z0} ) ( 1 0 {z0} ) ( 0 1 {z0} ) {TEXTURE} 0 0 0 1 1\n\
         ( 0 0 {z1} ) ( 0 1 {z1} ) ( 1 0 {z1} ) {TEXTURE} 0 0 0 1 1\n\
         }}"
    )
}

/// One sealed room: worldspawn (0), player spawn (1), two destinations (2, 3),
/// then `last`, so an appended region shifts no other source index.
fn room_map(last: Option<String>) -> String {
    let brushes = [
        box_brush([-272, -144, -16], [272, 144, 0]),
        box_brush([-272, -144, 128], [272, 144, 144]),
        box_brush([-272, -144, 0], [-256, 144, 128]),
        box_brush([256, -144, 0], [272, 144, 128]),
        box_brush([-256, -144, 0], [256, -128, 128]),
        box_brush([-256, 128, 0], [256, 144, 128]),
    ];
    let mut entities = vec![
        format!(
            "{{\n\"classname\" \"worldspawn\"\n{}\n}}",
            brushes.join("\n")
        ),
        "{\n\"classname\" \"player_spawn\"\n\"origin\" \"0 0 32\"\n}".to_string(),
        "{\n\"classname\" \"teleport_destination\"\n\"name\" \"copy_a\"\n\"origin\" \"-128 0 32\"\n\"angles\" \"0 0 0\"\n}".to_string(),
        "{\n\"classname\" \"teleport_destination\"\n\"name\" \"copy_b\"\n\"origin\" \"128 0 32\"\n\"angles\" \"0 90 0\"\n}".to_string(),
    ];
    entities.extend(last);
    entities.join("\n")
}

fn region(classname: &str, keys: &str) -> String {
    format!(
        "{{\n\"classname\" \"{classname}\"\n{keys}\n{}\n}}",
        box_brush([-200, -64, 0], [-168, 64, 64])
    )
}

/// The variants: no region, a ghost link last (disarmed, or armed on spawn), a
/// teleporter last.
fn variant_map(variant: &str) -> String {
    match variant {
        "base" => room_map(None),
        "link" => room_map(Some(region(
            "ghost_link",
            "\"landmark\" \"copy_a\"\n\"target\" \"copy_b\"\n\"enabled_on_spawn\" \"0\"\n\
             \"ghost_members\" \"players movers\"\n\"ghost_tags\" \"barrier\"\n\
             \"ghost_style\" \"echo\"\n\"_tags\" \"vault_echo\"",
        ))),
        "armed_link" => room_map(Some(region(
            "ghost_link",
            "\"landmark\" \"copy_a\"\n\"target\" \"copy_b\"\n\"ghost_members\" \"players\"",
        ))),
        "teleporter" => room_map(Some(region(
            "trigger_teleport",
            "\"mode\" \"seamless\"\n\"landmark\" \"copy_a\"\n\"target\" \"copy_b\"",
        ))),
        other => panic!("unknown variant {other}"),
    }
}

/// Compile a variant once per test process, then load a private copy of it.
fn compiled(variant: &'static str, out: &std::path::Path) -> LevelWorld {
    static COMPILED: OnceLock<Mutex<HashMap<&'static str, Vec<u8>>>> = OnceLock::new();
    let bytes = COMPILED
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entry(variant)
        .or_insert_with(|| {
            let source_dir = tempfile::tempdir().unwrap();
            let maps = source_dir.path().join("maps");
            std::fs::create_dir_all(&maps).unwrap();
            let source = maps.join(format!("ghost_{variant}.map"));
            std::fs::write(&source, format!("{}\n", variant_map(variant))).unwrap();
            crate::startup::teleporters_surface_tests::run_prl_build_on(
                &source,
                &["--sh-probe-spacing", "4", "--lightmap-density", "0.25"],
            )
        })
        .clone();
    let prl = out.join(format!("ghost_{variant}.prl"));
    std::fs::write(&prl, bytes).unwrap();
    postretro_level_loader::load_prl(prl.to_str().unwrap()).expect("compiled PRL loads")
}

fn digest(world: &LevelWorld) -> [u8; 32] {
    crate::runtime_movers::level_content_digest(&world.kinematic_geometry, world)
}

#[test]
fn ghost_link_last_entity_leaves_level_digest_unchanged_while_a_teleporter_changes_it() {
    let out = tempfile::tempdir().unwrap();
    let base = compiled("base", out.path());
    let link = compiled("link", out.path());
    let teleporter = compiled("teleporter", out.path());
    assert_eq!(link.ghost_links.links.len(), 1, "the link reached id 56");
    assert!(base.ghost_links.links.is_empty() && teleporter.ghost_links.links.is_empty());
    assert_eq!(teleporter.teleporters.teleporters.len(), 1);
    assert_eq!(
        digest(&link),
        digest(&base),
        "a ghost link is presentation, not a prediction input"
    );
    assert_ne!(
        digest(&teleporter),
        digest(&base),
        "a teleporter is a prediction input"
    );
}

#[test]
fn ghost_link_install_spawns_its_entity_and_never_dispatches_its_companion() {
    let out = tempfile::tempdir().unwrap();
    let world = compiled("link", out.path());
    let (ordinal, companion) = world
        .map_entities
        .iter()
        .enumerate()
        .find(|(_, entity)| entity.classname == "ghost_link")
        .expect("the compiler writes a companion");
    let companion = companion.clone();

    let mut registry = postretro_entities::EntityRegistry::new();
    spawn_loaded_ghost_links(&mut registry, &world);
    let links: Vec<_> = registry.iter_with_kind(ComponentKind::GhostLink).collect();
    assert_eq!(links.len(), 1);
    let (entity, ComponentValue::GhostLink(link)) = links[0] else {
        panic!("ghost-link column holds ghost-link state");
    };
    assert_eq!(
        *link,
        GhostLinkComponent {
            index: 0,
            source_entity: 4,
            armed: false,
            members: (GhostMembers::PLAYERS | GhostMembers::MOVERS).bits(),
            filter: Default::default(),
        }
    );
    assert_eq!(
        registry.get_tags(entity).unwrap(),
        &["vault_echo".to_owned()]
    );
    assert_eq!(
        registry.get_component::<MapOrdinal>(entity).unwrap().0 as usize,
        ordinal
    );
    let position = registry
        .get_component::<Transform>(entity)
        .unwrap()
        .position;
    assert!((position - glam::Vec3::from(companion.origin)).length() < 1e-5);

    // Through the real install seam, with a descriptor sharing the companion's
    // classname. A control record with its own descriptor shows the sweep does
    // spawn placements in this harness.
    let mut world = world;
    world.map_entities.push(MapEntityRecord {
        classname: "ghost_probe".to_owned(),
        origin: [0.0, 0.5, 0.0],
        ..Default::default()
    });
    let descriptors = ["ghost_link", "ghost_probe"].map(|name| {
        let mut descriptor =
            crate::scripting::builtins::data_archetype_test_fixtures::mesh_descriptor(name, false);
        descriptor.mesh = None;
        descriptor.light = Some(postretro_entities::LightDescriptor {
            color: [1.0; 3],
            intensity: 1.0,
            range: 8.0,
            is_dynamic: true,
        });
        descriptor
    });
    let installed = crate::startup::lifecycle::tests::install_descriptor_world(
        world,
        crate::trigger_pools::TriggerPoolSeedPolicy::ArmAll,
        &[],
        Vec::new(),
        false,
        descriptors.to_vec(),
    );
    let placements = |name: &str| {
        installed
            .registry
            .iter_with_kind(ComponentKind::DescriptorProvenance)
            .filter(|(_, value)| {
                matches!(value, ComponentValue::DescriptorProvenance(DescriptorProvenance {
                    canonical_name, ..
                }) if canonical_name == name)
            })
            .count()
    };
    assert_eq!(placements("ghost_probe"), 1, "the control placement spawns");
    assert_eq!(
        placements("ghost_link"),
        0,
        "the companion is never a placement"
    );
    assert_eq!(
        installed
            .registry
            .iter_with_kind(ComponentKind::GhostLink)
            .count(),
        1,
        "install spawns the link entity once"
    );
}

// A link armed on spawn, through the real compile, load and install: a partner
// pawn inside it ghosts at full membership on the first evaluated frame,
// at the pose the authored landmark → target mapping gives.
#[test]
fn ghost_link_armed_on_spawn_presents_its_member_in_full_on_the_first_frame() {
    use postretro_entities::provenance::DescriptorSpawnPath;

    let out = tempfile::tempdir().unwrap();
    let world = compiled("armed_link", out.path());
    let record = &world.ghost_links.links[0];
    assert!(record.enabled_on_spawn);
    let inside = (glam::Vec3::from(record.brushes[0].aabb_min)
        + glam::Vec3::from(record.brushes[0].aabb_max))
        * 0.5;

    let mut registry = postretro_entities::EntityRegistry::new();
    spawn_loaded_ghost_links(&mut registry, &world);
    let pawn = |registry: &mut postretro_entities::EntityRegistry, at, spawn_path| {
        let id = registry.spawn(Transform {
            position: at,
            ..Default::default()
        });
        registry
            .set_component(
                id,
                postretro_foundation::PlayerMovementComponent::from_descriptor(
                    &crate::mod_digest::tests::movement_descriptor(),
                ),
            )
            .unwrap();
        registry
            .set_component(
                id,
                DescriptorProvenance {
                    canonical_name: "player".into(),
                    owned_components: Default::default(),
                    map_overrides: Default::default(),
                    spawn_path,
                },
            )
            .unwrap();
        id
    };
    let viewer = pawn(
        &mut registry,
        glam::Vec3::splat(-1000.0),
        DescriptorSpawnPath::PlayerSpawn,
    );
    registry.mark_local_player_pawn(viewer).unwrap();
    let partner = pawn(&mut registry, inside, DescriptorSpawnPath::NetworkSlot);

    let mut ghosts = crate::ghost_links::GhostLinks::install(&world, &registry);
    ghosts.refresh_opt_ins(&crate::ghost_links::opt_ins_test_fixtures::player_opted_in());
    ghosts.update(
        &registry,
        crate::ghost_links::GhostFrame {
            clocks: crate::ghost_links::PresentationClocks::compute(
                crate::ghost_links::clocks::ClockSource::Host { tick: 1 },
                0.0,
            ),
            alpha: 0.0,
            elapsed_seconds: 1.0 / 60.0,
            view: crate::ghost_links::CandidateView::Authoritative,
            stamps: crate::ghost_links::clocks::PoseStamps::None,
        },
    );
    let [ghost] = ghosts.members() else {
        panic!("one ghost: the partner, never the viewer");
    };
    assert_eq!(ghost.entity, partner);
    assert_eq!(ghost.membership, 1.0);
    let expected = postretro_foundation::SeamlessMapping::new(
        record.landmark_origin.into(),
        record.landmark_yaw,
        record.target_origin.into(),
        record.target_yaw,
    )
    .point(inside);
    assert!((ghost.mapped.position - expected).length() < 1e-4);
}
