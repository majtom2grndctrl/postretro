// Planar water mirror preparation: plane selection, the mirror's seeded walk, and its demand sets.
// See: context/lib/rendering_pipeline.md §2, §4, §7.3a

use postretro_level_loader::LevelWorld;
use postretro_render_cpu::planar_reflection::{
    MirrorCamera, PlanarCandidates, PlanarPreset, SelectedPlane, collect_mirror_seed_cells,
    mark_seeded_planes, select_plane,
};
use postretro_stage_timing::StageFrame;
use postretro_visibility::{
    FrustumPlane, MirrorCropScratch, MirrorView, MirrorVisibility, MirrorVisibilityPath,
    PortalWalkScratch, VisibilityPath, VisibilityStage, VisibleCells,
    determine_mirror_visible_cells,
};

use super::VisibleRenderPreparation;
use crate::camera::RenderCamera;

/// The frame's planar mirror, prepared after the camera walk and before any
/// consumer reads visibility (`rendering_pipeline.md` §7.3a). Every buffer is
/// reused across frames, so a steady state allocates nothing.
///
/// The main view's visible and fog sets are never written: the main cull and
/// fog read them unchanged with planar on and off. The mirror adds cells only
/// to the sets this type hands out, under the §4 path rules: cells it draws
/// join collection, lightmap demand and SH sample regions on every path
/// (lightmap demand holds rather than reads on a non-walk path), and join SH
/// visible targeting only on a completed mirror walk with the main view on a
/// walk path.
pub(crate) struct PlanarRenderPreparation {
    candidates: PlanarCandidates,
    /// The plane selected last frame: selection's hysteresis input. Cleared at
    /// install, so a new level or fluid table never inherits it.
    selected: Option<SelectedPlane>,
    /// This frame's mirror view, its cull cropped to the selected plane;
    /// `None` with no plane (none above the area floor, seeded in the
    /// camera's reach and above the eye's fog band), below very high, with
    /// no part of the plane on screen, or with no seed in the cropped mirror
    /// frustum. With `None` every water surface reads SSR at the tier.
    camera: Option<MirrorCamera>,
    /// The crop's polygon clip buffers.
    crop_scratch: MirrorCropScratch,
    /// The seed domain on a main view whose visible set is `DrawAll`.
    seed_domain: Vec<u32>,
    /// Per candidate: a seed cell in this frame's camera reach. Selection's
    /// seed-in-reach filter reads it.
    seeded: Vec<bool>,
    seed_cells: Vec<u32>,
    seeds: Vec<usize>,
    /// The mirror's own walk state, never the camera's.
    scratch: PortalWalkScratch,
    mirror: MirrorVisibility,
    /// The mirror's drawn cells, as the renderer's mirror cull takes them.
    visible: VisibleCells,
    /// Main visible ∪ mirror reach, ascending: what the frame draws.
    drawn: VisibleCells,
    /// Main fog reach ∪ mirror reach, ascending; empty on the main view's
    /// unisolated sentinel.
    fog: Vec<u32>,
    /// A completed mirror walk with the main view on a walk path: the one
    /// case where mirror cells may add reads.
    reads: bool,
    previous_diagnostic: Option<(usize, usize, usize, MirrorVisibilityPath)>,
    /// A test-only step cap in place of the shared one, to force a trip.
    #[cfg(test)]
    step_cap: Option<u32>,
    /// Whether the mirror's walk and cull are cropped to the selected plane:
    /// always, except where a test, the crop-parity capture or the
    /// diagnostics panel turns it off.
    #[cfg(any(test, feature = "capture", feature = "dev-tools"))]
    crop: bool,
}

