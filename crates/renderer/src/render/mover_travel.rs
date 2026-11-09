// Mover travel: the per-frame blend input game hands the renderer for
// each present mover, and the renderer-owned table that holds it.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting)

/// One present mover's travel state for a frame: its authored id and its
/// travel fraction after the author ramp, in `[0, 1]`. Zero reads the
/// mover's first-waypoint lighting, one its last-waypoint lighting.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoverTravel {
    pub mover_id: u32,
    pub t: f32,
}

/// The latest frame's mover travel states, renderer-owned and reused across
/// frames. Holds only states handed over while a level is installed; level
/// install and release empty it, so no state reaches another level.
#[derive(Debug, Default)]
pub(in crate::render) struct MoverTravelTable {
    states: Vec<MoverTravel>,
}

impl MoverTravelTable {
    /// Replace the held states with this frame's, reusing the buffer.
    pub(in crate::render) fn replace(&mut self, states: &[MoverTravel]) {
        self.states.clear();
        self.states.extend_from_slice(states);
    }

    pub(in crate::render) fn clear(&mut self) {
        self.states.clear();
    }

    /// The held states; a mover absent here has no state and composes ungated.
    pub(in crate::render) fn states(&self) -> &[MoverTravel] {
        &self.states
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::Renderer;
    use crate::render::gpu_test_harness::offscreen_renderer_or_skip;

    fn state(mover_id: u32, t: f32) -> MoverTravel {
        MoverTravel { mover_id, t }
    }

    #[test]
    fn mover_travel_table_holds_only_the_latest_frame() {
        let mut table = MoverTravelTable::default();
        table.replace(&[state(0, 0.25), state(4, 1.0)]);
        table.replace(&[state(4, 0.5)]);
        assert_eq!(table.states(), &[state(4, 0.5)]);
        table.clear();
        assert!(table.states().is_empty());
    }

    fn held(renderer: &Renderer) -> Vec<MoverTravel> {
        renderer.full().mover_travel.states().to_vec()
    }

    /// Renderer half: a hand-over with no level is dropped, and
    /// install empties the table, so no level composes with another level's
    /// (or a pre-install) state. Release empties it on the same reset.
    #[test]
    fn mover_travel_is_held_only_while_a_level_is_installed_and_reset_at_install_and_release() {
        let test =
            "mover_travel_is_held_only_while_a_level_is_installed_and_reset_at_install_and_release";
        let Some(mut renderer) = offscreen_renderer_or_skip(test, 64, 64) else {
            return;
        };
        assert!(!renderer.has_level_resources());
        renderer.set_mover_travel(&[state(0, 0.75)]);
        assert!(
            held(&renderer).is_empty(),
            "no level: the hand-over is dropped"
        );

        renderer.install_empty_level_for_test();
        assert!(
            held(&renderer).is_empty(),
            "install never inherits a pre-install hand-over"
        );
        renderer.set_mover_travel(&[state(0, 0.75), state(2, 0.0)]);
        assert_eq!(held(&renderer), vec![state(0, 0.75), state(2, 0.0)]);
        renderer.set_mover_travel(&[state(0, 1.0)]);
        assert_eq!(
            held(&renderer),
            vec![state(0, 1.0)],
            "each frame replaces the last"
        );

        // Relevel reusing mover id 0: the new level starts with no state.
        renderer.install_empty_level_for_test();
        assert!(
            held(&renderer).is_empty(),
            "install resets the previous level's state"
        );

        // Release to the no-level Frontend: the table empties, and a later
        // hand-over with no level is dropped again.
        renderer.set_mover_travel(&[state(0, 0.5)]);
        assert_eq!(held(&renderer), vec![state(0, 0.5)]);
        renderer.release_level_resources();
        assert!(!renderer.has_level_resources());
        assert!(
            held(&renderer).is_empty(),
            "release resets the level's state"
        );
        renderer.set_mover_travel(&[state(0, 0.5)]);
        assert!(
            held(&renderer).is_empty(),
            "no level after release: the hand-over is dropped"
        );
    }
}
