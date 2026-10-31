//! Level install's presentation tail: camera pose, fog masks, sprite
//! collections and level sounds.
//! See: context/lib/boot_sequence.md §3

use super::*;

/// Where a level's fog pixel scale and per-cell masks reach the renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LevelFogMasks {
    /// The main-thread install sets them on the active set here.
    Upload,
    /// A load's set was built with both from the same level (its fog target
    /// at the level's pixel scale, its masks, cold fog state), so nothing
    /// reaches the renderer here.
    InSet,
}

/// Where a level's sounds come from.
#[derive(Debug)]
pub(super) enum LevelSounds {
    /// The main-thread install decodes the mod's `sounds/` here.
    #[cfg_attr(not(test), allow(dead_code))]
    Load,
    /// A load decoded them on its pool: `None` when its dispatch had no
    /// audio, which decodes none.
    Decoded(Option<postretro_audio::DecodedLevelSounds>),
}

impl App {
    /// Camera at the first spawn, the `--start-pose` probe, then the presented
    /// pose.
    pub(super) fn install_level_camera_pose(&mut self, first_spawn: Option<(Vec3, Vec3)>) {
        // Teleport the camera to the first player spawn (or the geometry center
        // when the map has none). Independent of spawn success.
        if let Some((pos, angles)) = first_spawn {
            self.camera.position = pos;
            // angles is engine-convention radians (YXZ): x=pitch, y=yaw.
            self.camera.yaw = angles.y;
            self.camera.pitch = angles.x;
        } else if let Some(world) = self.level.as_ref() {
            // Fallback when no player_spawn: center on level geometry.
            self.camera.position = world.spawn_position();
        }
        // `--start-pose` moves the local pawn (or, pawnless, the fly camera)
        // to a checked-in measurement probe instead of the map spawn, on the
        // session's first gameplay install only. A frontend backdrop install
        // (menu on the stack) leaves the pose for the map the player picks.
        let start_pose = if self.frontend_menu_is_present() {
            None
        } else {
            self.session_boot_config.take_start_pose()
        };
        if let Some(pose) = start_pose {
            let moved = self.session.as_ref().is_some_and(|session| {
                crate::startup::start_pose::place_local_pawn(
                    &mut session.scripting.script_ctx.registry.borrow_mut(),
                    pose,
                )
            });
            self.camera.position = pose.position;
            self.camera.yaw = pose.yaw;
            self.camera.pitch = pose.pitch;
            log::info!(
                "[Startup] start pose {:?} yaw {:.1}° pitch {:.1}° ({})",
                pose.position,
                pose.yaw.to_degrees(),
                pose.pitch.to_degrees(),
                if moved { "local pawn" } else { "camera only" },
            );
        }
        // Put the camera at the followed local pawn's eye (the point every tick
        // moves it to), else leave it where the start pose placed it, then move
        // it to the presented pose: that spawn pose, or the menu pose when the
        // frontend menu is up. Both interpolation endpoints hold it, so a frame
        // before the first tick renders from the pose Settling made resident,
        // and the first tick blends from it rather than from the pawn's origin
        // or the previous level's pose.
        self.camera.position = self.followed_pawn_eye().unwrap_or(self.camera.position);
        self.place_camera_at_presented_pose();
        self.level_timings.record("camera_pose");
    }

    /// The renderer half of the fog bridge: pixel scale and per-cell masks,
    /// unless the level's set was built with them (`LevelFogMasks::InSet`).
    pub(super) fn install_level_fog_masks(&mut self, fog_masks: LevelFogMasks) {
        // Renderer-side fog: pixel scale + per-cell masks. The fog-volume entities
        // were created in segment B; this is the windowed GPU half.
        if fog_masks == LevelFogMasks::Upload
            && let Some(world) = self.level.as_ref()
            && let Some(renderer) = self.renderer.as_mut()
        {
            renderer.set_fog_pixel_scale(world.fog_pixel_scale);
            renderer.install_fog_cell_masks_for_level(world.fog_cell_masks.clone());
        }
        self.level_timings.record("fog_masks");
    }

