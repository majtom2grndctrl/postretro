// Billboard ghost collection: mapped-cell culling, the packed look and pose, fading
// after despawn, and which bodies are billboards at all.
// See: context/lib/rendering_pipeline.md §7.9, §7.4 · context/lib/entity_model.md §7d

use std::collections::BTreeSet;

use glam::{Quat, Vec3};
use postretro_entities::components::mesh::MeshComponent;
use postretro_entities::components::sprite_visual::SpriteVisual;
use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
use postretro_entities::{EntityId, EntityRegistry, Transform};
use postretro_foundation::SeamlessMapping;
use postretro_level_loader::{CellData, CellLocatorChild, CellLocatorNodeData, LevelWorld};
use postretro_render_cpu::ghost_instance::{GHOST_SPRITE_INSTANCE_SIZE, GhostInstanceLook};
use postretro_visibility::VisibleCells;

use super::*;
use crate::ghost_links::{GhostCategory, GhostLook};

const REAL_CELL: u32 = 0;
const MAPPED_CELL: u32 = 1;
const SOURCE: Vec3 = Vec3::new(12.0, 1.0, 2.0);
const COLLECTION: &str = "rocket_trail";
const STYLE: GhostLook = GhostLook {
    tint: [0.6, 0.8, 1.0],
    opacity: 0.5,
    saturation: 0.25,
};

fn cell(min: Vec3, max: Vec3) -> CellData {
    CellData {
        bounds_min: min,
        bounds_max: max,
        face_start: 0,
        face_count: 0,
        portal_ref_start: 0,
        portal_ref_count: 0,
        is_solid: false,
        is_exterior: false,
        is_drawable: false,
    }
}

/// Cell 0 holds the real copy (x >= 0), cell 1 the other copy (x < 0).
fn two_copy_world() -> LevelWorld {
    LevelWorld::new_visibility_only(
        vec![
            cell(Vec3::new(0.0, -100.0, -100.0), Vec3::splat(100.0)),
            cell(Vec3::splat(-100.0), Vec3::new(0.0, 100.0, 100.0)),
        ],
        Vec::new(),
        CellLocatorChild::Node(0),
        vec![CellLocatorNodeData {
            plane_normal: Vec3::X,
            plane_distance: 0.0,
            front: CellLocatorChild::Cell(REAL_CELL as usize),
            back: CellLocatorChild::Cell(MAPPED_CELL as usize),
        }],
        Vec::new(),
        false,
    )
    .unwrap()
}

fn mapping() -> SeamlessMapping {
    SeamlessMapping::new(
        Vec3::new(10.0, 0.0, 0.0),
        0.0,
        Vec3::new(-20.0, 0.0, 0.0),
        std::f32::consts::FRAC_PI_2,
    )
}

fn sprite() -> SpriteVisual {
    SpriteVisual {
        collection: COLLECTION.into(),
        size: 0.75,
        opacity: 0.8,
        rotation: 0.3,
        tint: [1.0; 3],
    }
}

/// A projectile display copy with a sprite body: presentation provenance and
/// no gameplay state, as a client's remote shot or the host's remote-fire flight.
fn spawn_projectile_copy(registry: &mut EntityRegistry, at: Vec3) -> EntityId {
    let id = registry.spawn(Transform {
        position: at,
        ..Default::default()
    });
    registry.set_component(id, sprite()).unwrap();
    registry
        .set_component(
            id,
            DescriptorProvenance {
                canonical_name: "rocket_launcher".into(),
                owned_components: BTreeSet::new(),
                map_overrides: BTreeSet::new(),
                spawn_path: DescriptorSpawnPath::ProjectilePresentation,
            },
        )
        .unwrap();
    id
}

fn member(entity: EntityId, membership: f32, frozen: bool) -> GhostMember {
    let mapping = mapping();
    let source = Transform {
        position: SOURCE,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    };
    GhostMember {
        link: 0,
        entity,
        category: GhostCategory::Projectiles,
        membership,
        frozen,
        mapping,
        source,
        mapped: Transform {
            position: mapping.point(source.position),
            rotation: mapping.rotation() * source.rotation,
            scale: source.scale,
        },
        look: STYLE,
    }
}