impl Default for PlanarRenderPreparation {
    fn default() -> Self {
        Self {
            candidates: PlanarCandidates::default(),
            selected: None,
            camera: None,
            crop_scratch: MirrorCropScratch::default(),
            seed_domain: Vec::new(),
            seeded: Vec::new(),
            seed_cells: Vec::new(),
            seeds: Vec::new(),
            scratch: PortalWalkScratch::new(),
            mirror: MirrorVisibility::default(),
            visible: VisibleCells::Culled(Vec::new()),
            drawn: VisibleCells::Culled(Vec::new()),
            fog: Vec::new(),
            reads: false,
            previous_diagnostic: None,
            #[cfg(test)]
            step_cap: None,
            #[cfg(any(test, feature = "capture", feature = "dev-tools"))]
            crop: true,
        }
    }
}

impl PlanarRenderPreparation {
    /// Install `world`'s candidate planes: the probe-bound up-facing faces
    /// of `reflecting_volumes`, the volumes whose fluid reflects, grouped by
    /// height, less planes under the area floor. Resets selection history
    /// (O11), at level install and at every fluid refresh, so the first frame
    /// selects without hysteresis. The crop switch carries over, so a refresh
    /// mid-capture keeps it.
    pub(crate) fn install(&mut self, world: &LevelWorld, reflecting_volumes: &[u32]) {
        *self = Self {
            candidates: PlanarCandidates::from_volumes(
                &world.environment_volumes.volumes,
                reflecting_volumes,
                world.reflection_probes.as_ref(),
            ),
            #[cfg(any(test, feature = "capture", feature = "dev-tools"))]
            crop: self.crop,
            ..Self::default()
        };
    }

    /// Drop the level's candidates and mirror state at unload.
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    /// One frame's mirror, after the camera walk (`main`). `preset` is the
    /// tier's planar preset, read once this frame from the tier the renderer
    /// draws with: `None` below very high runs no selection and no walk, and
    /// adds nothing to any set. The mirror walk's time goes into `cpu`, the
    /// frame's visibility stages.
    pub(crate) fn prepare(
        &mut self,
        world: &LevelWorld,
        main_camera: &RenderCamera,
        main: &VisibleRenderPreparation,
        preset: Option<PlanarPreset>,
        blocked_portals: &[bool],
        cpu: &StageFrame<VisibilityStage>,
    ) {
        self.camera = None;
        self.reads = false;
        if let VisibleCells::Culled(cells) = &mut self.visible {
            cells.clear();
        }
        let Some(preset) = preset else {
            self.selected = None;
            self.previous_diagnostic = None;
            return;
        };
        let crop = self.crop_enabled();
        // O12, O14: the seed-in-reach filter, from this frame's camera reach
        // (the fog reach on a walk path, the drawn set on a fallback), before
        // scoring and hysteresis. The area floor applied at install; the eye
        // band applies inside selection, before scoring too.
        let domain = seed_domain(main, world, &mut self.seed_domain);
        mark_seeded_planes(
            &self.candidates,
            domain,
            world,
            main_camera.eye_position,
            preset.r,
            &mut self.seeded,
        );
        self.selected = select_plane(
            &self.candidates,
            main_camera.eye_position,
            main_camera.view_projection,
            &self.seeded,
            self.selected,
        );
        let Some(selected) = self.selected else {
            self.previous_diagnostic = None;
            return;
        };
        let projection = main_camera.view_projection * main_camera.view_matrix.inverse();
        let Some(camera) = MirrorCamera::new(
            main_camera.eye_position,
            main_camera.view_matrix,
            projection,
            selected.h,
            preset.d,
        ) else {
            self.selected = None;
            self.previous_diagnostic = None;
            return;
        };
        // The crop (O1: after selection, before the walk): the walk and the
        // mirror cull take the plane's padded on-screen bounds, so the trip
        // set filters against them too (O5). Raster and clip stay uncropped.
        let camera = if crop {
            let cropped = self
                .candidates
                .planes
                .get(selected.candidate_index)
                .and_then(|plane| {
                    camera.cropped_to_plane(
                        plane,
                        main_camera.view_projection,
                        &mut self.crop_scratch,
                    )
                });
            let Some(cropped) = cropped else {
                // No face keeps any part on screen, so no mirror sample lands
                // anywhere: no mirror view this frame, and selection holds.
                self.previous_diagnostic = None;
                return;
            };
            cropped
        } else {
            camera
        };

        // Seeds come from the reach the filter read, so the selected plane
        // has at least one.
        collect_mirror_seed_cells(
            domain,
            world,
            &selected,
            main_camera.eye_position,
            preset.r,
            &mut self.seed_cells,
        );
        self.seeds.clear();
        self.seeds
            .extend(self.seed_cells.iter().map(|&cell| cell as usize));

        let view = MirrorView {
            eye: camera.eye,
            view_proj: camera.cull_vp,
            // The clip plane: the water lowered by `PLANAR_CLIP_BIAS`, the
            // near plane the mirror rasters with.
            near_plane: FrustumPlane {
                normal: camera.clip_plane.truncate(),
                dist: camera.clip_plane.w,
            },
        };
        self.walk(&view, main, world, blocked_portals, cpu);
        if self.mirror.path == MirrorVisibilityPath::NoSeeds {
            // Every seed missed the cropped mirror frustum: no mirror sample
            // lands, so no mirror view, and the water reads SSR at the tier.
            // Selection holds, as with an empty crop.
            self.previous_diagnostic = None;
            return;
        }
        self.camera = Some(camera);
        match &mut self.visible {
            VisibleCells::Culled(cells) => cells.clone_from(&self.mirror.visible_cells),
            visible => *visible = VisibleCells::Culled(self.mirror.visible_cells.clone()),
        }
        self.reads = matches!(main.stats.path, VisibilityPath::PrlPortal { .. })
            && self.mirror.path == MirrorVisibilityPath::Walked;
        union_cells(
            &main.visible_cells,
            &self.mirror.reach_cells,
            &mut self.drawn,
        );
        self.fog.clear();
        if !main.fog_reachable.is_empty() {
            self.fog.extend_from_slice(&main.fog_reachable);
            self.fog.extend_from_slice(&self.mirror.reach_cells);
            self.fog.sort_unstable();
            self.fog.dedup();
        }
        self.log_change(selected, main);
    }

