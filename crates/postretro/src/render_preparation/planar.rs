// App-owned mirror selection, seeded visibility, and shared resource demand.
// See: context/plans/in-progress/planar-water-spike-contract.md

use postretro_level_loader::LevelWorld;
use postretro_render_cpu::planar_reflection::{
    MirrorCamera, PlanarCandidate, PlanarCandidates, PlanarReflectionSettings, SelectedPlane,
    collect_mirror_seed_cells, select_plane,
};
use postretro_stage_timing::{StageFrame, TimingGate};
use postretro_visibility::{
    MirrorVisibilityPath, VisibilityStage, VisibleCells, determine_mirror_visible_cells,
};

use super::VisibleRenderPreparation;
use crate::camera::RenderCamera;

pub(crate) struct PlanarRenderPreparation {
    candidates: PlanarCandidates,
    selected: Option<SelectedPlane>,
    camera: Option<MirrorCamera>,
    seeds: Vec<u32>,
    scratch: Vec<u32>,
    visible: VisibleCells,
    collected: VisibleCells,
    reachable: Vec<u32>,
    fog_union: Vec<u32>,
    previous_diagnostic: Option<(usize, usize, usize, MirrorVisibilityPath)>,
}

impl Default for PlanarRenderPreparation {
    fn default() -> Self {
        Self {
            candidates: PlanarCandidates::default(),
            selected: None,
            camera: None,
            seeds: Vec::new(),
            scratch: Vec::new(),
            visible: VisibleCells::Culled(Vec::new()),
            collected: VisibleCells::Culled(Vec::new()),
            reachable: Vec::new(),
            fog_union: Vec::new(),
            previous_diagnostic: None,
        }
    }
}

impl PlanarRenderPreparation {
    pub(crate) fn install(&mut self, world: &LevelWorld, reflecting_volumes: &[u32]) {
        *self = Self {
            candidates: PlanarCandidates::from_volumes(
                &world.environment_volumes.volumes,
                reflecting_volumes,
            ),
            ..Self::default()
        };
    }

    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare(
        &mut self,
        world: &LevelWorld,
        main_camera: RenderCamera,
        main: &VisibleRenderPreparation,
        settings: PlanarReflectionSettings,
        blocked_portals: &[bool],
        timing: TimingGate,
    ) -> StageFrame<VisibilityStage> {
        self.camera = None;
        if let VisibleCells::Culled(cells) = &mut self.visible {
            cells.clear();
        }
        self.reachable.clear();
        if !settings.on {
            self.selected = None;
            self.previous_diagnostic = None;
            return StageFrame::new(timing);
        }
        self.selected = select_plane(
            &self.candidates,
            main_camera.eye_position,
            main_camera.view_projection,
            self.selected,
        );
        let Some(selected) = self.selected else {
            self.previous_diagnostic = None;
            return StageFrame::new(timing);
        };
        let projection = main_camera.view_projection * main_camera.view_matrix.inverse();
        let Some(camera) = MirrorCamera::new(
            main_camera.eye_position,
            main_camera.view_matrix,
            projection,
            selected.h,
            settings.d,
        ) else {
            self.selected = None;
            return StageFrame::new(timing);
        };
        // Main fallback's empty fog set means no portal isolation. Its drawable
        // set omits empty cells, so derive the full seed domain in that case.
        if main.fog_reachable.is_empty() {
            self.fog_union.clear();
            let frustum = postretro_visibility::extract_frustum_planes(main_camera.view_projection);
            self.fog_union
                .extend(world.cells.iter().enumerate().filter_map(|(id, cell)| {
                    frustum
                        .planes
                        .iter()
                        .all(|plane| {
                            let positive = glam::Vec3::new(
                                if plane.normal.x >= 0.0 {
                                    cell.bounds_max.x
                                } else {
                                    cell.bounds_min.x
                                },
                                if plane.normal.y >= 0.0 {
                                    cell.bounds_max.y
                                } else {
                                    cell.bounds_min.y
                                },
                                if plane.normal.z >= 0.0 {
                                    cell.bounds_max.z
                                } else {
                                    cell.bounds_min.z
                                },
                            );
                            plane.normal.dot(positive) + plane.dist >= 0.0
                        })
                        .then_some(id as u32)
                }));
        }
        let main_reachable = if main.fog_reachable.is_empty() {
            self.fog_union.as_slice()
        } else {
            &main.fog_reachable
        };
        collect_mirror_seed_cells(
            main_reachable,
            world,
            &selected,
            main_camera.eye_position,
            settings.r,
            &mut self.seeds,
        );
        let (result, _) = determine_mirror_visible_cells(
            camera.eye,
            camera.cull_vp,
            camera.clip_plane,
            &self.seeds,
            world,
            blocked_portals,
            &mut self.scratch,
            timing,
        );
        if let VisibleCells::Culled(cells) = result.visible_cells {
            let previous = std::mem::replace(&mut self.visible, VisibleCells::Culled(cells));
            if let VisibleCells::Culled(cells) = previous {
                self.scratch = cells;
            }
        }
        self.reachable = result.reachable_cells;
        self.camera = Some(camera);
        merge_cells(&main.visible_cells, &self.reachable, &mut self.collected);
        self.fog_union.clear();
        if !main.fog_reachable.is_empty() {
            self.fog_union.extend_from_slice(&main.fog_reachable);
            self.fog_union.extend_from_slice(&self.reachable);
            self.fog_union.sort_unstable();
            self.fog_union.dedup();
        }
        let mirror_cells = match &self.visible {
            VisibleCells::Culled(cells) => cells.as_slice(),
            VisibleCells::DrawAll => &[],
        };
        if let VisibleCells::Culled(cells) = &main.visible_cells {
            self.scratch.clone_from(cells);
            self.scratch.sort_unstable();
        }
        let mirror_only = mirror_cells
            .iter()
            .filter(|id| match &main.visible_cells {
                VisibleCells::DrawAll => false,
                VisibleCells::Culled(_) => self.scratch.binary_search(id).is_err(),
            })
            .count();
        let visible_count = mirror_cells.len();
        let diagnostic = (
            selected.candidate_index,
            visible_count,
            mirror_only,
            result.path,
        );
        if self.previous_diagnostic != Some(diagnostic) {
            log::info!(
                "[Planar reflection] plane={:.3} seeds={} visible={} reachable={} mirror-only={} path={:?} considered={} accepted={}",
                selected.h,
                self.seeds.len(),
                visible_count,
                self.reachable.len(),
                mirror_only,
                result.path,
                result.considered,
                result.accepted,
            );
            self.previous_diagnostic = Some(diagnostic);
        }
        result.cpu
    }