/// One decoded ghost sprite: position, age, size, rotation, look.
#[derive(Debug, PartialEq)]
struct Decoded {
    position: Vec3,
    age: f32,
    size: f32,
    rotation: f32,
    look: GhostInstanceLook,
}

fn decode(bytes: &[u8]) -> Vec<Decoded> {
    let (instances, rest) = bytes.as_chunks::<GHOST_SPRITE_INSTANCE_SIZE>();
    assert!(rest.is_empty(), "whole instances only");
    instances
        .iter()
        .map(|instance| {
            let word =
                |i: usize| u32::from_ne_bytes(instance[i * 4..i * 4 + 4].try_into().unwrap());
            let float = |i: usize| f32::from_bits(word(i));
            Decoded {
                position: Vec3::new(float(0), float(1), float(2)),
                age: float(3),
                size: float(4),
                rotation: float(5),
                look: GhostInstanceLook::unpack([word(6), word(7)]),
            }
        })
        .collect()
}

struct Fixture {
    world: LevelWorld,
    particles: ParticleRenderCollector,
    ghosts: BillboardGhostRenderCollector,
}

impl Fixture {
    fn new() -> Self {
        let mut particles = ParticleRenderCollector::new();
        particles.register_sprite(COLLECTION);
        Self {
            world: two_copy_world(),
            particles,
            ghosts: BillboardGhostRenderCollector::default(),
        }
    }

    fn collect(
        &mut self,
        registry: &EntityRegistry,
        members: &[GhostMember],
        visible: &VisibleCells,
    ) {
        self.ghosts.collect(
            registry,
            members,
            &mut self.particles,
            &BillboardGhostFrame {
                world: &self.world,
                visible,
                presentation_tick: 0.0,
                limiter: None,
            },
        );
    }

    fn drawn(&self) -> Vec<Decoded> {
        let collections: Vec<_> = self.ghosts.iter_collections(&self.particles).collect();
        assert!(collections.iter().all(|(name, _)| *name == COLLECTION));
        collections
            .into_iter()
            .flat_map(|(_, bytes)| decode(bytes))
            .collect()
    }
}

// Billboards: a projectile billboard ghost draws when its
// mapped cell is visible and not otherwise, with the real cell visible and not.
#[test]
fn billboard_ghost_draws_when_its_mapped_cell_is_visible_whatever_the_real_cell() {
    let mut registry = EntityRegistry::new();
    let shot = spawn_projectile_copy(&mut registry, SOURCE);
    let members = [member(shot, 1.0, false)];
    let mut fixture = Fixture::new();
    assert_eq!(
        fixture.world.locate_cell(members[0].mapped.position) as u32,
        MAPPED_CELL
    );
    for (visible, draws) in [
        (vec![REAL_CELL, MAPPED_CELL], true),
        (vec![MAPPED_CELL], true),
        (vec![REAL_CELL], false),
        (vec![], false),
    ] {
        fixture.collect(&registry, &members, &VisibleCells::Culled(visible.clone()));
        assert_eq!(fixture.drawn().len(), usize::from(draws), "{visible:?}");
    }
}

// The ghost sprite sits at the mapped position with the body's size, rotation
// and age, its presented opacity the link's opacity times membership times the
// sprite's own, and the link's tint and saturation.
#[test]
fn billboard_ghost_packs_the_mapped_position_and_the_presented_look() {
    let mut registry = EntityRegistry::new();
    let shot = spawn_projectile_copy(&mut registry, SOURCE);
    let ghost = member(shot, 0.5, false);
    let mut fixture = Fixture::new();
    fixture.collect(&registry, &[ghost], &VisibleCells::DrawAll);
    let drawn = fixture.drawn();
    assert_eq!(drawn.len(), 1);
    let drawn = &drawn[0];
    assert!(drawn.position.abs_diff_eq(ghost.mapped.position, 1e-5));
    assert_eq!((drawn.size, drawn.rotation, drawn.age), (0.75, 0.3, 0.0));
    let opacity = STYLE.opacity * 0.5 * sprite().opacity;
    assert!(
        (drawn.look.opacity - opacity).abs() < 1e-4,
        "{:?}",
        drawn.look
    );
    assert!((drawn.look.saturation - STYLE.saturation).abs() < 1e-4);
    for channel in 0..3 {
        assert!((drawn.look.tint[channel] - STYLE.tint[channel]).abs() < 0.5 / 255.0 + 1e-6);
    }
}