    fn walk(
        &mut self,
        view: &MirrorView,
        main: &VisibleRenderPreparation,
        world: &LevelWorld,
        blocked_portals: &[bool],
        cpu: &StageFrame<VisibilityStage>,
    ) {
        #[cfg(test)]
        if let Some(step_cap) = self.step_cap {
            postretro_visibility::determine_mirror_visible_cells_capped(
                view,
                &self.seeds,
                &main.visible_cells,
                world,
                blocked_portals,
                step_cap,
                &mut self.scratch,
                cpu,
                &mut self.mirror,
            );
            return;
        }
        determine_mirror_visible_cells(
            view,
            &self.seeds,
            &main.visible_cells,
            world,
            blocked_portals,
            &mut self.scratch,
            cpu,
            &mut self.mirror,
        );
    }

    /// One debug line whenever the plane, the mirror's set sizes or its path
    /// change; nothing in a steady frame.
    fn log_change(&mut self, selected: SelectedPlane, main: &VisibleRenderPreparation) {
        let mirror_only = match (&self.drawn, &main.visible_cells) {
            (VisibleCells::Culled(drawn), VisibleCells::Culled(main)) => {
                drawn.len().saturating_sub(main.len())
            }
            _ => 0,
        };
        let diagnostic = (
            selected.candidate_index,
            self.mirror.visible_cells.len(),
            mirror_only,
            self.mirror.path,
        );
        if self.previous_diagnostic == Some(diagnostic) {
            return;
        }
        log::debug!(
            "[Planar reflection] plane={:.3} seeds={} visible={} reach={} mirror_only={} \
             path={:?} considered={} accepted={}",
            selected.h,
            self.seeds.len(),
            self.mirror.visible_cells.len(),
            self.mirror.reach_cells.len(),
            mirror_only,
            self.mirror.path,
            self.mirror.considered,
            self.mirror.accepted,
        );
        self.previous_diagnostic = Some(diagnostic);
    }

