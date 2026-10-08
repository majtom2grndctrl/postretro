//! The single-parse install path (`insert_loaded`) against the two-parse path it
//! replaced: the pre-change `insert_from_load` body, kept here verbatim as the
//! oracle, run over every dev-mod model and over models that fail to load.
//! See: context/lib/boot_sequence.md §3

use super::*;

/// The `insert_from_load` of the commit before the sweep parsed each glTF once:
/// it parses the file itself, then installs. Kept verbatim (only the receiver
/// name differs) so the equality below compares the new path against the old
/// code, not against a refactor of it.
fn insert_from_load_two_parse_oracle(
    store: &mut HitZoneStore,
    model_rel: &str,
    content_root: &Path,
    warning_owner: ModelLoadWarningOwner,
) {
    let open_path = content_root.join(model_rel);
    let handle = ModelHandle::from(model_rel.to_string());
    store.models.remove(handle.as_str());
    store.pose_modified_models.remove(&handle);

    let model = match gltf_loader::load_model(&open_path) {
        Ok(m) => m,
        Err(err) => {
            if warning_owner == ModelLoadWarningOwner::GameSide
                && store.model_load_warnings.insert(model_rel.to_owned())
            {
                log::warn!(
                    "[Model] model load failed for {}: {err} — \
                     any mesh or attachment using it remains unresolved",
                    open_path.display(),
                );
            }
            return;
        }
    };

    let gltf_loader::LoadedModel {
        skeleton,
        clips,
        joint_zones,
        sockets,
        pose_stack,
        legs,
        ..
    } = model;

    let derived_bound = if ModelHitZones::has_zones(&joint_zones) {
        derive_bound(&skeleton, &clips, &joint_zones)
    } else {
        None
    };

    if pose_stack.is_empty() {
        store.pose_modified_models.remove(&handle);
    } else {
        store.pose_modified_models.insert(handle.clone());
    }
    store.models.insert(
        handle.as_str().to_owned(),
        ModelHitZones {
            skeleton: Arc::new(skeleton),
            clips: Arc::new(clips),
            joint_zones,
            sockets,
            derived_bound,
            legs,
            pose_stack: Arc::new(pose_stack),
        },
    );
}

fn content_dev_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../content/dev")
}

/// Every glTF under `content/dev/models`, as handles relative to the content
/// root. A superset of any dev map's sweep (campaign-test's sweep uploads ten).
fn dev_models() -> Vec<String> {
    let root = content_dev_root();
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
    found
}

/// Field-for-field equality of two stores. `Debug` text compares floats
/// bit-exactly enough for this purpose (it also treats NaN as equal to itself,
/// which `==` would not).
fn assert_stores_equal(old: &HitZoneStore, new: &HitZoneStore, context: &str) {
    let mut old_keys: Vec<_> = old.models.keys().collect();
    let mut new_keys: Vec<_> = new.models.keys().collect();
    old_keys.sort();
    new_keys.sort();
    assert_eq!(old_keys, new_keys, "{context}: same models retained");
    for (handle, a) in &old.models {
        let b = &new.models[handle];
        let at = |field: &str| format!("{context}: {handle}: {field}");
        assert_eq!(
            format!("{:?}", a.skeleton),
            format!("{:?}", b.skeleton),
            "{}",
            at("skeleton")
        );
        assert_eq!(
            format!("{:?}", a.clips),
            format!("{:?}", b.clips),
            "{}",
            at("clips")
        );
        assert_eq!(a.joint_zones, b.joint_zones, "{}", at("joint_zones"));
        assert_eq!(a.sockets, b.sockets, "{}", at("sockets"));
        assert_eq!(
            format!("{:?}", a.derived_bound),
            format!("{:?}", b.derived_bound),
            "{}",
            at("derived_bound")
        );
        assert_eq!(
            format!("{:?}", a.legs),
            format!("{:?}", b.legs),
            "{}",
            at("legs")
        );
        assert_eq!(
            format!("{:?}", a.pose_stack),
            format!("{:?}", b.pose_stack),
            "{}",
            at("pose_stack")
        );
    }
    assert_eq!(
        old.pose_modified_models, new.pose_modified_models,
        "{context}: pose-stack membership"
    );
    assert_eq!(
        old.model_load_warnings, new.model_load_warnings,
        "{context}: failed loads reported once, by the same owner"
    );
}

/// Install `models` both ways, for both warning owners, and compare. Returns
/// the new store from the `GameSide` run so callers can assert coverage.
fn compare_both_ways(root: &Path, models: &[String]) -> HitZoneStore {
    let mut game_side_new = None;
    for owner in [
        ModelLoadWarningOwner::Renderer,
        ModelLoadWarningOwner::GameSide,
    ] {
        let mut old = HitZoneStore::new();
        let mut new = HitZoneStore::new();
        for model in models {
            insert_from_load_two_parse_oracle(&mut old, model, root, owner);
            // The sweep's shape: one parse, handed over by value.
            let open_path = root.join(model);
            let parsed = gltf_loader::load_model(&open_path);
            new.insert_loaded(model, &open_path, parsed, owner);
        }
        assert_stores_equal(&old, &new, &format!("owner {owner:?}"));
        if owner == ModelLoadWarningOwner::GameSide {
            game_side_new = Some(new);
        }
    }
    game_side_new.expect("the GameSide pass ran")
}

