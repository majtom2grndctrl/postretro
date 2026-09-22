// Synchronous offscreen capture of world geometry and authored receivers.
// See: context/lib/rendering_pipeline.md §7.8

#[cfg(test)]
use std::collections::HashSet;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
#[cfg(test)]
use glam::{Mat4, Vec3};
use image::ImageEncoder as _;
#[cfg(test)]
use postretro_entities::ComponentKind;
#[cfg(test)]
use postretro_entities::EntityRegistry;

use super::prepared::PreparedCapture;
use super::report::measurement_report;
use super::scene::parse_scene;
#[cfg(test)]
use super::scene::{CameraPose, ForcedAnimLight, ForcedAnimatedPromotion};
#[cfg(test)]
use super::setup::{
    capture_static_lights_and_shadow_selection, capture_view_projection,
    resolve_forced_animated_promotion_rows,
};

#[cfg(test)]
use super::prepared::{
    capture_mesh_models, collect_capture_receiver_draws, spawn_capture_receiver_registry,
};
#[cfg(test)]
use crate::runtime_movers::KinematicMoverRenderCollector;
#[cfg(test)]
use crate::scripting_systems::mesh_render::MeshRenderCollector;
#[cfg(test)]
use postretro_entities::ComponentValue;
#[cfg(test)]
use postretro_visibility::VisibleCells;
const MAX_UNIQUE_FILE_ATTEMPTS: usize = 1024;
static NEXT_UNIQUE_FILE_ID: AtomicU64 = AtomicU64::new(0);

/// Entry point wired from `startup::build_session`. Capture performs all work
/// synchronously and always terminates, preventing winit startup on both success
/// and failure.
pub(crate) fn run_capture(scene_arg: Option<&str>) -> ! {
    match run_capture_inner(scene_arg) {
        Ok(()) => std::process::exit(0),
        Err(err) => {
            eprintln!("[Capture] {err:#}");
            std::process::exit(1);
        }
    }
}

fn run_capture_inner(scene_arg: Option<&str>) -> Result<()> {
    let scene_arg =
        scene_arg.ok_or_else(|| anyhow!("`--capture` requires a scene JSON path argument"))?;
    let scene_path = Path::new(scene_arg);
    let text = fs::read_to_string(scene_path)
        .with_context(|| format!("failed to read capture scene `{scene_arg}`"))?;
    let scene = parse_scene(&text)?;

    let map_path = Path::new(&scene.map);
    if !map_path.is_file() {
        bail!("map not found: `{}`", scene.map);
    }
    let output_path = Path::new(&scene.output);
    reject_output_source_aliases(output_path, map_path, scene_path)?;
    preflight_output_path(output_path)?;

    let report_path = scene
        .measurement
        .as_ref()
        .map(|measurement| {
            let report_path = Path::new(&measurement.report);
            reject_output_source_aliases(report_path, map_path, scene_path)?;
            reject_path_alias(report_path, output_path, "capture PNG output")?;
            preflight_output_path(report_path)?;
            Ok::<_, anyhow::Error>(report_path)
        })
        .transpose()?;

    let mut prepared = PreparedCapture::prepare(&scene)?;
    let [width, height] = prepared.resolution();

    if let Some(measurement) = &scene.measurement {
        let map_bytes = fs::metadata(map_path)
            .with_context(|| format!("failed to inspect capture map `{}`", map_path.display()))?
            .len();
        for _ in 0..measurement.warmup_frames {
            let _ = prepared.capture_measurement_frame()?;
        }
        // Warmup may have completed a full timing window. It belongs to setup,
        // never to sample statistics or report output.
        prepared.reset_measurement_timing();

        let mut cpu_samples_ms = Vec::with_capacity(measurement.sample_frames as usize);
        let mut gpu_windows = Vec::new();
        for _ in 0..measurement.sample_frames {
            let sample_start = Instant::now();
            if let Some(window) = prepared.capture_measurement_frame()? {
                gpu_windows.push(window);
            }
            cpu_samples_ms.push(sample_start.elapsed().as_secs_f64() * 1000.0);
        }

        let report = measurement_report(
            &scene,
            map_bytes,
            capture_git_revision(),
            prepared.measurement_adapter_identity(),
            prepared.sh_residency_report(),
            cpu_samples_ms,
            prepared.measurement_timing_state(),
            gpu_windows,
        );
        let staged_report = stage_measurement_report(
            report_path.expect("measurement report path was preflighted"),
            &report,
        )?;

        // The only readback is after all timed work. `scene_color` readback is
        // already RGBA8 sRGB; PNG publication remains the legacy atomic path.
        let rgba = prepared.capture_frame()?;
        write_capture_png(output_path, &rgba, width, height)?;
        publish_staged_measurement_report(
            staged_report,
            report_path.expect("measurement report path was preflighted"),
        )?;
    } else {
        let rgba = prepared.capture_frame()?;

        // `scene_color` readback is already RGBA8 sRGB. Write only after all
        // rendering succeeded, so invalid input or GPU failures never touch output.
        write_capture_png(output_path, &rgba, width, height)?;
    }

    Ok(())
}