    /// Sprite collections for every distinct emitter `sprite`, after
    /// `levelLoad`. Returns the GPU objects registration created on this
    /// thread: a named, counted exception at a load's commit.
    pub(super) fn install_level_sprite_collections(
        &mut self,
        script_ctx: &postretro_entities::ScriptCtx,
        prm_cache_root: &Path,
        suppress: bool,
    ) -> postretro_renderer::GpuCreationCounts {
        let before = postretro_renderer::gpu_creation_counts();
        // Register sprite collections for every distinct emitter `sprite` in the
        // registry — map-spawned and descriptor-spawned alike — plus descriptor
        // projectile bodies/trails, suppressed descriptor emitters, resolved
        // spawner emitters and the weapon impact collection. Deferred presentation
        // must have its assets ready before gameplay; uploads stay renderer-owned.
        if let Some(renderer) = self.renderer.as_mut() {
            let texture_root = self.content_root.join("textures");
            let map_billboard_collections = self
                .level
                .as_ref()
                .map(|world| {
                    lifecycle_sprite_collections::map_billboard_sprite_collections(
                        &world.map_entities,
                    )
                })
                .unwrap_or_default();
            let registry = script_ctx.registry.borrow();
            let sprite_candidates = {
                let data_registry = script_ctx.data_registry.borrow();
                let projectile_sprites =
                    crate::scripting::builtins::data_archetype::projectile_presentation_assets(
                        &data_registry.entities,
                    )
                    .1;
                let suppressed_placements = self
                    .level
                    .as_ref()
                    .filter(|_| suppress)
                    .map_or(&[][..], |world| world.map_entities.as_slice());
                lifecycle_sprite_collections::discover_sprite_collection_candidates(
                    &registry,
                    projectile_sprites,
                    suppressed_placements,
                    &data_registry.entities,
                )
            };
            let particle_render = &mut self
                .session
                .as_mut()
                .expect("session installed before level install")
                .particle_render;
            lifecycle_sprite_collections::install_sprite_collections(
                renderer,
                particle_render,
                &texture_root,
                prm_cache_root,
                sprite_candidates,
                &map_billboard_collections,
            );
        }
        self.level_timings.record("sprite_collections");
        postretro_renderer::gpu_creation_counts().since(before)
    }

    /// The level's sounds, last in install: decoded here, or registered
    /// from a load's decode.
    pub(super) fn install_level_sounds(&mut self, sounds: LevelSounds) {
        // Sound registry follows level lifetime, parallel to textures: load the
        // level's sounds from `sounds/`, released at unload. Fault-tolerant — a
        // missing directory or undecodable file warns and is skipped; silent if
        // audio init failed (`audio` is `None`). Session-owned; clone the content
        // root so the `self.session` borrow does not alias the read. Deliberately
        // last in install, after `levelLoad`: safe because `playSound` enqueues an
        // async `SystemReactionCommand` drained a frame later, after install
        // completes, so no reaction observes unloaded sounds.
        let content_root = self.content_root.clone();
        if let Some(session) = self.session.as_mut() {
            if let Some(audio) = session.audio.as_mut() {
                match sounds {
                    LevelSounds::Load => audio.load_level_sounds(&content_root),
                    // Registered in sorted path order, whatever order the
                    // pool decoded them in.
                    LevelSounds::Decoded(Some(decoded)) => audio.register_level_sounds(decoded),
                    // A dispatch without audio decoded none, as a level with
                    // no `sounds/` registers none; audio never starts
                    // mid-session, so this replaces the registry with nothing.
                    LevelSounds::Decoded(None) => audio.release_level_sounds(),
                }
            }
            session.refresh_descriptor_sounds();
        }
        self.level_timings.record("audio_load");
    }
}