// A fading billboard ghost redraws its last sprite at its last in-volume
// mapped position after its projectile despawns.
#[test]
fn a_fading_billboard_ghost_redraws_its_cached_sprite_after_despawn() {
    let mut registry = EntityRegistry::new();
    let shot = spawn_projectile_copy(&mut registry, SOURCE);
    let mut fixture = Fixture::new();
    fixture.collect(
        &registry,
        &[member(shot, 1.0, false)],
        &VisibleCells::DrawAll,
    );
    registry.despawn(shot).unwrap();

    let fading = member(shot, 0.4, true);
    fixture.collect(&registry, &[fading], &VisibleCells::DrawAll);
    let drawn = fixture.drawn();
    assert_eq!(drawn.len(), 1);
    assert!(drawn[0].position.abs_diff_eq(fading.mapped.position, 1e-5));

    // Faded out: no member, no sprite, and nothing left to redraw.
    fixture.collect(&registry, &[], &VisibleCells::DrawAll);
    assert!(fixture.drawn().is_empty());
    fixture.collect(&registry, &[fading], &VisibleCells::DrawAll);
    assert!(fixture.drawn().is_empty(), "a dropped sprite never returns");
}

// Only projectile billboard bodies ghost here: a bare sprite entity is never a
// billboard body, and a model-bodied projectile draws through the skinned ghost
// collector instead.
#[test]
fn bare_sprites_and_model_bodied_projectiles_draw_no_billboard_ghost() {
    let mut registry = EntityRegistry::new();
    let bare = registry.spawn(Transform::default());
    registry.set_component(bare, sprite()).unwrap();
    let model = spawn_projectile_copy(&mut registry, SOURCE);
    registry.remove_component::<SpriteVisual>(model).unwrap();
    registry
        .set_component(model, MeshComponent::stateless("models/rocket.glb".into()))
        .unwrap();
    let mut fixture = Fixture::new();
    fixture.collect(
        &registry,
        &[member(bare, 1.0, false), member(model, 1.0, false)],
        &VisibleCells::DrawAll,
    );
    assert!(fixture.drawn().is_empty());
}

// Regression: billboard ghosts tested membership and drew at the interpolated
// pose, up to a tick behind the uninterpolated billboard the real pass draws.
#[test]
fn a_billboard_body_presents_at_its_current_transform_and_other_members_interpolate() {
    let mut registry = EntityRegistry::new();
    let previous = Vec3::new(10.0, 1.0, 2.0);
    let shot = spawn_projectile_copy(&mut registry, previous);
    let pawn = registry.spawn(Transform {
        position: previous,
        ..Default::default()
    });
    registry
        .set_component(pawn, MeshComponent::stateless("models/pawn.glb".into()))
        .unwrap();
    // A tick moves both; neither has been snapshotted since spawn.
    for id in [shot, pawn] {
        registry
            .set_component(
                id,
                Transform {
                    position: SOURCE,
                    ..Default::default()
                },
            )
            .unwrap();
    }
    let alpha = 0.5;
    let billboard = ghost_presented_transform(&registry, shot, alpha).unwrap();
    assert!(billboard.position.abs_diff_eq(SOURCE, 1e-6));
    let mesh = ghost_presented_transform(&registry, pawn, alpha).unwrap();
    assert!(
        mesh.position
            .abs_diff_eq(previous.lerp(SOURCE, alpha), 1e-5)
    );
}
