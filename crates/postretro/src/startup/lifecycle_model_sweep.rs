//! CPU half of the level-load model sweep: one glTF parse per distinct model.
//! See: context/lib/boot_sequence.md §3

use std::path::{Path, PathBuf};

use postretro_model::gltf_loader::{self, LoadedModel, ModelLoadError};

use crate::scripting_systems::hit_zones::{HitZoneStore, ModelLoadWarningOwner};

/// One distinct sweep model, parsed once. The renderer upload borrows it and the
/// hit-zone store then consumes it, so the file is read, parsed and its source
/// PNGs hashed once per load rather than once per consumer.
///
/// A failed parse stays in `result`: both consumers see the same error, and the
/// [`ModelLoadWarningOwner`] the upload hook returns decides which of them
/// reports it.
pub(crate) struct ParsedSweepModel {
    /// The verbatim handle: the renderer's cache key and `MeshComponent.model`.
    pub(crate) handle: String,
    /// `content_root.join(handle)`, the file that was opened.
    pub(crate) open_path: PathBuf,
    pub(crate) result: Result<LoadedModel, ModelLoadError>,
}

/// Parse every model in `models`, in order, on the calling thread. Parsing never
/// depends on a renderer, so a headless install runs the same step.
pub(crate) fn parse_sweep_models(models: &[String], content_root: &Path) -> Vec<ParsedSweepModel> {
    models
        .iter()
        .map(|handle| {
            let open_path = content_root.join(handle);
            let result = gltf_loader::load_model(&open_path);
            ParsedSweepModel {
                handle: handle.clone(),
                open_path,
                result,
            }
        })
        .collect()
}