/// Reject an output that resolves to either capture input. Existing paths are
/// canonicalized, so relative, absolute, and symlink spellings compare by the
/// file they name rather than by their source text.
fn reject_output_source_aliases(output: &Path, map: &Path, scene: &Path) -> Result<()> {
    for (label, source) in [("map", map), ("scene JSON", scene)] {
        reject_path_alias(output, source, &format!("capture {label}"))?;
    }
    Ok(())
}

fn reject_path_alias(output: &Path, source: &Path, source_label: &str) -> Result<()> {
    if path_alias_key(output)? == path_alias_key(source)? {
        bail!(
            "output path must not alias the {source_label}: `{}`",
            output.display()
        );
    }
    Ok(())
}

fn path_alias_key(path: &Path) -> Result<PathBuf> {
    match fs::canonicalize(path) {
        Ok(canonical) => Ok(canonical),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let absolute = if path.is_absolute() {
                path.to_owned()
            } else {
                std::env::current_dir()
                    .context("failed to resolve current directory for capture paths")?
                    .join(path)
            };
            let mut normalized = PathBuf::new();
            for component in absolute.components() {
                match component {
                    std::path::Component::CurDir => {}
                    std::path::Component::ParentDir => {
                        normalized.pop();
                    }
                    other => normalized.push(other.as_os_str()),
                }
            }
            // A missing intermediate component (for example, `nested/..`) can
            // make the initial canonicalization fail even when the normalized
            // path names an existing source. Canonicalize that result when
            // possible so it compares consistently with source paths.
            match fs::canonicalize(&normalized) {
                Ok(canonical) => Ok(canonical),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(normalized),
                Err(err) => {
                    Err(err).with_context(|| format!("failed to resolve `{}`", path.display()))
                }
            }
        }
        Err(err) => Err(err).with_context(|| format!("failed to resolve `{}`", path.display())),
    }
}

/// Verify the atomic publication operations before renderer initialization.
/// Existing targets must be regular files; the parent must support sibling
/// creation and replacement rename.
fn preflight_output_path(output: &Path) -> Result<()> {
    if output.as_os_str().is_empty() {
        bail!("output path is empty");
    }
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent_metadata = fs::metadata(parent)
        .with_context(|| format!("output parent does not exist: `{}`", parent.display()))?;
    if !parent_metadata.is_dir() {
        bail!("output parent is not a directory: `{}`", parent.display());
    }

    let output_exists = match fs::symlink_metadata(output) {
        Ok(metadata) if !metadata.file_type().is_file() => {
            bail!("output path is not a regular file: `{}`", output.display());
        }
        Ok(_) => true,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
        Err(err) => {
            return Err(err)
                .with_context(|| format!("failed to inspect output `{}`", output.display()));
        }
    };
    probe_atomic_publication(parent, output, output_exists)?;
    Ok(())
}

fn probe_atomic_publication(parent: &Path, output: &Path, replacing: bool) -> Result<()> {
    let mut source = create_unique_sibling_file(parent, "write-probe-source")
        .with_context(|| format!("output parent is not writable: `{}`", parent.display()))?;
    let mut destination = create_unique_sibling_file(parent, "write-probe-destination")
        .with_context(|| format!("output parent is not writable: `{}`", parent.display()))?;
    source
        .file_mut()
        .write_all(&[0])
        .with_context(|| format!("output parent is not writable: `{}`", parent.display()))?;
    source.close();
    destination.close();
    if !replacing {
        fs::remove_file(destination.path())
            .with_context(|| format!("output parent is not writable: `{}`", parent.display()))?;
    }

    fs::rename(source.path(), destination.path()).with_context(|| {
        format!(
            "output parent does not support atomic publication beside `{}`",
            output.display()
        )
    })?;
    source.persist();
    fs::remove_file(destination.path()).with_context(|| {
        format!(
            "failed to remove output publication probe `{}`",
            destination.path().display()
        )
    })?;
    destination.persist();
    Ok(())
}

/// Encode the complete PNG into a sibling file, then atomically replace the
/// requested output. The sibling placement makes the rename atomic and keeps
/// a failed encode from truncating an existing capture.
fn write_capture_png(output: &Path, rgba: &[u8], width: u32, height: u32) -> Result<()> {
    let mut temporary = create_capture_temp_file(output)?;
    let temporary_path = temporary.path().to_owned();
    {
        let file = temporary.file_mut();
        image::codecs::png::PngEncoder::new(&mut *file)
            .write_image(rgba, width, height, image::ColorType::Rgba8.into())
            .with_context(|| format!("failed to encode capture PNG `{}`", output.display()))?;
        file.flush().with_context(|| {
            format!("failed to flush capture PNG `{}`", temporary_path.display())
        })?;
        file.sync_all().with_context(|| {
            format!("failed to sync capture PNG `{}`", temporary_path.display())
        })?;
    }
    temporary.close();

    fs::rename(temporary.path(), output)
        .with_context(|| format!("failed to finalize capture PNG `{}`", output.display()))?;
    temporary.persist();
    Ok(())
}

