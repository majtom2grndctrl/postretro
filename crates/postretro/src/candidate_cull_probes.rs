// Stress-map camera probe fixtures for the candidate-cull equivalence proof.
//
// The fixture TABLE is checked-in data: each entry names a map, a camera pose,
// and the projection parameters for one probe frame. Routine `cargo test`
// reads the table (a cheap data test) but never compiles or loads the maps —
// `stress-warren`, `stress-warren-crates`, and `campaign-test` are large and
// their cold bake is ~1h (testing_guide.md "Slow / cold-bake suites"). The
// heavy test that actually compiles + loads a map and runs the CPU mirror is
// `#[ignore]` / on-demand; compact synthetic fixtures in
// `candidate_cull_mirror` cover the same equivalence contract in the routine
// suite.

#![cfg(test)]

/// How a probe expects the two camera-cull paths to relate for the frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ComparisonMode {
    /// Portal path produces `VisibleCells::Culled`; the candidate path runs and
    /// must match the tree walk on submitted leaves (the normal case).
    CandidateMatchesTreeWalk,
}

/// One camera probe over a named map. Camera origin is in engine/PRL space
/// (TrenchBroom map units, matching the player_spawn origin in the `.map`);
/// `yaw`/`pitch` follow the engine camera convention (`yaw = 0` faces -Z,
/// `render_view_matrix`). Projection is given explicitly so the heavy test
/// builds the view-projection without any game-state plumbing.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CameraProbe {
    /// Map identifier, e.g. `"stress-warren"`. The `.map` lives under
    /// `content/dev/maps/<map>.map`.
    pub map: &'static str,
    pub origin: [f32; 3],
    pub yaw_radians: f32,
    pub pitch_radians: f32,
    /// Horizontal field of view in radians.
    pub hfov_radians: f32,
    pub aspect: f32,
    pub near: f32,
    pub far: f32,
    pub mode: ComparisonMode,
}

