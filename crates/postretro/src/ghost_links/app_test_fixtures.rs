// Test fixture: a windowless App holding installed ghost links, driven through the App's own
// per-tick and per-frame ghost-link seams in the order the frame loop runs them.
// See: context/lib/entity_model.md §7d

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use glam::Vec3;
use postretro_entities::components::mesh::MeshComponent;
use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
use postretro_entities::{ComponentKind, ComponentValue, EntityId, ScriptCtx, Transform};
use postretro_foundation::GhostStyleRegistry;
use postretro_level_format::environment_volumes::{EnvironmentBrush, EnvironmentPlane};
use postretro_level_format::ghost_links::{GhostLinkRecord, GhostMembers};
use postretro_level_loader::LevelWorld;
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;
use serde_json::Value;

use super::opt_ins_test_fixtures::descriptor;
use super::{GhostLinks, GhostMember, spawn_loaded_ghost_links};

/// One fixed tick per presented frame, at 60 Hz.
pub(crate) const FRAME_SECONDS: f64 = 1.0 / 60.0;
/// The render sub-tick alpha every frame presents at.
const ALPHA: f32 = 0.5;

/// The box `min..max` as a convex brush with its six planes.
pub(crate) fn box_brush(min: Vec3, max: Vec3) -> EnvironmentBrush {
    let planes = (0..3)
        .flat_map(|axis| {
            let mut normal = [0.0; 3];
            normal[axis] = 1.0;
            let mut negative = normal;
            negative[axis] = -1.0;
            [
                EnvironmentPlane {
                    normal,
                    distance: max[axis],
                },
                EnvironmentPlane {
                    normal: negative,
                    distance: -min[axis],
                },
            ]
        })
        .collect();
    EnvironmentBrush {
        aabb_min: min.to_array(),
        aabb_max: max.to_array(),
        planes,
    }
}

/// A link over the box `volume`, mapping `landmark` (origin, yaw) onto
/// `target`, naming `style`.
pub(crate) fn box_link(
    source_entity: u32,
    volume: (Vec3, Vec3),
    landmark: (Vec3, f32),
    target: (Vec3, f32),
    enabled_on_spawn: bool,
    members: GhostMembers,
    style: &str,
) -> GhostLinkRecord {
    GhostLinkRecord {
        source_entity,
        enabled_on_spawn,
        members,
        style: style.to_string(),
        brushes: vec![box_brush(volume.0, volume.1)],
        landmark_origin: landmark.0.to_array(),
        landmark_yaw: landmark.1,
        target_origin: target.0.to_array(),
        target_yaw: target.1,
        ghost_tags: Vec::new(),
    }
}

/// A windowless App with `world`'s links installed and bound against a
/// committed style table, and the named descriptors opted into ghosting.
/// Single player: no endpoint, so the App's own tick counter stamps link
/// history.
pub(crate) struct LinkedApp {
    pub app: crate::App,
    /// The App session's script context (shared, not a copy).
    pub ctx: ScriptCtx,
    steps: SequencedPrimitiveRegistry,
    /// Script time, advanced one frame per [`Self::tick_and_frame`].
    time: f64,
    now: Instant,
}

impl LinkedApp {
    pub(crate) fn new(world: &LevelWorld, styles: GhostStyleRegistry, opted_in: &[&str]) -> Self {
        let mut app = crate::startup::lifecycle::tests::test_app();
        let ctx = app
            .session
            .as_ref()
            .expect("test app session installed")
            .scripting
            .script_ctx
            .clone();
        spawn_loaded_ghost_links(&mut ctx.registry.borrow_mut(), world);
        app.ghost_links = GhostLinks::install(world, &ctx.registry.borrow());
        app.ghost_links.bind_styles(world, &styles);
        {
            // What mod init committed: the descriptors and the style table.
            let mut data = ctx.data_registry.borrow_mut();
            data.replace_entity_types(opted_in.iter().map(|name| descriptor(name, true)).collect());
            data.replace_ghost_styles(styles);
        }
        let mut steps = SequencedPrimitiveRegistry::new();
        postretro_sim::ghost_link_commands::register_sequenced_ghost_link_primitives(
            &mut steps,
            ctx.clone(),
        );
        Self {
            app,
            ctx,
            steps,
            time: 0.0,
            now: Instant::now(),
        }
    }

    /// Link `index`'s installed entity.
    pub(crate) fn link(&self, index: u16) -> EntityId {
        self.ctx
            .registry
            .borrow()
            .iter_with_kind(ComponentKind::GhostLink)
            .find_map(|(id, value)| match value {
                ComponentValue::GhostLink(link) if link.index == index => Some(id),
                _ => None,
            })
            .expect("install spawned the link")
    }

    /// A meshed descriptor entity materialized from `name` at map load,
    /// presented at `at`.
    pub(crate) fn entity(&self, name: &str, at: Vec3) -> EntityId {
        let id = self.ctx.registry.borrow_mut().spawn(Transform::default());
        self.place(id, at);
        let mut registry = self.ctx.registry.borrow_mut();
        registry
            .set_component(
                id,
                DescriptorProvenance {
                    canonical_name: name.into(),
                    owned_components: BTreeSet::new(),
                    map_overrides: BTreeSet::new(),
                    spawn_path: DescriptorSpawnPath::MapPlacement,
                },
            )
            .unwrap();
        registry
            .set_component(id, MeshComponent::stateless(format!("models/{name}.gltf")))
            .unwrap();
        id
    }

    /// Present `id` at `at` at any alpha.
    pub(crate) fn place(&self, id: EntityId, at: Vec3) {
        self.ctx
            .registry
            .borrow_mut()
            .set_presentation_transform(
                id,
                Transform {
                    position: at,
                    ..Default::default()
                },
            )
            .unwrap();
    }

    /// A member step on `link`, run as the sequence dispatcher runs it; it
    /// applies at the next tick's start.
    pub(crate) fn step(&self, link: EntityId, primitive: &str, args: Value) {
        self.steps.get(primitive).expect("registered")(link, &args).expect("a valid step");
    }

    /// One fixed tick then one presented frame, through the App: queued steps
    /// apply at the tick's start, the tick's end records link history, and
    /// the frame evaluates membership after every presented pose is written.
    pub(crate) fn tick_and_frame(&mut self) {
        self.time += FRAME_SECONDS;
        self.ctx.presentation_script_time.set(self.time);
        self.app.apply_pending_ghost_link_commands();
        self.app.record_ghost_link_history();
        self.now += Duration::from_secs_f64(FRAME_SECONDS);
        self.app.update_ghost_links(ALPHA, self.now);
    }

    /// This frame's ghosts.
    pub(crate) fn members(&self) -> &[GhostMember] {
        self.app.ghost_links.members()
    }

    /// `entity`'s ghost through link `link` this frame.
    pub(crate) fn ghost(&self, link: u16, entity: EntityId) -> Option<GhostMember> {
        self.members()
            .iter()
            .find(|member| member.link == link && member.entity == entity)
            .copied()
    }
}