    pub(crate) fn camera(&self) -> Option<MirrorCamera> {
        self.camera
    }

    pub(crate) fn plane(&self) -> Option<&PlanarCandidate> {
        self.selected
            .and_then(|selected| self.candidates.planes.get(selected.candidate_index))
    }

    pub(crate) fn visible(&self) -> &VisibleCells {
        &self.visible
    }

    pub(crate) fn additional_cells(&self) -> Option<&VisibleCells> {
        self.camera
            .filter(|_| matches!(&self.visible, VisibleCells::Culled(cells) if !cells.is_empty()))
            .map(|_| &self.visible)
    }

    /// Include empty mirror cells: an entity can occupy a cell with no world faces.
    pub(crate) fn collected_cells<'a>(&'a self, main: &'a VisibleCells) -> &'a VisibleCells {
        if self.camera.is_none() || matches!(main, VisibleCells::DrawAll) {
            return main;
        }
        &self.collected
    }

    pub(crate) fn fog_cells<'a>(&'a self, main: &'a [u32]) -> &'a [u32] {
        if self.camera.is_some() && !main.is_empty() {
            &self.fog_union
        } else {
            main
        }
    }
}

fn merge_cells(main: &VisibleCells, mirror: &[u32], out: &mut VisibleCells) {
    if !matches!(out, VisibleCells::Culled(_)) {
        *out = VisibleCells::Culled(Vec::new());
    }
    let VisibleCells::Culled(out) = out else {
        unreachable!()
    };
    out.clear();
    if let VisibleCells::Culled(main) = main {
        out.extend_from_slice(main);
        out.extend_from_slice(mirror);
        out.sort_unstable();
        out.dedup();
    }
}

#[cfg(test)]
#[path = "planar_tests.rs"]
mod tests;
