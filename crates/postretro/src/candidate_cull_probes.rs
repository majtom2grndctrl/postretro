// Stress-map camera probe fixtures for the candidate-cull equivalence proof.
//
// The fixture TABLE is checked-in data: each entry names a map, a camera pose,
// and the projection parameters for one probe frame. Routine `cargo test`
// reads the table (a cheap data test) but never compiles or loads the maps —
// `stress-warren`, `stress-warren-crates`, and `campaign-test` are large and
// their cold bake is ~1h (testing_guide.md "Slow / cold-bake suites"). The
// heavy test that loads a prebuilt map (or compiles a missing one) and runs the CPU mirror is
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
/// (meters, converted from the player_spawn origin in the `.map`);
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

/// The checked-in probe table. Origins are converted from authored `.map`
/// positions to engine meters; yaw follows the engine convention, with the sign
/// reversed from Quake's spawn angle. These are the four maps the plan calls out
/// for deterministic stress-map equivalence probes.
pub(crate) const PROBES: &[CameraProbe] = &[
    CameraProbe {
        map: "stress-warren",
        origin: [81.28, 2.4384, 97.536],
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
        origin: [48.768, 2.4384, 65.024],
        yaw_radians: 0.0, // spawn angle 0
        pitch_radians: 0.0,
        hfov_radians: std::f32::consts::FRAC_PI_2,
        aspect: 16.0 / 9.0,
        near: 0.1,
        far: 8192.0,
        mode: ComparisonMode::CandidateMatchesTreeWalk,
    },
    CameraProbe {
        map: "stress-warren-hallway-inspection",
        // Authored spawn (-2496, -1664, 96), engine (-qy, qz, -qx) × 0.0254.
        origin: [42.2656, 2.4384, 63.3984],
        yaw_radians: 0.0,
        pitch_radians: 0.0,
        hfov_radians: std::f32::consts::FRAC_PI_2,
        aspect: 16.0 / 9.0,
        near: 0.1,
        far: 8192.0,
        mode: ComparisonMode::CandidateMatchesTreeWalk,
    },
    CameraProbe {
        map: "campaign-test",
        origin: [-65.8368, 1.8288, -45.9232],
        yaw_radians: -std::f32::consts::FRAC_PI_2, // Quake angle 90 → engine yaw -90
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

    /// Routine, cheap: the probe table is well-formed and covers the four
    /// named maps. Compiles/loads nothing.
    #[test]
    fn probe_table_covers_the_four_stress_maps() {
        let maps: Vec<&str> = PROBES.iter().map(|p| p.map).collect();
        assert!(maps.contains(&"stress-warren"));
        assert!(maps.contains(&"stress-warren-crates"));
        assert!(maps.contains(&"campaign-test"));
        assert!(maps.contains(&"stress-warren-hallway-inspection"));
        for p in PROBES {
            assert!(p.near > 0.0 && p.far > p.near, "{}: bad near/far", p.map);
            assert!(p.aspect > 0.0, "{}: bad aspect", p.map);
            assert!(p.hfov_radians > 0.0, "{}: bad fov", p.map);
        }
    }

    /// Heavy / on-demand: load each probe PRL, compiling only missing maps,
    /// then check cull parity and exact visible-span coverage from the camera.
    /// `#[ignore]` because missing maps require expensive bakes. Run with:
    ///   cargo test -p postretro --bin postretro -- --ignored stress_map_probes
    ///
    /// Reuses a prebuilt `.prl` alongside the map, or compiles with Cargo if
    /// absent. Set `POSTRETRO_PROBE_MAPS=campaign-test,stress-warren-hallway-inspection`
    /// to run only those prebuilt poses; omit it to prove all four maps.
    /// Add `--nocapture` to see paths, total leaves, runs and slots per pass.
    #[test]
    #[ignore = "compiles + loads large stress maps; on-demand only"]
    fn stress_map_probes_candidate_matches_tree_walk() {
        use crate::candidate_cull_mirror::{SyntheticWorld, candidate_mirror, tree_walk_mirror};
        use glam::{Mat4, Vec3};

        let selected = std::env::var("POSTRETRO_PROBE_MAPS").ok();
        let mut probed = 0;
        for probe in PROBES {
            if selected
                .as_ref()
                .is_some_and(|maps| !maps.split(',').any(|map| map == probe.map))
            {
                continue;
            }
            probed += 1;
            let prl_path = compile_probe_map(probe.map);
            let world = postretro_level_loader::load_prl(&prl_path)
                .unwrap_or_else(|e| panic!("{}: load_prl failed: {e:?}", probe.map));

            let Some(index) = world.cell_draw_index.clone() else {
                panic!("{}: loaded PRL has no CellDrawIndex section", probe.map);
            };

            let view_proj = probe_view_proj(probe);
            let position = Vec3::from_array(probe.origin);

            let mut scratch = Vec::new();
            let (vis, _frustum) = postretro_visibility::determine_visible_cells(
                position,
                view_proj,
                &world,
                &[],
                false,
                &mut scratch,
                postretro_visibility::TimingGate::OFF,
            );

            // Every concrete drawable set must restrict draws to its spans,
            // including over-budget portal walks and tree-walk fallbacks.
            assert!(
                matches!(
                    vis.visible_cells,
                    postretro_visibility::VisibleCells::Culled(_)
                ),
                "{}: probe produced no concrete visible set",
                probe.map
            );

            let mirror_world = SyntheticWorld::from_level_world(&world, index);
            let tree = tree_walk_mirror(&mirror_world, &vis.visible_cells, &view_proj);
            let (runs, slots) =
                tree.assert_visible_span_coverage(&mirror_world, &vis.visible_cells);
            if postretro_renderer::visibility_path_uses_candidate_cull(vis.stats.path) {
                let cand = candidate_mirror(&mirror_world, &vis.visible_cells, &view_proj)
                    .unwrap_or_else(|| panic!("{}: candidate path declined", probe.map));
                match probe.mode {
                    ComparisonMode::CandidateMatchesTreeWalk => cand.assert_matches(&tree),
                }
                assert_eq!(
                    cand.assert_visible_span_coverage(&mirror_world, &vis.visible_cells),
                    (runs, slots)
                );
            }
            println!(
                "{}: prl={} pose={:?} yaw={} pitch={} path={:?} total_leaves={} coalesced_runs_per_camera_pass={} drawn_slots_per_camera_pass={} submitted_leaves={}",
                probe.map,
                prl_path,
                probe.origin,
                probe.yaw_radians.to_degrees(),
                probe.pitch_radians.to_degrees(),
                vis.stats.path,
                mirror_world.leaves.len(),
                runs,
                slots,
                tree.submitted.len()
            );
        }
        assert!(
            probed > 0,
            "POSTRETRO_PROBE_MAPS selected no known probe maps"
        );

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

    /// On-demand search for CPU-timing walk-reach probes on the stress maps
    /// (brief: cpu-frame-profiling, parallelization gate). Sweeps every open
    /// drawable cell at pawn height with eight headings, and ranks poses by
    /// portals the walk considered — the cost the gate measures. Reads each
    /// map's already-compiled `.prl` beside its `.map`, skipping any that is
    /// missing or stale. Run with:
    ///   cargo test -p postretro --bin postretro -- --ignored walk_reach_probe_search --nocapture
    #[test]
    #[ignore = "loads the compiled stress-warren PRL; on-demand only"]
    fn walk_reach_probe_search() {
        let mut searched = 0;
        for map in [
            "stress-warren",
            "stress-warren-mini",
            "stress-warren-hallway-inspection",
        ] {
            let prl = format!(
                "{}/../../content/dev/maps/{map}.prl",
                env!("CARGO_MANIFEST_DIR")
            );
            match postretro_level_loader::load_prl(&prl) {
                Ok(world) => {
                    println!("\n== {map}");
                    search_map(&world);
                    searched += 1;
                }
                Err(err) => println!("\n== {map}: skipped, {err}"),
            }
        }
        assert!(searched > 0, "compile at least one stress map first");
    }

    fn search_map(world: &postretro_level_loader::LevelWorld) {
        use glam::{Mat4, Vec3};
        use postretro_visibility::{TimingGate, VisibilityPath, VisibilityStage};

        // Dev player capsule: 0.8 m half-height, eye 0.5 m above the origin.
        const HALF_HEIGHT: f32 = 0.8;
        const EYE_ABOVE_ORIGIN: f32 = 0.5;
        let aspect = 16.0 / 9.0;
        let vfov = 2.0 * ((std::f32::consts::FRAC_PI_4).tan() / aspect).atan();
        let proj = Mat4::perspective_rh(vfov, aspect, 0.1, 4096.0);

        let mut results = Vec::new();
        for cell in world
            .cells
            .iter()
            .filter(|c| !c.is_solid && !c.is_exterior && c.is_drawable)
        {
            let center = (cell.bounds_min + cell.bounds_max) * 0.5;
            let origin = Vec3::new(center.x, cell.bounds_min.y + HALF_HEIGHT, center.z);
            let eye = origin + Vec3::Y * EYE_ABOVE_ORIGIN;
            for step in 0..8 {
                let yaw = step as f32 * std::f32::consts::FRAC_PI_4;
                let look = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
                let view = Mat4::look_at_rh(eye, eye + look, Vec3::Y);
                let mut scratch = Vec::new();
                let (vis, _) = postretro_visibility::determine_visible_cells(
                    eye,
                    proj * view,
                    world,
                    &[],
                    false,
                    &mut scratch,
                    TimingGate::ON,
                );
                let considered = vis
                    .stats
                    .cpu
                    .value(VisibilityStage::Considered)
                    .unwrap_or(0);
                let step_limit = matches!(
                    vis.stats.path,
                    VisibilityPath::PortalStepLimitFallback { .. }
                );
                results.push((considered, vis.stats.walk_reach(), step_limit, origin, yaw));
            }
        }
        results.sort_by_key(|r| std::cmp::Reverse(r.0));
        println!("considered | walk_reach | step_limit | --start-pose x,y,z,yaw_deg,0");
        for (considered, reach, step_limit, origin, yaw) in results.iter().take(12) {
            println!(
                "{considered:>10} | {reach:>10?} | {step_limit:>10} | {:.2},{:.2},{:.2},{:.0},0",
                origin.x,
                origin.y,
                origin.z,
                yaw.to_degrees()
            );
        }
        assert!(!results.is_empty(), "no open drawable cell to probe");
    }

    /// Prefer the prebuilt PRL; compile a missing map to a temporary PRL.
    /// Loading validation rejects stale or invalid prebuilt draw indexes.
    fn compile_probe_map(map: &str) -> String {
        let manifest = env!("CARGO_MANIFEST_DIR");
        let map_path = format!("{manifest}/../../content/dev/maps/{map}.map");
        let prebuilt = format!("{manifest}/../../content/dev/maps/{map}.prl");
        if std::path::Path::new(&prebuilt).is_file() {
            // The caller loads and validates the complete PRL, including the
            // required CellDrawIndex, before using any of its ranges.
            return prebuilt;
        }
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
                "--lightmap-density",
                "0.5",
            ])
            .status()
            .unwrap_or_else(|e| panic!("{map}: failed to spawn prl-build: {e}"));
        assert!(status.success(), "{map}: prl-build failed");
        out_path
    }
}