/// Serialize the completed report beside its final path but keep it unnamed
/// until the final PNG has published. `CaptureTempFile::Drop` removes this
/// staged sibling on any later capture or PNG error.
fn stage_measurement_report(
    output: &Path,
    report: &impl serde::Serialize,
) -> Result<CaptureTempFile> {
    let mut temporary = create_capture_temp_file(output)?;
    let temporary_path = temporary.path().to_owned();
    {
        let file = temporary.file_mut();
        serde_json::to_writer_pretty(&mut *file, report).with_context(|| {
            format!("failed to encode measurement report `{}`", output.display())
        })?;
        file.write_all(b"\n").with_context(|| {
            format!(
                "failed to finalize measurement report `{}`",
                temporary_path.display()
            )
        })?;
        file.flush().with_context(|| {
            format!(
                "failed to flush measurement report `{}`",
                temporary_path.display()
            )
        })?;
        file.sync_all().with_context(|| {
            format!(
                "failed to sync measurement report `{}`",
                temporary_path.display()
            )
        })?;
    }
    temporary.close();
    Ok(temporary)
}

fn publish_staged_measurement_report(mut temporary: CaptureTempFile, output: &Path) -> Result<()> {
    temporary.close();
    fs::rename(temporary.path(), output).with_context(|| {
        format!(
            "failed to finalize measurement report `{}`",
            output.display()
        )
    })?;
    temporary.persist();
    Ok(())
}

fn capture_git_revision() -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let revision = String::from_utf8(output.stdout).ok()?;
    let revision = revision.trim();
    (!revision.is_empty()).then(|| revision.to_owned())
}

/// A newly-created capture file that removes itself unless it is renamed into
/// place successfully.
struct CaptureTempFile {
    path: Option<PathBuf>,
    file: Option<File>,
}

impl CaptureTempFile {
    fn path(&self) -> &Path {
        self.path
            .as_deref()
            .expect("temporary capture file path exists until persisted")
    }

    fn file_mut(&mut self) -> &mut File {
        self.file
            .as_mut()
            .expect("temporary capture file remains open until finalization")
    }

    fn close(&mut self) {
        drop(self.file.take());
    }

    fn persist(mut self) {
        self.path = None;
    }
}

impl Drop for CaptureTempFile {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = fs::remove_file(path);
        }
    }
}

fn create_capture_temp_file(output: &Path) -> Result<CaptureTempFile> {
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut label = OsString::from("capture-");
    label.push(
        output
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("capture")),
    );
    create_unique_sibling_file(parent, &label).with_context(|| {
        format!(
            "failed to create temporary capture PNG beside `{}`",
            output.display()
        )
    })
}

fn create_unique_sibling_file(
    parent: &Path,
    label: impl AsRef<std::ffi::OsStr>,
) -> Result<CaptureTempFile> {
    for _ in 0..MAX_UNIQUE_FILE_ATTEMPTS {
        let path = unique_sibling_path(parent, label.as_ref());
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => {
                return Ok(CaptureTempFile {
                    path: Some(path),
                    file: Some(file),
                });
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => {
                return Err(err).with_context(|| format!("failed to create `{}`", path.display()));
            }
        }
    }
    bail!("could not create a unique file in `{}`", parent.display())
}

