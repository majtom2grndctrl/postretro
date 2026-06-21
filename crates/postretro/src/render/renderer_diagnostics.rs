// Renderer query + diagnostics methods: SH/SDF/lightmap state queries and the
// dev-tools diagnostic overlays (SH, nav, agent paths, wireframe).
// See: context/lib/rendering_pipeline.md

use super::*;

impl Renderer {
    /// `true` when the loaded map carries a baked SH volume. The diagnostic
    /// panel queries this to render either live controls or a disabled-state label.
    #[cfg(feature = "dev-tools")]
    pub fn has_sh_volume(&self) -> bool {
        self.sh_volume_resources.present
    }

    /// `true` when the loaded map carries a baked SDF static-occluder atlas.
    /// The SDF shadow pass gates its dispatch on this; the SDF visibility
    /// applies to the per-light `sdf`-tagged diffuse/specular forward loops,
    /// not to `lm_irr`. Legacy PRLs report `false` and the renderer degrades
    /// cleanly to `main`-equivalent lighting.
    #[allow(dead_code)]
    pub fn has_sdf_atlas(&self) -> bool {
        self.sdf_atlas_resources.present
    }

    /// Borrow the SDF atlas resources. The SDF shadow pass consumes the
    /// bind group + layout here; no other pass should bind these — forward
    /// gets only an upsampled shadow-factor texture in group 5.
    #[allow(dead_code)]
    pub fn sdf_atlas_resources(&self) -> &SdfAtlasResources {
        &self.sdf_atlas_resources
    }

    /// Lightmap bake mode read from the PRL (Shadowed = visibility baked in).
    /// Under the disjoint-direct design, `sdf` lights are excluded from
    /// `lm_irr` at bake time, so the forward pass never multiplies SDF
    /// visibility into the static-lightmap term; this accessor is retained
    /// only for legacy-PRL compatibility.
    #[allow(dead_code)]
    pub fn lightmap_mode(&self) -> crate::prl::LightmapMode {
        self.lightmap_mode
    }

    /// Per-animated-light delta-volume metadata for the SH diagnostic overlay.
    /// Empty when the map has no delta SH volumes.
    #[cfg(feature = "dev-tools")]
    pub fn sh_delta_volumes(&self) -> &[sh_volume::DeltaVolumeMeta] {
        &self.sh_delta_volumes_meta
    }

    /// Emits SH diagnostic line segments into the renderer's per-frame debug-line
    /// buffer. Called from the frame loop between egui UI build and
    /// `render_frame_indirect`. The caller is responsible for clearing the
    /// debug-line buffer before this call (via `clear_debug_lines`) so the
    /// emit path stays purely additive and other debug-line producers can
    /// coexist; this also keeps the buffer bounded across early-return frames
    /// (Timeout/Occluded/Outdated) where `render_frame_indirect` skips its
    /// debug-line render pass.
    ///
    /// `visible_leaf_mask` is the same portal-reachable leaf mask passed to
    /// `render_frame_indirect`; the cells overlay colors each cell by the
    /// frame-visibility of the leaf its center sits in.
    #[cfg(feature = "dev-tools")]
    pub fn emit_sh_diagnostics(
        &mut self,
        state: &sh_diagnostics::ShDiagnosticsState,
        camera_pos: Vec3,
        world: &crate::prl::LevelWorld,
        visible_leaf_mask: &[bool],
    ) {
        // Drive the live atlas readback only while the irradiance overlay is
        // actually drawn — every other frame it costs nothing.
        let want_live_irradiance = state.show_markers
            && state.marker_mode == sh_diagnostics::MarkerMode::Irradiance
            && self.sh_volume_resources.present;
        self.sh_probe_readback.set_wanted(want_live_irradiance);

        sh_diagnostics::emit(
            state,
            &self.sh_volume_resources,
            &self.sh_delta_volumes_meta,
            camera_pos,
            world,
            visible_leaf_mask,
            &mut self.debug_lines,
        );
    }