#[test]
fn insert_loaded_equals_the_two_parse_path_on_every_dev_model() {
    let models = dev_models();
    assert!(
        models.len() >= 20,
        "the dev mod ships its model set: {}",
        models.len()
    );
    let store = compare_both_ways(&content_dev_root(), &models);

    // The comparison must not be vacuous: the dev set exercises zones, derived
    // bounds, sockets, pose stacks and legs.
    let entries = store.models.values().collect::<Vec<_>>();
    let count = |pred: &dyn Fn(&ModelHitZones) -> bool| entries.iter().filter(|m| pred(m)).count();
    let loaded = entries.len();
    let zoned = count(&|m| ModelHitZones::has_zones(&m.joint_zones));
    let bounded = count(&|m| m.derived_bound.is_some());
    let socketed = count(&|m| !m.sockets.is_empty());
    let posed = count(&|m| !m.pose_stack.is_empty());
    let legged = count(&|m| !m.legs.is_empty());
    let animated = count(&|m| !m.clips.is_empty());
    eprintln!(
        "[load-equivalence] {} dev models: {loaded} loaded, {zoned} with zones, {bounded} with derived bounds, \
         {socketed} with sockets, {posed} with pose stacks, {legged} with legs, {animated} with clips",
        models.len(),
    );
    assert!(loaded >= 20, "most dev models load: {loaded}");
    assert!(zoned >= 1, "some dev model carries hit zones");
    assert!(bounded >= 1, "some dev model derives a bound");
    assert!(socketed >= 1, "some dev model carries sockets");
    assert!(posed >= 1, "some dev model carries a pose stack");
    assert!(animated >= 3, "several dev models animate");
}

#[test]
fn insert_loaded_equals_the_two_parse_path_when_a_load_fails() {
    let scratch = std::env::temp_dir().join(format!(
        "postretro_hit_zone_load_equivalence_{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&scratch).expect("scratch directory creates");
    std::fs::write(scratch.join("garbage.gltf"), b"not json").expect("garbage model writes");
    std::fs::write(
        scratch.join("missing_buffer.gltf"),
        br#"{"asset":{"version":"2.0"},"buffers":[{"uri":"nope.bin","byteLength":4}]}"#,
    )
    .expect("missing-buffer model writes");
    let models: Vec<String> = ["garbage.gltf", "missing_buffer.gltf", "absent.gltf", ""]
        .iter()
        .map(|m| m.to_string())
        .collect();

    let store = compare_both_ways(&scratch, &models);
    let _ = std::fs::remove_dir_all(&scratch);

    assert!(store.models.is_empty(), "every scratch model fails to load");
    assert_eq!(
        store.model_load_warnings.len(),
        models.len(),
        "a game-side failure is recorded once per model",
    );
}

/// The failure-owner contract that `ModelLoadWarningOwner` encodes: when the
/// renderer owns the diagnostic the store stays silent (records nothing), and a
/// failed load still clears a stale entry, as before.
#[test]
fn insert_loaded_failure_is_silent_for_the_renderer_owner_and_clears_stale_entries() {
    let fixture = content_dev_root().join("models/pose-modifier-fixture/joint_zones.gltf");
    let mut store = HitZoneStore::new();
    let handle = ModelHandle::from("m.gltf".to_string());
    store.insert_loaded(
        "m.gltf",
        &fixture,
        gltf_loader::load_model(&fixture),
        ModelLoadWarningOwner::Renderer,
    );
    assert!(store.get(&handle).is_some(), "fixture loads");
    assert!(store.has_pose_modifiers(&handle));

    let missing = Path::new("/definitely/not/here.gltf");
    store.insert_loaded(
        "m.gltf",
        missing,
        gltf_loader::load_model(missing),
        ModelLoadWarningOwner::Renderer,
    );
    assert!(store.get(&handle).is_none(), "stale entry cleared");
    assert!(!store.has_pose_modifiers(&handle));
    assert!(
        store.model_load_warnings.is_empty(),
        "the renderer owns this diagnostic; the store reports nothing",
    );
}

/// The premise of sharing one parse: parsing the same file twice yields the same
/// model, field for field, so what the renderer derives from its parse (clip
/// metadata, bounds, material keys) equals what it derived from the second parse.
#[test]
fn the_loader_is_deterministic_on_every_dev_model() {
    let root = content_dev_root();
    let mut compared = 0;
    for model in dev_models() {
        let path = root.join(&model);
        let (Ok(a), Ok(b)) = (
            gltf_loader::load_model(&path),
            gltf_loader::load_model(&path),
        ) else {
            continue;
        };
        // `sockets` is a `HashMap`: compare it by value, the rest by `Debug`.
        assert_eq!(a.sockets, b.sockets, "{model}: sockets");
        let strip = |m: &gltf_loader::LoadedModel| {
            let mut m = m.clone();
            m.sockets.clear();
            format!("{m:?}")
        };
        assert_eq!(strip(&a), strip(&b), "{model}: parse is deterministic");
        compared += 1;
    }
    assert!(compared >= 20, "compared {compared} dev models");
}