/// The checked-in probe table. Origins are the maps' `player_spawn` positions;
/// the wall-facing yaw is the spawn `angle`. These are the three maps the plan
/// calls out for deterministic stress-map equivalence probes.
pub(crate) const PROBES: &[CameraProbe] = &[
    CameraProbe {
        map: "stress-warren",
        origin: [-3840.0, -3200.0, 96.0],
        yaw_radians: 0.0, // spawn angle 0
        pitch_radians: 0.0,
        hfov_radians: std::f32::consts::FRAC_PI_2,
        aspect: 16.0 / 9.0,
        near: 0.1,
        far: 8192.0,
        mode: ComparisonMode::CandidateMatchesTreeWalk,
    },
    CameraProbe {
        map: "stress-warren-crates",
        origin: [-2560.0, -1920.0, 96.0],
        yaw_radians: 0.0, // spawn angle 0
        pitch_radians: 0.0,
        hfov_radians: std::f32::consts::FRAC_PI_2,
        aspect: 16.0 / 9.0,
        near: 0.1,
        far: 8192.0,
        mode: ComparisonMode::CandidateMatchesTreeWalk,
    },
    CameraProbe {
        map: "campaign-test",
        origin: [1808.0, 2592.0, 72.0],
        yaw_radians: std::f32::consts::FRAC_PI_2, // spawn angle 90
        pitch_radians: 0.0,
        hfov_radians: std::f32::consts::FRAC_PI_2,
        aspect: 16.0 / 9.0,
        near: 0.1,
        far: 8192.0,
        mode: ComparisonMode::CandidateMatchesTreeWalk,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Routine, cheap: the probe table is well-formed and covers the three
    /// named maps. Compiles/loads nothing.
    #[test]
    fn probe_table_covers_the_three_stress_maps() {
        let maps: Vec<&str> = PROBES.iter().map(|p| p.map).collect();
        assert!(maps.contains(&"stress-warren"));
        assert!(maps.contains(&"stress-warren-crates"));
        assert!(maps.contains(&"campaign-test"));
        for p in PROBES {
            assert!(p.near > 0.0 && p.far > p.near, "{}: bad near/far", p.map);
            assert!(p.aspect > 0.0, "{}: bad aspect", p.map);
            assert!(p.hfov_radians > 0.0, "{}: bad fov", p.map);
        }
    }

    /// Heavy / on-demand: compile each probe's `.map`, load the `.prl`, run
    /// portal visibility from the probe camera, then assert the candidate path
    /// submits the same leaves (and identical global `bucket_ranges`) as the
    /// tree walk. `#[ignore]` because compiling these maps is a multi-minute-to
    /// -hour cold bake — never part of routine `cargo test`. Run with:
    ///   cargo test -p postretro --bin postretro -- --ignored stress_map_probes
    ///
    /// Requires `prl-build` on `PATH` (or a prebuilt `.prl` alongside the map).
    #[test]
    #[ignore = "compiles + loads large stress maps; on-demand only"]
    fn stress_map_probes_candidate_matches_tree_walk() {
        use crate::candidate_cull_mirror::{SyntheticWorld, candidate_mirror, tree_walk_mirror};
        use glam::{Mat4, Vec3};

        for probe in PROBES {
            let prl_path = compile_probe_map(probe.map);
            let world = crate::prl::load_prl(&prl_path)
                .unwrap_or_else(|e| panic!("{}: load_prl failed: {e:?}", probe.map));

            let Some(index) = world.cell_draw_index.clone() else {
                panic!("{}: loaded PRL has no CellDrawIndex section", probe.map);
            };

            let view_proj = probe_view_proj(probe);
            let position = Vec3::from_array(probe.origin);

            let mut scratch = Vec::new();
            let (vis, _frustum) = crate::visibility::determine_visible_cells(
                position, view_proj, &world, false, &mut scratch,
            );

            // The probe asserts the portal path is exercised; non-portal
            // provenance would route to the tree walk in the renderer.
            assert!(
                matches!(vis.stats.path, crate::visibility::VisibilityPath::PrlPortal { .. }),
                "{}: expected portal path, got {:?}",
                probe.map,
                vis.stats.path,
            );

            let mirror_world = SyntheticWorld::from_level_world(&world, index);
            let tree = tree_walk_mirror(&mirror_world, &vis.visible_cells, &view_proj);
            let cand = candidate_mirror(&mirror_world, &vis.visible_cells, &view_proj)
                .unwrap_or_else(|| panic!("{}: candidate path declined", probe.map));

            match probe.mode {
                ComparisonMode::CandidateMatchesTreeWalk => cand.assert_matches(&tree),
            }
        }

        // Pose → view-projection in the engine camera convention.
        fn probe_view_proj(probe: &CameraProbe) -> Mat4 {
            let look = Vec3::new(
                -probe.yaw_radians.sin() * probe.pitch_radians.cos(),
                probe.pitch_radians.sin(),
                -probe.yaw_radians.cos() * probe.pitch_radians.cos(),
            );
            let pos = Vec3::from_array(probe.origin);
            let view = Mat4::look_at_rh(pos, pos + look, Vec3::Y);
            let vfov = 2.0 * ((probe.hfov_radians / 2.0).tan() / probe.aspect).atan();
            let proj = Mat4::perspective_rh(vfov, probe.aspect, probe.near, probe.far);
            proj * view
        }
    }

    /// Compile `content/dev/maps/<map>.map` to a temp `.prl` via `prl-build`,
    /// returning the output path. On-demand helper for the `#[ignore]` probe.
    fn compile_probe_map(map: &str) -> String {
        let manifest = env!("CARGO_MANIFEST_DIR");
        let map_path = format!("{manifest}/../../content/dev/maps/{map}.map");
        let out_path = std::env::temp_dir()
            .join(format!("postretro-probe-{map}.prl"))
            .to_string_lossy()
            .into_owned();

        let status = std::process::Command::new("cargo")
            .args([
                "run",
                "--release",
                "-p",
                "postretro-level-compiler",
                "--",
                &map_path,
                "-o",
                &out_path,
                "--sh-probe-spacing",
                "10.0",
            ])
            .status()
            .unwrap_or_else(|e| panic!("{map}: failed to spawn prl-build: {e}"));
        assert!(status.success(), "{map}: prl-build failed");
        out_path
    }
}