/// Install each parsed model's hit-zone entry, consuming the parses. Equivalent
/// to calling [`HitZoneStore::insert_from_load`] on every handle, minus the
/// second parse.
pub(crate) fn install_hit_zones(
    store: &mut HitZoneStore,
    parsed: Vec<ParsedSweepModel>,
    warning_owner: ModelLoadWarningOwner,
) {
    for model in parsed {
        store.insert_loaded(&model.handle, &model.open_path, model.result, warning_owner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting_systems::mesh_anim::{MeshClipTables, ModelClipTable};
    use postretro_model::ModelHandle;

    /// Every glTF under `content/dev`: a superset of what any dev map's sweep
    /// uploads (campaign-test's sweep uploads ten of these), as handles relative
    /// to the content root.
    fn dev_models() -> (PathBuf, Vec<String>) {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/dev");
        let mut found = Vec::new();
        let mut stack = vec![root.join("models")];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("dev model directory reads") {
                let path = entry.expect("directory entry reads").path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|ext| ext == "gltf") {
                    let rel = path.strip_prefix(&root).expect("under the content root");
                    found.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
        found.sort();
        (root, found)
    }

    /// What the renderer caches and the sweep reads back for the game-side clip
    /// table: the clip name/duration list in glTF order and the mesh's
    /// conservative bound. Both are plain functions of the parse.
    fn clip_table_inputs(
        model: &LoadedModel,
    ) -> (
        Vec<postretro_model::ClipMetadata>,
        postretro_render_data::cone_frustum::Aabb,
    ) {
        let meta = model
            .clips
            .iter()
            .map(|clip| postretro_model::ClipMetadata {
                name: clip.name.clone(),
                duration: clip.duration,
            })
            .collect();
        (meta, model.mesh.bounds())
    }

    /// The old sweep: each consumer parses the file itself.
    fn two_parse_sweep(
        root: &Path,
        models: &[String],
        owner: ModelLoadWarningOwner,
    ) -> (HitZoneStore, MeshClipTables) {
        let mut store = HitZoneStore::new();
        let mut tables = MeshClipTables::new();
        for model in models {
            // Renderer side: its own parse.
            if let Ok(parsed) = gltf_loader::load_model(&root.join(model)) {
                let (meta, bounds) = clip_table_inputs(&parsed);
                tables.insert_with_bounds(ModelHandle::from(model.clone()), &meta, bounds);
            }
            // Game side: its own parse.
            store.insert_from_load(model, root, owner);
        }
        (store, tables)
    }

    /// The new sweep: one parse feeds both.
    fn one_parse_sweep(
        root: &Path,
        models: &[String],
        owner: ModelLoadWarningOwner,
    ) -> (HitZoneStore, MeshClipTables) {
        let mut store = HitZoneStore::new();
        let mut tables = MeshClipTables::new();
        let parsed = parse_sweep_models(models, root);
        for model in &parsed {
            if let Ok(loaded) = &model.result {
                let (meta, bounds) = clip_table_inputs(loaded);
                tables.insert_with_bounds(ModelHandle::from(model.handle.clone()), &meta, bounds);
            }
        }
        install_hit_zones(&mut store, parsed, owner);
        (store, tables)
    }

    /// Compare what the public API of the two stores and clip tables exposes for
    /// one handle. The deep field comparison (sockets, legs, warning set) lives
    /// beside the store, in `hit_zones`' own oracle test.
    fn assert_same_for(
        handle: &str,
        old: &(HitZoneStore, MeshClipTables),
        new: &(HitZoneStore, MeshClipTables),
    ) {
        let key = ModelHandle::from(handle.to_string());
        match (old.0.get(&key), new.0.get(&key)) {
            (None, None) => {}
            (Some(a), Some(b)) => {
                assert_eq!(
                    format!("{:?}", a.skeleton),
                    format!("{:?}", b.skeleton),
                    "{handle}"
                );
                assert_eq!(
                    format!("{:?}", a.clips),
                    format!("{:?}", b.clips),
                    "{handle}"
                );
                assert_eq!(a.joint_zones, b.joint_zones, "{handle}");
                assert_eq!(a.derived_bound, b.derived_bound, "{handle}");
                assert_eq!(
                    format!("{:?}", a.pose_stack),
                    format!("{:?}", b.pose_stack),
                    "{handle}"
                );
            }
            (a, b) => panic!(
                "{handle}: entry present in old={} new={}",
                a.is_some(),
                b.is_some()
            ),
        }
        assert_eq!(
            old.0.has_pose_modifiers(&key),
            new.0.has_pose_modifiers(&key),
            "{handle}"
        );
        assert_eq!(
            old.0.has_precise_zones(&key),
            new.0.has_precise_zones(&key),
            "{handle}"
        );
        let a: Option<&ModelClipTable> = old.1.get(&key);
        let b: Option<&ModelClipTable> = new.1.get(&key);
        assert_eq!(a, b, "{handle}: clip table");
        assert_eq!(
            format!("{:?}", old.1.model_bounds(&key)),
            format!("{:?}", new.1.model_bounds(&key)),
            "{handle}: model bounds"
        );
    }

    /// Run the two sweeps over `models` under both warning owners and compare
    /// everything the public API exposes. Returns (models with a hit-zone entry,
    /// models with a non-empty clip table) from the one-parse sweep.
    fn compare_sweeps(root: &Path, models: &[String]) -> (usize, usize) {
        let mut coverage = (0, 0);
        for owner in [
            ModelLoadWarningOwner::Renderer,
            ModelLoadWarningOwner::GameSide,
        ] {
            let old = two_parse_sweep(root, models, owner);
            let new = one_parse_sweep(root, models, owner);
            let mut with_entry = 0;
            let mut with_clips = 0;
            for model in models {
                assert_same_for(model, &old, &new);
                let key = ModelHandle::from(model.clone());
                with_entry += usize::from(new.0.get(&key).is_some());
                with_clips += usize::from(
                    new.1
                        .get(&key)
                        .is_some_and(|table| table.duration(0).is_some()),
                );
            }
            coverage = (with_entry, with_clips);
        }
        coverage
    }

    /// Hit-zone entries, clip tables and bounds from one shared parse equal what
    /// two independent parses produce, for every dev model.
    #[test]
    fn single_parse_sweep_matches_the_two_parse_sweep_on_every_dev_model() {
        let (root, models) = dev_models();
        assert!(
            models.len() >= 20,
            "dev mod should ship its model set, found {}",
            models.len()
        );
        let (with_entry, with_clips) = compare_sweeps(&root, &models);
        eprintln!(
            "[sweep-equivalence] {} dev models: {with_entry} hit-zone entries, {with_clips} with clips",
            models.len(),
        );
        assert!(with_entry >= 20, "most dev models load: {with_entry}");
        assert!(with_clips >= 3, "some dev models animate: {with_clips}");
    }

    /// Models that fail to load (missing file, not glTF, missing buffer, empty
    /// handle) leave the same nothing behind either way.
    #[test]
    fn single_parse_sweep_matches_the_two_parse_sweep_when_loads_fail() {
        let scratch = std::env::temp_dir().join(format!(
            "postretro_single_parse_sweep_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&scratch).expect("scratch directory creates");
        std::fs::write(scratch.join("garbage.gltf"), b"not json").expect("garbage writes");
        std::fs::write(
            scratch.join("missing_buffer.gltf"),
            br#"{"asset":{"version":"2.0"},"buffers":[{"uri":"nope.bin","byteLength":4}]}"#,
        )
        .expect("missing-buffer model writes");
        let models: Vec<String> = ["garbage.gltf", "missing_buffer.gltf", "absent.gltf", ""]
            .iter()
            .map(|m| m.to_string())
            .collect();

        let (with_entry, with_clips) = compare_sweeps(&scratch, &models);
        let _ = std::fs::remove_dir_all(&scratch);
        assert_eq!(
            (with_entry, with_clips),
            (0, 0),
            "every scratch model fails"
        );
    }

    #[test]
    fn parse_sweep_models_keeps_order_handles_and_open_paths() {
        let (root, models) = dev_models();
        let subset: Vec<String> = models.into_iter().take(3).collect();
        let parsed = parse_sweep_models(&subset, &root);
        assert_eq!(parsed.len(), 3);
        for (model, handle) in parsed.iter().zip(&subset) {
            assert_eq!(&model.handle, handle);
            assert_eq!(model.open_path, root.join(handle));
        }
    }
}