fn unique_sibling_path(parent: &Path, label: &std::ffi::OsStr) -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = NEXT_UNIQUE_FILE_ID.fetch_add(1, Ordering::Relaxed);
    let mut name = OsString::from(".postretro-capture-");
    name.push(label);
    name.push(format!(
        "-{}-{timestamp:032x}-{sequence:016x}.tmp",
        std::process::id()
    ));
    parent.join(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_view_projection_uses_pitch_in_look_direction() {
        let camera = CameraPose {
            position: [0.0, 0.0, 0.0],
            yaw_deg: 0.0,
            pitch_deg: 30.0,
            fov_deg: 100.0,
        };
        let view_proj = capture_view_projection(&camera, 1280, 720);
        let expected = Mat4::perspective_rh(
            2.0 * ((camera.fov_deg.to_radians() / 2.0).tan() / (1280.0 / 720.0)).atan(),
            1280.0 / 720.0,
            camera::NEAR,
            camera::FAR,
        ) * Mat4::look_at_rh(
            Vec3::ZERO,
            Vec3::new(
                0.0,
                30.0_f32.to_radians().sin(),
                -30.0_f32.to_radians().cos(),
            ),
            Vec3::Y,
        );
        assert_mat4_approx_eq(view_proj, expected, 1.0e-6);
    }

    #[test]
    fn capture_view_projection_honors_scene_fov() {
        let narrow = CameraPose {
            position: [0.0, 0.0, 0.0],
            yaw_deg: 0.0,
            pitch_deg: 0.0,
            fov_deg: 60.0,
        };
        let wide = CameraPose {
            fov_deg: 130.0,
            ..narrow.clone()
        };
        let narrow_projection = capture_view_projection(&narrow, 1280, 720);
        let wide_projection = capture_view_projection(&wide, 1280, 720);
        assert_ne!(narrow_projection, wide_projection);
    }

    // Regression: passing the full light list fixed shadowmask indexing but
    // also introduced dynamic lights into captures that were static-only.
    #[test]
    fn capture_lights_remain_static_only_and_remap_shadow_selection() {
        let lights = [
            test_light(true, 1.0),
            test_light(false, 2.0),
            test_light(true, 3.0),
            test_light(false, 4.0),
        ];

        let influences =
            [1.0, 2.0, 3.0, 4.0].map(|radius| postretro_render_data::influence::LightInfluence {
                center: glam::Vec3::splat(radius),
                radius,
            });
        let (captured, captured_influences, selection) =
            capture_static_lights_and_shadow_selection(&lights, &influences, &[3, 0, 1, 99]);

        assert_eq!(captured.len(), 2, "capture must retain two static lights");
        assert!(
            (captured[0].intensity - 2.0).abs() < f32::EPSILON
                && (captured[1].intensity - 4.0).abs() < f32::EPSILON,
            "capture must retain the pre-change static-only compact order",
        );
        assert!(captured.iter().all(|light| !light.is_dynamic));
        assert!(
            (captured_influences[0].center - influences[1].center).length_squared() < f32::EPSILON
                && (captured_influences[0].radius - influences[1].radius).abs() < f32::EPSILON
                && (captured_influences[1].center - influences[3].center).length_squared()
                    < f32::EPSILON
                && (captured_influences[1].radius - influences[3].radius).abs() < f32::EPSILON,
            "capture must retain influence values in static-light compact order",
        );
        assert_eq!(
            selection,
            vec![1, u32::MAX, 0, u32::MAX],
            "selection order must stay channel-aligned while indices move into compact static space",
        );
    }

    #[test]
    fn absent_force_active_leaves_baked_descriptors_unmodified() {
        let writes = resolve_forced_active_animation_slots(&[test_light(false, 1.0)], None)
            .expect("absent force_active must not fail");
        assert!(
            writes.is_empty(),
            "without authored state, capture must leave install_level_geometry's baked descriptors intact"
        );
    }

    #[test]
    fn force_active_resolves_every_matching_animated_slot_once_in_stable_order() {
        let mut first = test_light(true, 1.0);
        first.tags = vec!["alarm_light".into()];
        first.animated_slot = Some(7);
        let mut second = test_light(false, 2.0);
        second.tags = vec!["alarm_light".into()];
        second.animated_slot = Some(3);
        let mut duplicate_slot = test_light(false, 3.0);
        duplicate_slot.tags = vec!["alarm_light".into()];
        duplicate_slot.animated_slot = Some(7);
        let forced = [ForcedAnimLight {
            tag: "alarm_light".into(),
            radiance: [4.0, 0.0, 0.0],
        }];

        let forward = resolve_forced_active_animation_slots(
            &[first.clone(), second.clone(), duplicate_slot.clone()],
            Some(&forced),
        )
        .expect("tag must resolve");
        let reversed =
            resolve_forced_active_animation_slots(&[duplicate_slot, second, first], Some(&forced))
                .expect("tag must resolve after map-light reordering");

        let expected = vec![(3, [4.0, 0.0, 0.0]), (7, [4.0, 0.0, 0.0])];
        assert_eq!(forward, expected);
        assert_eq!(reversed, expected);
    }

    #[test]
    fn force_active_rejects_unknown_map_light_tag() {
        let forced = [ForcedAnimLight {
            tag: "unknown_light".into(),
            radiance: [4.0, 0.0, 0.0],
        }];
        let err = resolve_forced_active_animation_slots(&[test_light(false, 1.0)], Some(&forced))
            .expect_err("unknown tag must not silently skip");
        assert!(err.to_string().contains("unknown_light"));
    }

    #[test]
    fn force_active_rejects_slots_outside_real_installed_descriptors() {
        let writes = [(3, [4.0, 0.0, 0.0])];
        for count in [0, 3] {
            let error = validate_forced_animation_slot_bounds(&writes, count)
                .expect_err("invalid slots must fail before renderer's no-op write");
            assert!(error.to_string().contains("animated slot 3"));
        }
        assert!(validate_forced_animation_slot_bounds(&writes, 4).is_ok());
        assert!(validate_forced_animation_slot_bounds(&[], 0).is_ok());
    }

    #[test]
    fn force_promotion_resolves_the_section_45_row_not_the_descriptor_slot() {
        let mut alarm = test_light(false, 1.0);
        alarm.tags = vec!["alarm_light".into()];
        alarm.animated_slot = Some(7);
        let section = postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection {
            affinity_factor: 4,
            affinity_dims: [0; 3],
            tile_dimension: 6,
            tile_border: 1,
            animation_descriptor_indices: vec![19, 7],
            valid_probe_masks: Vec::new(),
            cell_levels: Vec::new(),
            affinity_offsets: vec![0],
            affinity_lights: vec![1],
            delta_subblocks: Vec::new(),
        };
        let forced = [ForcedAnimatedPromotion {
            tag: "alarm_light".into(),
            weight: 0.75,
        }];

        assert_eq!(
            resolve_forced_animated_promotion_rows(&[alarm], Some(&section), Some(&forced))
                .expect("tagged animated light must resolve"),
            vec![(1, 0.75)],
            "the renderer weight state is keyed by raw AnimatedBakedLights row, not descriptor slot 7"
        );
    }

    // Regression: capture used the tag-matched duplicate while runtime joins
    // each raw roster row to the first static light carrying that slot.
    #[test]
    fn force_promotion_rejects_tag_on_shadowed_duplicate_animated_slot() {
        let mut runtime_owner = test_light(false, 1.0);
        runtime_owner.tags = vec!["first_light".into()];
        runtime_owner.animated_slot = Some(7);
        let mut shadowed_duplicate = test_light(false, 2.0);
        shadowed_duplicate.tags = vec!["alarm_light".into()];
        shadowed_duplicate.animated_slot = Some(7);
        let section = postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection {
            affinity_factor: 4,
            affinity_dims: [0; 3],
            tile_dimension: 6,
            tile_border: 1,
            animation_descriptor_indices: vec![7],
            valid_probe_masks: Vec::new(),
            cell_levels: Vec::new(),
            affinity_offsets: vec![0],
            affinity_lights: vec![0],
            delta_subblocks: Vec::new(),
        };
        let forced = [ForcedAnimatedPromotion {
            tag: "alarm_light".into(),
            weight: 0.75,
        }];

        let error = resolve_forced_animated_promotion_rows(
            &[runtime_owner, shadowed_duplicate],
            Some(&section),
            Some(&forced),
        )
        .expect_err("capture must not promote a different duplicate than runtime");

        assert!(
            error.to_string().contains("duplicate animated slot 7")
                && error.to_string().contains("first static map light"),
            "capture must report the raw-roster identity conflict: {error:#}",
        );

        let mut runtime_owner = test_light(false, 1.0);
        runtime_owner.tags = vec!["first_light".into()];
        runtime_owner.animated_slot = Some(7);
        let mut shadowed_duplicate = test_light(false, 2.0);
        shadowed_duplicate.tags = vec!["alarm_light".into()];
        shadowed_duplicate.animated_slot = Some(7);
        let owner_forced = [ForcedAnimatedPromotion {
            tag: "first_light".into(),
            weight: 0.5,
        }];
        assert_eq!(
            resolve_forced_animated_promotion_rows(
                &[runtime_owner, shadowed_duplicate],
                Some(&section),
                Some(&owner_forced),
            )
            .expect("the runtime-owned duplicate identity remains forceable"),
            vec![(0, 0.5)],
        );
    }

    #[test]
    fn force_promotion_rejects_animated_lights_missing_from_section_45() {
        let mut alarm = test_light(false, 1.0);
        alarm.tags = vec!["alarm_light".into()];
        alarm.animated_slot = Some(7);
        let section = postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection {
            affinity_factor: 4,
            affinity_dims: [0; 3],
            tile_dimension: 6,
            tile_border: 1,
            animation_descriptor_indices: vec![19],
            valid_probe_masks: Vec::new(),
            cell_levels: Vec::new(),
            affinity_offsets: vec![0],
            affinity_lights: Vec::new(),
            delta_subblocks: Vec::new(),
        };
        let forced = [ForcedAnimatedPromotion {
            tag: "alarm_light".into(),
            weight: 0.5,
        }];

        let error = resolve_forced_animated_promotion_rows(&[alarm], Some(&section), Some(&forced))
            .expect_err("a descriptor without an AnimatedBakedLights row cannot promote");
        assert!(error.to_string().contains("section-45"));
    }

    // Regression: a descriptor-only roster row reached the renderer and failed
    // later with the misleading "did not win a shadow-pool slot" error.
    #[test]
    fn force_promotion_rejects_section_45_row_without_affinity_delta() {
        let mut alarm = test_light(false, 1.0);
        alarm.tags = vec!["alarm_light".into()];
        alarm.animated_slot = Some(7);
        let section = postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection {
            affinity_factor: 4,
            affinity_dims: [0; 3],
            tile_dimension: 6,
            tile_border: 1,
            animation_descriptor_indices: vec![7],
            valid_probe_masks: Vec::new(),
            cell_levels: Vec::new(),
            affinity_offsets: vec![0],
            affinity_lights: Vec::new(),
            delta_subblocks: Vec::new(),
        };
        let forced = [ForcedAnimatedPromotion {
            tag: "alarm_light".into(),
            weight: 0.5,
        }];

        let error = resolve_forced_animated_promotion_rows(&[alarm], Some(&section), Some(&forced))
            .expect_err("a row without a direct delta cannot be promoted");
        assert!(
            error.to_string().contains("no affinity/direct delta")
                && error.to_string().contains("not promotion-eligible"),
            "capture must reject the ineligible row at configuration time: {error:#}",
        );
    }

    // Regression: standing up the promotion bridge from the full authored
    // light list restored dynamic-tier lighting to static-only captures.
    #[test]
    fn forced_promotion_bridge_input_remains_static_only_with_mixed_authored_lights() {
        let mut dynamic = test_light(true, 19.0);
        dynamic.animated_slot = Some(3);
        let mut animated_baked = test_light(false, 4.0);
        animated_baked.animated_slot = Some(7);
        let (capture_lights, capture_influences, _) = capture_static_lights_and_shadow_selection(
            &[dynamic, animated_baked.clone()],
            &[],
            &[],
        );

        let mut registry = EntityRegistry::new();
        let mut bridge = LightBridge::new();
        bridge.populate_from_level_with_influences(
            &capture_lights,
            &capture_influences,
            &[],
            &mut registry,
            0,
        );
        bridge.set_animated_baked_promotion_roster(&[7]);
        let update = bridge
            .update(&mut registry, 0.0, 1.0)
            .expect("the animated-baked tail must be emitted");

        assert!(
            update.effective_brightness.is_empty(),
            "capture promotion must not recreate an authored dynamic prefix",
        );
        assert_eq!(
            update.lights_bytes,
            postretro_lighting::pack_light(&animated_baked),
            "the renderer handoff contains only the raw section-45 tail record",
        );
    }

    #[test]
    fn forced_active_descriptor_uses_shared_active_without_animation_layout() {
        let descriptor = forced_active_animation_descriptor([4.0, 0.5, 0.25]);
        assert_eq!(
            f32::from_ne_bytes(descriptor[0..4].try_into().unwrap()),
            1.0
        );
        assert_eq!(
            u32::from_ne_bytes(descriptor[12..16].try_into().unwrap()),
            0,
            "the authored instant has no brightness curve samples"
        );
        assert_eq!(
            f32::from_ne_bytes(descriptor[16..20].try_into().unwrap()),
            4.0
        );
        assert_eq!(
            f32::from_ne_bytes(descriptor[20..24].try_into().unwrap()),
            0.5
        );
        assert_eq!(
            f32::from_ne_bytes(descriptor[24..28].try_into().unwrap()),
            0.25
        );
        assert_eq!(
            u32::from_ne_bytes(descriptor[32..36].try_into().unwrap()),
            0,
            "the authored instant has no color curve samples"
        );
        assert_eq!(
            u32::from_ne_bytes(descriptor[36..40].try_into().unwrap()),
            1
        );
    }

    #[test]
    fn capture_mesh_models_rejects_absent_or_empty_authored_prop_model() {
        // Regression: built-in dispatch kept invalid props, then capture silently
        // skipped their empty model handles and reported a successful image.
        for model in [None, Some("")] {
            let mut world = capture_receiver_test_world();
            world.map_entities[0].key_values = model
                .map(|model| vec![("model".to_string(), model.to_string())])
                .unwrap_or_default();
            let registry = spawn_capture_receiver_registry(&world)
                .expect("built-in dispatch retains the invalid prop for capture validation");
            assert_eq!(registry.iter_with_kind(ComponentKind::Mesh).count(), 1);
            let error = capture_mesh_models(&registry).expect_err("empty models must fail capture");
            assert!(error.to_string().contains("prop_mesh"));
            assert!(error.to_string().contains("absent or empty `model`"));
        }
    }

    #[test]
    fn capture_mesh_models_accepts_no_props_and_other_builtin_entities() {
        use postretro_level_format::map_entity::MapEntityRecord;

        let mut world = capture_receiver_test_world();
        world.map_entities.clear();
        let registry = spawn_capture_receiver_registry(&world).expect("no-prop map must spawn");
        assert!(capture_mesh_models(&registry).unwrap().is_empty());

        world.map_entities = vec![MapEntityRecord {
            classname: "billboard_emitter".into(),
            ..Default::default()
        }];
        let registry = spawn_capture_receiver_registry(&world).expect("emitter map must spawn");
        assert_eq!(
            registry
                .iter_with_kind(ComponentKind::BillboardEmitter)
                .count(),
            1
        );
        assert!(capture_mesh_models(&registry).unwrap().is_empty());
    }

    #[test]
    fn capture_receiver_draws_come_from_vm_free_spawned_registry_collectors() {
        let world = capture_receiver_test_world();
        let registry = spawn_capture_receiver_registry(&world)
            .expect("valid capture receiver fixture must spawn without scripts");

        assert_eq!(
            registry
                .iter_with_kind(ComponentKind::KinematicMover)
                .count(),
            1,
            "the loaded mover must enter the capture registry"
        );
        let mesh_models: Vec<_> = registry
            .iter_with_kind(ComponentKind::Mesh)
            .filter_map(|(_id, value)| match value {
                ComponentValue::Mesh(mesh) => Some(mesh.model.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            mesh_models,
            ["models/capture-prop.gltf"],
            "the prop_mesh record must be converted and routed through built-in dispatch"
        );
        assert_eq!(
            capture_mesh_models(&registry).unwrap(),
            ["models/capture-prop.gltf"]
        );

        let mut mover_collector = KinematicMoverRenderCollector::new();
        let mut mesh_collector = MeshRenderCollector::new();
        collect_capture_receiver_draws(
            &registry,
            &world,
            &VisibleCells::DrawAll,
            Vec3::ZERO,
            &mut mover_collector,
            &mut mesh_collector,
        );

        assert_eq!(mover_collector.instances().len(), 1);
        assert_eq!(mover_collector.shadow_instances().len(), 1);
        assert_eq!(mover_collector.occluder_aabbs().len(), 1);
        let occluder = mover_collector.occluder_aabbs()[0];
        assert_eq!(
            occluder.mover_id,
            mover_collector.shadow_instances()[0].mover_id
        );
        assert!(
            occluder
                .world_aabb
                .min
                .abs_diff_eq(Vec3::new(1.0, 2.0, 3.0), 1.0e-6)
        );
        assert!(
            occluder
                .world_aabb
                .max
                .abs_diff_eq(Vec3::new(2.0, 3.0, 3.0), 1.0e-6)
        );
        assert_eq!(mesh_collector.instances().len(), 1);
        assert_eq!(
            mover_collector.instances()[0].transform.w_axis.truncate(),
            Vec3::new(1.0, 2.0, 3.0),
            "the spawned mover is collected at its authored rest transform"
        );
        assert_eq!(
            mesh_collector.instances()[0].transform.w_axis.truncate(),
            Vec3::new(4.0, 5.0, 6.0),
            "the dispatched prop_mesh is collected at its authored rest transform"
        );
    }

    #[test]
    fn output_must_not_alias_map_or_scene_path() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let map = directory.path().join("level.prl");
        let scene = directory.path().join("scene.json");
        fs::write(&map, b"map").expect("map fixture");
        fs::write(&scene, b"scene").expect("scene fixture");

        let map_alias = directory.path().join("nested").join("..").join("level.prl");
        assert!(reject_output_source_aliases(&map_alias, &map, &scene).is_err());
        assert!(reject_output_source_aliases(&scene, &map, &scene).is_err());
        assert!(
            reject_output_source_aliases(&directory.path().join("new.png"), &map, &scene).is_ok()
        );
    }

    #[test]
    fn measurement_report_must_not_alias_scene_map_or_png_output() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let map = directory.path().join("level.prl");
        let scene = directory.path().join("scene.json");
        let png = directory.path().join("capture.png");
        fs::write(&map, b"map").expect("map fixture");
        fs::write(&scene, b"scene").expect("scene fixture");

        assert!(reject_output_source_aliases(&map, &map, &scene).is_err());
        assert!(reject_output_source_aliases(&scene, &map, &scene).is_err());
        assert!(reject_path_alias(&png, &png, "capture PNG output").is_err());
        assert!(
            reject_path_alias(
                &directory.path().join("measurement.json"),
                &png,
                "capture PNG output"
            )
            .is_ok()
        );
    }

    #[cfg(unix)]
    #[test]
    fn output_symlink_alias_and_non_regular_target_are_rejected() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("temporary directory");
        let map = directory.path().join("level.prl");
        let scene = directory.path().join("scene.json");
        let output = directory.path().join("capture.png");
        fs::write(&map, b"map").expect("map fixture");
        fs::write(&scene, b"scene").expect("scene fixture");
        symlink(&map, &output).expect("output symlink");

        assert!(reject_output_source_aliases(&output, &map, &scene).is_err());
        assert!(preflight_output_path(&output).is_err());
    }

    #[test]
    fn output_preflight_exercises_atomic_publication_without_leaking_probes() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let output = directory.path().join("capture.png");

        preflight_output_path(&output).expect("new output preflight");
        assert_eq!(
            fs::read_dir(directory.path()).expect("directory").count(),
            0
        );

        fs::write(&output, b"existing").expect("existing output");
        preflight_output_path(&output).expect("replacement output preflight");
        assert_eq!(
            fs::read(&output).expect("existing output intact"),
            b"existing"
        );
        assert_eq!(
            fs::read_dir(directory.path()).expect("directory").count(),
            1
        );
    }

    #[test]
    fn staged_measurement_report_preserves_prior_report_until_final_publication() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let report = directory.path().join("measurement.json");
        fs::write(&report, b"prior successful report").expect("prior report");

        let staged = stage_measurement_report(&report, &serde_json::json!({ "run": "new" }))
            .expect("stage replacement report");
        assert_eq!(
            fs::read(&report).expect("prior report stays visible"),
            b"prior successful report"
        );
        drop(staged);
        assert_eq!(
            fs::read(&report).expect("prior report stays visible after staged failure"),
            b"prior successful report"
        );

        let staged = stage_measurement_report(&report, &serde_json::json!({ "run": "new" }))
            .expect("stage replacement report");
        publish_staged_measurement_report(staged, &report).expect("publish report last");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&fs::read(&report).expect("report"))
                .expect("published JSON")["run"],
            "new"
        );
    }

    #[test]
    fn unique_sibling_creation_is_not_limited_to_sixteen_candidates() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let files: Vec<_> = (0..64)
            .map(|_| create_unique_sibling_file(directory.path(), "collision-test").expect("file"))
            .collect();
        let paths: HashSet<_> = files.iter().map(|file| file.path().to_owned()).collect();
        assert_eq!(paths.len(), files.len());
        drop(files);
        assert_eq!(
            fs::read_dir(directory.path()).expect("directory").count(),
            0
        );
    }

    #[test]
    fn capture_png_replaces_existing_output_without_leaving_temp_file() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let output = directory.path().join("capture.png");
        fs::write(&output, b"old capture").expect("existing capture");

        write_capture_png(&output, &[1, 2, 3, 4], 1, 1).expect("write capture");

        let decoded = image::open(&output).expect("decode capture").to_rgba8();
        assert_eq!(decoded.as_raw(), &[1, 2, 3, 4]);
        assert_eq!(
            fs::read_dir(directory.path())
                .expect("read capture directory")
                .count(),
            1,
            "successful capture removes its temporary sibling"
        );
    }

    fn test_light(is_dynamic: bool, intensity: f32) -> postretro_level_loader::MapLight {
        postretro_level_loader::MapLight {
            origin: [0.0; 3],
            light_type: postretro_level_loader::LightType::Point,
            intensity,
            color: [1.0; 3],
            falloff_model: postretro_level_loader::FalloffModel::Linear,
            falloff_range: 8.0,
            cone_angle_inner: 0.0,
            cone_angle_outer: 0.0,
            cone_direction: [0.0, 0.0, -1.0],
            is_dynamic,
            casts_entity_shadows: false,
            animated_slot: None,
            tags: Vec::new(),
            cell_index: 0,
            shadow_type: postretro_level_loader::ShadowType::StaticLightMap,
        }
    }

    fn capture_receiver_test_world() -> postretro_level_loader::LevelWorld {
        use postretro_level_format::geometry::Vertex;
        use postretro_level_format::map_entity::MapEntityRecord;
        use postretro_level_loader::{
            CellData, CellLocatorChild, KinematicGeometry, LoadedKinematicMover,
            LoadedKinematicWaypoint,
        };

        let mut world = postretro_level_loader::LevelWorld::new_visibility_only(
            vec![CellData {
                bounds_min: Vec3::splat(-1_000.0),
                bounds_max: Vec3::splat(1_000.0),
                face_start: 0,
                face_count: 0,
                portal_ref_start: 0,
                portal_ref_count: 0,
                is_solid: false,
                is_exterior: false,
                is_drawable: false,
            }],
            Vec::new(),
            CellLocatorChild::Cell(0),
            Vec::new(),
            Vec::new(),
            false,
        )
        .expect("single-cell capture receiver fixture must be valid");
        world.kinematic_geometry = KinematicGeometry {
            movers: vec![LoadedKinematicMover {
                mover_id: 17,
                name: "capture-lift".to_string(),
                tags: Vec::new(),
                origin: Vec3::new(1.0, 2.0, 3.0),
                path: "a".to_string(),
                speed_mps: 1.0,
                wait_ms: 0.0,
                move_mode: 1,
                start_on_spawn: false,
                vertices: vec![
                    Vertex::new(
                        [0.0, 0.0, 0.0],
                        [0.0, 0.0],
                        [0.0, 1.0, 0.0],
                        [1.0, 0.0, 0.0],
                        true,
                        [0.0, 0.0],
                        0,
                    ),
                    Vertex::new(
                        [1.0, 0.0, 0.0],
                        [1.0, 0.0],
                        [0.0, 1.0, 0.0],
                        [1.0, 0.0, 0.0],
                        true,
                        [0.0, 0.0],
                        0,
                    ),
                    Vertex::new(
                        [0.0, 1.0, 0.0],
                        [0.0, 1.0],
                        [0.0, 1.0, 0.0],
                        [1.0, 0.0, 0.0],
                        true,
                        [0.0, 0.0],
                        0,
                    ),
                ],
                indices: vec![0, 1, 2],
                face_meta: Vec::new(),
                spin_axis: Vec3::ZERO,
                spin_speed_deg_s: 0.0,
                spin_accel_deg_s2: 0.0,
                carry_yaw: false,
                block_policy: "displace".to_string(),
                crush_damage: 0.0,
                crush_interval_ms: 0.0,
                auto_close_ms: None,
                open_event: None,
                close_event: None,
                blocked_event: None,
                crush_event: None,
                sealed_portal_ids: Vec::new(),
                carried_lights: Vec::new(),
            }],
            waypoints: vec![
                LoadedKinematicWaypoint {
                    name: "a".to_string(),
                    next: "b".to_string(),
                    origin: Vec3::new(1.0, 2.0, 3.0),
                },
                LoadedKinematicWaypoint {
                    name: "b".to_string(),
                    next: String::new(),
                    origin: Vec3::new(2.0, 2.0, 3.0),
                },
            ],
        };
        world.map_entities = vec![MapEntityRecord {
            classname: "prop_mesh".to_string(),
            origin: [4.0, 5.0, 6.0],
            key_values: vec![("model".to_string(), "models/capture-prop.gltf".to_string())],
            ..Default::default()
        }];
        world
    }

    fn assert_mat4_approx_eq(actual: Mat4, expected: Mat4, epsilon: f32) {
        for (actual, expected) in actual
            .to_cols_array()
            .into_iter()
            .zip(expected.to_cols_array())
        {
            assert!(
                (actual - expected).abs() <= epsilon,
                "matrix values differ: {actual} != {expected}"
            );
        }
    }
}
