//! Planar mode for the residency walks: the play frame's mirror step per frame.
//! See: context/lib/rendering_pipeline.md §4, §7.3a

// `POSTRETRO_LIGHTMAP_WALK_PLANAR=1` runs, after each frame's camera walk,
// the app's own `PlanarRenderPreparation` at the very high tier's preset:
// selection over the level's candidate planes, the cropped mirror camera
// from the walk's eye, and the mirror's seeded walk on its own scratch. The
// frame then streams main visible ∪ mirror reach and SH's gated set, exactly
// the sets the play frame hands `StreamingFrame`, so the lightmap's §4 path
// rules decide reads as in play. Light ranking is not modelled: it reads no
// lightmap block. Off, the run is the plain camera walk.

use std::ops::Range;

use postretro_level_loader::LevelWorld;
use postretro_render_cpu::fluid_surface::reflection::volume_has_bound_face;
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_render_cpu::planar_reflection::{PlanarPreset, planar_preset};
use postretro_visibility::{MirrorVisibilityPath, VisibilityPath, VisibleCells};

use crate::camera::RenderCamera;
use crate::render_preparation::{PlanarRenderPreparation, VisibleRenderPreparation};

/// The play frame's planar mirror step, held across one run's frames so
/// selection keeps its hysteresis, as in play.
pub(super) struct WalkMirror {
    render: PlanarRenderPreparation,
    preset: PlanarPreset,
    /// This frame's mirror-only cells; reused.
    mirror_only: Vec<u32>,
    /// Per cell: mirror-only on some frame of this run.
    seen: Vec<bool>,
}

impl WalkMirror {
    /// Install `world`'s candidate planes as level install does, with every
    /// fluid volume that has a probe-bound face reflecting: the walk loads no
    /// manifest, so it takes capture's rule, the widest the app can select.
    pub(super) fn install(world: &LevelWorld) -> Self {
        let reflecting: Vec<u32> = world
            .environment_volumes
            .volumes
            .iter()
            .enumerate()
            .filter(|(index, volume)| {
                !volume.fluid.is_empty()
                    && volume_has_bound_face(world.reflection_probes.as_ref(), *index)
            })
            .map(|(index, _)| index as u32)
            .collect();
        let mut render = PlanarRenderPreparation::default();
        render.install(world, &reflecting);
        Self {
            render,
            preset: planar_preset(FluidReflectionQuality::VeryHigh)
                .expect("very high runs the planar layer"),
            mirror_only: Vec::new(),
            seen: vec![false; world.cells.len()],
        }
    }

    pub(super) fn candidate_planes(&self) -> usize {
        self.render.candidate_planes()
    }

    /// O1 after the camera walk (`main`): select, crop, seed and walk the
    /// mirror, then tally this frame into `stats`.
    pub(super) fn prepare(
        &mut self,
        world: &LevelWorld,
        camera: &RenderCamera,
        main: &VisibleRenderPreparation,
        cell_blocks: &[Range<u32>],
        stats: &mut MirrorStats,
    ) {
        self.render
            .prepare(world, camera, main, Some(self.preset), &[], &main.stats.cpu);
        stats.frames += 1;
        stats.held_frames += u64::from(self.render.held_selection().is_some());
        let Some(path) = self.render.path() else {
            return;
        };
        stats.selected_frames += 1;
        match path {
            MirrorVisibilityPath::Walked => {
                stats.walked += 1;
                stats.read_frames +=
                    u64::from(matches!(main.stats.path, VisibilityPath::PrlPortal { .. }));
            }
            MirrorVisibilityPath::Tripped { .. } => stats.tripped += 1,
            MirrorVisibilityPath::NoSeeds | MirrorVisibilityPath::NoPortals => stats.other += 1,
        }
        mirror_only_cells(
            &main.visible_cells,
            self.render.drawn_cells(&main.visible_cells),
            &mut self.mirror_only,
        );
        let count = self.mirror_only.len() as u32;
        stats.mirror_only_cell_frames += u64::from(count);
        stats.frames_with_mirror_only += u64::from(count > 0);
        stats.max_mirror_only_cells = stats.max_mirror_only_cells.max(count);
        for &cell in &self.mirror_only {
            let blocks = cell_blocks[cell as usize].len() as u64;
            stats.mirror_only_block_frames += blocks;
            let seen = &mut self.seen[cell as usize];
            if !*seen {
                *seen = true;
                stats.distinct_mirror_only_cells += 1;
            }
        }
    }