    /// Emit navmesh diagnostic debug lines (region rectangles + portal edges)
    /// from the runtime nav graph. No-op while the overlay is toggled off.
    /// Must run after `clear_debug_lines` and before the frame's debug-line
    /// pass, mirroring `emit_sh_diagnostics`.
    #[cfg(feature = "dev-tools")]
    pub fn emit_nav_diagnostics(&mut self, graph: &crate::nav::NavGraph) {
        if !self.show_navmesh {
            return;
        }
        nav_diagnostics::emit(graph, &mut self.debug_lines);
    }

    /// Emit agent path/corridor diagnostic debug lines: the corridor from the
    /// agent's `position` through its remaining funnel waypoints (from `cursor`),
    /// plus a per-waypoint cross marker sized to the capsule `radius`. Gated by
    /// the same navmesh overlay toggle (`Alt+Shift+N`) so the path draws
    /// alongside the region/portal overlay. Must run after `clear_debug_lines`
    /// and before the frame's debug-line pass, mirroring `emit_nav_diagnostics`.
    ///
    /// Keeps all wgpu renderer-side (Renderer-owns-GPU): the call site hands in
    /// plain agent geometry, never a debug-line / wgpu handle. The render-private
    /// `nav_diagnostics::emit_agent_path` (it is `pub(super)`) is reached only
    /// through this wrapper.
    #[cfg(feature = "dev-tools")]
    pub fn emit_agent_path_overlay(
        &mut self,
        position: Vec3,
        path: &[Vec3],
        cursor: usize,
        radius: f32,
    ) {
        if !self.show_navmesh {
            return;
        }
        nav_diagnostics::emit_agent_path(position, path, cursor, radius, &mut self.debug_lines);
    }

    /// Draw an "ugly-but-honest" wireframe capsule at each replicated remote
    /// entity position (M15 Phase 1 netcode visibility aid). `centers` are the
    /// pawn `Transform.position`s collected client-side by
    /// `netcode::remote_entity_positions`; `radius`/`half_height` size the
    /// capsule to the standing player volume. Cyan so it reads clearly against
    /// the world. No-op when `centers` is empty (single-player and host).
    ///
    /// Routed through the **overlay (always-on-top)** debug-line path
    /// (`push_capsule_overlay`), NOT the depth-tested `push_capsule`: the marker's
    /// whole job is "where is the other player," so it must stay visible when the
    /// remote pawn moves behind world geometry. The depth-tested path culls the
    /// capsule's fragments against the opaque world depth buffer, which made the
    /// capsule vanish whenever a wall came between the client camera and the host
    /// pawn — the "gets lost after moving around" symptom.
    ///
    /// Renderer-owns-GPU: the call site hands in plain `Vec3` positions and
    /// capsule dims — never a debug-line / wgpu handle. Must run after
    /// `clear_debug_lines` and before the frame's debug-line pass, mirroring the
    /// other per-frame overlay emitters.
    #[cfg(feature = "dev-tools")]
    pub fn emit_remote_entity_markers(&mut self, centers: &[Vec3], radius: f32, half_height: f32) {
        const REMOTE_ENTITY_COLOR: [u8; 4] = [0, 255, 255, 255]; // cyan
        for &center in centers {
            self.debug_lines
                .push_capsule_overlay(center, radius, half_height, REMOTE_ENTITY_COLOR);
        }
    }

    /// Flip the navmesh overlay on/off. Bound to `Alt+Shift+N`.
    #[cfg(feature = "dev-tools")]
    pub fn toggle_navmesh_overlay(&mut self) -> bool {
        self.show_navmesh = !self.show_navmesh;
        log::info!(
            "[Renderer] Navmesh overlay: {}",
            if self.show_navmesh { "on" } else { "off" },
        );
        self.show_navmesh
    }

    pub fn toggle_wireframe(&mut self) -> bool {
        self.wireframe_enabled = !self.wireframe_enabled;
        log::info!(
            "[Renderer] Wireframe overlay: {}",
            if self.wireframe_enabled { "on" } else { "off" },
        );
        self.wireframe_enabled
    }
}