    /// This frame's mirror view, for the renderer; `None` draws no planar.
    /// Its `cull_vp` is cropped to the selected plane, so the renderer's
    /// mirror cull covers only what the walk does.
    pub(crate) fn camera(&self) -> Option<MirrorCamera> {
        self.camera
    }

    fn crop_enabled(&self) -> bool {
        #[cfg(any(test, feature = "capture", feature = "dev-tools"))]
        {
            self.crop
        }
        #[cfg(not(any(test, feature = "capture", feature = "dev-tools")))]
        {
            true
        }
    }

    /// Turn the crop off, or back on, for the crop-parity capture row and
    /// the diagnostics panel's crop switch: the mirror then walks and culls
    /// its whole frustum. On by default.
    #[cfg(any(test, feature = "capture", feature = "dev-tools"))]
    pub(crate) fn set_crop(&mut self, on: bool) {
        self.crop = on;
    }

    /// The selected plane this frame drew, if any.
    #[cfg(any(test, feature = "capture"))]
    pub(crate) fn plane(
        &self,
    ) -> Option<&postretro_render_cpu::planar_reflection::PlanarCandidate> {
        self.camera?;
        self.selected
            .and_then(|selected| self.candidates.planes.get(selected.candidate_index))
    }

    /// Whether the installed level holds a candidate plane: the verdict the
    /// renderer's planar target lifetime follows.
    pub(crate) fn has_candidate(&self) -> bool {
        !self.candidates.planes.is_empty()
    }

    /// How many candidate planes the installed level holds.
    #[cfg(test)]
    pub(crate) fn candidate_planes(&self) -> usize {
        self.candidates.planes.len()
    }

    /// The plane selection holds into the next frame, if any.
    #[cfg(test)]
    pub(crate) fn held_selection(&self) -> Option<SelectedPlane> {
        self.selected
    }

    /// How this frame's mirror visibility formed; `None` with no mirror.
    #[cfg(any(test, feature = "capture"))]
    pub(crate) fn path(&self) -> Option<MirrorVisibilityPath> {
        self.camera.map(|_| self.mirror.path)
    }

    /// Cells the frame draws only for the mirror: its reach less the main
    /// view's visible set. Zero with no mirror or a main view drawing all.
    #[cfg(feature = "capture")]
    pub(crate) fn mirror_only_cells(&self, main: &VisibleCells) -> usize {
        match (self.drawn_cells(main), main) {
            (VisibleCells::Culled(drawn), VisibleCells::Culled(main)) => {
                drawn.len().saturating_sub(main.len())
            }
            _ => 0,
        }
    }

    /// Force this frame's mirror walk to trip after `step_cap` portal tests.
    #[cfg(test)]
    pub(crate) fn set_step_cap(&mut self, step_cap: Option<u32>) {
        self.step_cap = step_cap;
    }

    /// The mirror's drawn cells, for the renderer's mirror cull and the
    /// animated-lightmap compose union.
    pub(crate) fn visible(&self) -> &VisibleCells {
        &self.visible
    }

    /// Non-solid cells the mirror reaches, empty ones included: the cells
    /// mirror lighting joins.
    pub(crate) fn reach(&self) -> &[u32] {
        match self.camera {
            Some(_) => &self.mirror.reach_cells,
            None => &[],
        }
    }

    /// Every cell the frame draws: the main view's, plus the mirror's reach.
    /// Mover collection, lightmap demand and SH sample regions read it; mesh
    /// collection reads the main and mirror sets apart, to tag mirror-only
    /// instances. The lightmap's path rules keep it read-free on a non-walk path:
    /// there its blocks are held, resident ones still draw, and misses count.
    pub(crate) fn drawn_cells<'a>(&'a self, main: &'a VisibleCells) -> &'a VisibleCells {
        if self.camera.is_none() || matches!(main, VisibleCells::DrawAll) {
            return main;
        }
        &self.drawn
    }