    /// Every cell the frame draws, as the play frame streams it.
    pub(super) fn drawn_cells<'a>(&'a self, main: &'a VisibleCells) -> &'a VisibleCells {
        self.render.drawn_cells(main)
    }

    /// The cells SH visible targeting reads, as the play frame gates it.
    pub(super) fn sh_visible_cells<'a>(&'a self, main: &'a VisibleCells) -> &'a VisibleCells {
        self.render.sh_visible_cells(main)
    }
}

/// One run's mirror, per frame of the walk.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct MirrorStats {
    pub(super) frames: u64,
    /// Frames ending with a plane selected, a mirror view or not (an empty
    /// crop or no seed in the cropped frustum holds selection, drawing none).
    pub(super) held_frames: u64,
    /// Frames with a selected plane and a mirror view.
    pub(super) selected_frames: u64,
    pub(super) walked: u64,
    pub(super) tripped: u64,
    pub(super) other: u64,
    /// Completed mirror walks with the main view on a walk path: the frames
    /// whose mirror cells may add reads.
    pub(super) read_frames: u64,
    /// Mirror-only cells (mirror reach the main view does not draw), summed
    /// over frames.
    pub(super) mirror_only_cell_frames: u64,
    pub(super) frames_with_mirror_only: u64,
    pub(super) max_mirror_only_cells: u32,
    /// Cells mirror-only on at least one frame of the run.
    pub(super) distinct_mirror_only_cells: u64,
    /// Lightmap blocks of the mirror-only cells, summed over frames.
    pub(super) mirror_only_block_frames: u64,
}

impl MirrorStats {
    pub(super) fn line(&self) -> String {
        format!(
            "{} frames: selected plane with a mirror view {} frames ({} held selection; mirror \
             walked {}, tripped {}, other {}; reads allowed {}); mirror-only cells {} \
             cell-frames in {} frames (max {}/frame, {} distinct cells), {} block-frames",
            self.frames,
            self.selected_frames,
            self.held_frames,
            self.walked,
            self.tripped,
            self.other,
            self.read_frames,
            self.mirror_only_cell_frames,
            self.frames_with_mirror_only,
            self.max_mirror_only_cells,
            self.distinct_mirror_only_cells,
            self.mirror_only_block_frames,
        )
    }
}

/// `drawn` less `main` into `out`, ascending: the cells the frame draws only
/// for the mirror. Both sets are ascending and `drawn` holds `main`; a
/// `DrawAll` main or drawn set leaves `out` empty.
fn mirror_only_cells(main: &VisibleCells, drawn: &VisibleCells, out: &mut Vec<u32>) {
    out.clear();
    let (VisibleCells::Culled(main), VisibleCells::Culled(drawn)) = (main, drawn) else {
        return;
    };
    let mut main = main.iter().peekable();
    for &cell in drawn {
        while main.next_if(|&&kept| kept < cell).is_some() {}
        if main.next_if_eq(&&cell).is_none() {
            out.push(cell);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The mirror-only count is the mirror's reach the main view does not
    // draw: cells both draw count once, as the main view's.
    #[test]
    fn mirror_only_cells_keep_only_cells_the_main_view_does_not_draw() {
        let main = VisibleCells::Culled(vec![1, 3, 5]);
        let drawn = VisibleCells::Culled(vec![0, 1, 2, 3, 5, 8]);
        let mut out = vec![99];
        mirror_only_cells(&main, &drawn, &mut out);
        assert_eq!(out, [0, 2, 8]);

        mirror_only_cells(&main, &main, &mut out);
        assert!(out.is_empty(), "no mirror: nothing mirror-only");

        mirror_only_cells(&VisibleCells::DrawAll, &VisibleCells::DrawAll, &mut out);
        assert!(out.is_empty(), "a main view drawing all leaves none");
    }
}