    /// The cells SH visible targeting reads, which turns every one into
    /// reads on any path: the mirror's join only on a completed mirror walk
    /// with the main view on a walk path. A tripped mirror draws only cells
    /// the main view draws, so excluding it changes nothing.
    pub(crate) fn sh_visible_cells<'a>(&'a self, main: &'a VisibleCells) -> &'a VisibleCells {
        if self.reads {
            self.drawn_cells(main)
        } else {
            main
        }
    }

    /// SH sample regions' fog cells: the main fog reach plus the mirror's,
    /// keeping the main view's empty "no portal isolation" sentinel.
    pub(crate) fn sample_fog_cells<'a>(&'a self, main: &'a [u32]) -> &'a [u32] {
        if self.camera.is_some() && !main.is_empty() {
            &self.fog
        } else {
            main
        }
    }
}

impl VisibleRenderPreparation {
    /// Tag the mirror's light reach (`rendering_pipeline.md` §7.3a, Pool
    /// rank): the bounds of the cells the mirror reaches and the main view
    /// does not go to `mirror_light_cell_aabbs`, a separate set, so lights
    /// only the mirror reaches rank after every main-view light and light only
    /// the reflection. The main view's light reach, `visible_cells` and
    /// `fog_reachable` stay exactly as without the mirror, and a cell both
    /// reach is the main view's. The unisolated sentinel (an empty fog set on
    /// a main fallback) adds nothing: every cell-assigned light is main-view
    /// already.
    pub(crate) fn include_mirror_lighting(
        &mut self,
        world: &LevelWorld,
        mirror: &PlanarRenderPreparation,
    ) {
        self.mirror_light_cell_aabbs.clear();
        if self.fog_reachable.is_empty() {
            return;
        }
        for &cell in mirror.reach() {
            let index = cell as usize;
            if self
                .light_reachable_cell_mask
                .get(index)
                .is_none_or(|&main| main)
            {
                continue;
            }
            if let Some(cell) = world.cells.get(index) {
                self.mirror_light_cell_aabbs
                    .push((cell.bounds_min, cell.bounds_max));
            }
        }
    }
}

/// The camera's reach the seeds come from: the walk's fog reach on a walk
/// path, the drawn set on a fallback (every cell when it draws all).
fn seed_domain<'a>(
    main: &'a VisibleRenderPreparation,
    world: &LevelWorld,
    all_cells: &'a mut Vec<u32>,
) -> &'a [u32] {
    if matches!(main.stats.path, VisibilityPath::PrlPortal { .. }) {
        return &main.fog_reachable;
    }
    match &main.visible_cells {
        VisibleCells::Culled(cells) => cells,
        VisibleCells::DrawAll => {
            all_cells.clear();
            all_cells.extend(0..world.cell_count() as u32);
            all_cells
        }
    }
}

/// `main ∪ extra` into `out`, ascending. A `DrawAll` main leaves `out` empty:
/// callers hand out `main` itself then.
fn union_cells(main: &VisibleCells, extra: &[u32], out: &mut VisibleCells) {
    if !matches!(out, VisibleCells::Culled(_)) {
        *out = VisibleCells::Culled(Vec::new());
    }
    let VisibleCells::Culled(out) = out else {
        return;
    };
    out.clear();
    if let VisibleCells::Culled(main) = main {
        out.extend_from_slice(main);
        out.extend_from_slice(extra);
        out.sort_unstable();
        out.dedup();
    }
}

#[cfg(test)]
#[path = "planar_test_fixtures.rs"]
pub(crate) mod test_fixtures;

#[cfg(test)]
#[path = "planar_gate_tests.rs"]
mod gate_tests;

#[cfg(test)]
#[path = "planar_tests.rs"]
mod tests;
