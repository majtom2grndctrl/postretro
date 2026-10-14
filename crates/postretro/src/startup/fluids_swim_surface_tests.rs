// The fluids-and-swimming scripting-surface example run as mod manifests
// through the production script runtime: the `content/dev/scripts/
// fluids-swim.{ts,luau}` twins must declare identical fluids, swim tuning,
// player events and crossings, and content/dev's own TypeScript manifest must
// declare the same under its mod id. content/dev's manifest is TS-only, so
// the Luau half of the example lives in the twin.
// See: context/lib/scripting.md §12 (Player events)

use std::path::{Path, PathBuf};

use postretro_entities::ScriptCtx;
use postretro_foundation::data_descriptors::{FluidDescriptor, SwimParams};
use postretro_foundation::ir::IrNode;
use postretro_scripting_core::data_descriptors::{CrossingCondition, PlayerEventEdge};
use postretro_scripting_core::primitives_registry::PrimitiveRegistry;
use postretro_scripting_core::runtime::{ModManifestResult, ScriptRuntime, ScriptRuntimeConfig};
use postretro_test_log_capture::LogCapture;

use crate::scripting::primitives::register_all;

pub(super) fn dev_content() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/dev")
}

/// The TS twin bundled the way a debug launch compiles a start script.
fn bundled_ts_twin() -> String {
    postretro_script_compiler::bundle_entry(&dev_content().join("scripts/fluids-swim.ts"))
        .expect("fluids-swim.ts bundles")
}

fn luau_twin() -> String {
    std::fs::read_to_string(dev_content().join("scripts/fluids-swim.luau"))
        .expect("fluids-swim.luau reads")
}

/// Run `source` as a mod root's start script (`start-script.js` or
/// `start-script.luau`) through a fresh runtime, as mod init does at boot.
pub(super) fn run_manifest(file_name: &str, source: &str) -> Result<ModManifestResult, String> {
    let ctx = ScriptCtx::new();
    let mut primitives = PrimitiveRegistry::new();
    register_all(&mut primitives, ctx.clone());
    let mut runtime = ScriptRuntime::new(
        &primitives,
        &ScriptRuntimeConfig {
            skip_identity_enforcement: true,
            ..Default::default()
        },
        &ctx,
    )
    .expect("script runtime constructs");
    let mod_root = tempfile::tempdir().expect("temp mod root");
    std::fs::write(mod_root.path().join(file_name), source).expect("start script writes");
    runtime
        .run_mod_init(mod_root.path())
        .map_err(|error| error.to_string())?;
    Ok(runtime
        .mod_manifest()
        .expect("mod init commits a manifest")
        .clone())
}

/// content/dev's own start script, initialized as at boot.
pub(super) fn dev_manifest() -> ModManifestResult {
    let bundle = postretro_script_compiler::bundle_entry(&dev_content().join("start-script.ts"))
        .expect("content/dev start-script.ts bundles");
    run_manifest("start-script.js", &bundle).expect("content/dev initializes")
}

fn swim_of(manifest: &ModManifestResult, canonical_name: &str) -> Option<SwimParams> {
    manifest
        .entities
        .iter()
        .find(|entity| entity.canonical_name.as_deref() == Some(canonical_name))
        .unwrap_or_else(|| panic!("no `{canonical_name}` entity"))
        .movement
        .as_ref()
        .expect("movement present")
        .swim
        .clone()
}

fn fluids_of(manifest: &ModManifestResult) -> Vec<FluidDescriptor> {
    manifest.fluids.descriptors().to_vec()
}

fn swimming_read() -> IrNode {
    IrNode::Input {
        name: "player.swimming".into(),
        owner: None,
    }
}

/// Replace exactly one occurrence, so a mutation that no longer matches the
/// fixture fails loudly instead of testing the unmutated manifest.
fn mutate(source: &str, from: &str, to: &str) -> String {
    assert_eq!(source.matches(from).count(), 1, "`{from}` must occur once");
    source.replacen(from, to, 1)
}

// The fluids-and-swimming example runs as a content/dev fixture in
// TypeScript and Luau, and both declare the same manifest.
#[test]
fn fluids_swim_twins_run_as_mod_manifests_and_declare_the_same_surface() {
    let ts = run_manifest("start-script.js", &bundled_ts_twin()).expect("TS twin initializes");
    let luau = run_manifest("start-script.luau", &luau_twin()).expect("Luau twin initializes");

    assert_eq!(ts.fluids, luau.fluids);
    assert_eq!(swim_of(&ts, "player"), swim_of(&luau, "player"));
    assert_eq!(ts.player_events, luau.player_events);
    assert_eq!(ts.crossings, luau.crossings);
    assert_eq!(ts.reactions, luau.reactions);

    let fluids = fluids_of(&ts);
    assert_eq!(
        fluids,
        vec![
            FluidDescriptor {
                name: "water".into(),
                viscosity: 2.0,
                view_tint: [0.1, 0.3, 0.4, 0.35],
                muffle: 0.8,
                surface_opacity: 0.75,
                enter_sound: Some("fixtures/land".into()),
                exit_sound: Some("fixtures/jump".into()),
            },
            FluidDescriptor {
                name: "sludge".into(),
                viscosity: 6.0,
                view_tint: [0.25, 0.3, 0.08, 0.85],
                muffle: 0.95,
                surface_opacity: 0.95,
                enter_sound: Some("fixtures/land".into()),
                exit_sound: None,
            },
        ]
    );
    assert_eq!(
        swim_of(&ts, "player"),
        Some(SwimParams {
            speed: 4.5,
            accel: 6.0,
            sink_speed: 0.8,
            enter_depth: 0.5,
            exit_depth: 0.35,
            fluid_jump_velocity: Some(9.35),
        })
    );

    // `players().on(becomes(read(player.swimming)), [markSwimming])`.
    assert_eq!(ts.player_events.len(), 1);
    let event = &ts.player_events[0];
    assert_eq!(event.edge, PlayerEventEdge::Becomes);
    assert_eq!(event.condition, swimming_read());
    assert_eq!(event.fire, ["swim.markSwimming"]);
    assert!(event.levels.is_empty());

    // `onStateCrossing(read(player.swimming), [splashSound])`: the fluent read
    // lowers to the predicate form's IR, not a threshold on a slot.
    assert_eq!(ts.crossings.len(), 1);
    let crossing = &ts.crossings[0].crossing;
    assert_eq!(crossing.slot, None);
    assert_eq!(crossing.condition, CrossingCondition::Ir(swimming_read()));
    assert_eq!(crossing.fire, ["swim.splash"]);

    let names: Vec<&str> = ts
        .reactions
        .iter()
        .map(|reaction| reaction.reaction.name.as_str())
        .collect();
    assert_eq!(names, ["swim.markSwimming", "swim.splash"]);
}

// content/dev's existing TypeScript manifest and player descriptor
// carry the example's fluids, swim block and swim wiring, under its mod id.
#[test]
fn dev_manifest_declares_the_example_fluids_swim_block_and_wiring_under_its_mod_id() {
    let dev = dev_manifest();
    let twin = run_manifest("start-script.js", &bundled_ts_twin()).expect("TS twin initializes");

    assert_eq!(dev.id, "postretro.dev", "content/dev keeps its mod id");
    assert_eq!(dev.fluids, twin.fluids);
    assert_eq!(dev.fluids.names(), ["water", "sludge"]);
    assert_eq!(swim_of(&dev, "player"), swim_of(&twin, "player"));
    assert!(swim_of(&dev, "player").is_some(), "the dev player can swim");
    assert_eq!(dev.player_events, twin.player_events);
    assert!(
        twin.crossings
            .iter()
            .all(|crossing| dev.crossings.contains(crossing)),
        "content/dev carries the local splash crossing"
    );
    for reaction in &twin.reactions {
        assert!(
            dev.reactions.contains(reaction),
            "content/dev registers `{}`",
            reaction.reaction.name
        );
    }
}

// At the fixture: a negative viscosity rejects that fluid with a warning
// naming the field, in both runtimes; the rest stands.
#[test]
fn fluids_swim_twin_with_negative_water_viscosity_drops_water_naming_the_field() {
    let capture = LogCapture::start();
    let ts = run_manifest(
        "start-script.js",
        &mutate(&bundled_ts_twin(), "viscosity: 2", "viscosity: -2"),
    )
    .expect("a rejected fluid never rejects the TS manifest");
    let luau = run_manifest(
        "start-script.luau",
        &mutate(&luau_twin(), "viscosity = 2.0", "viscosity = -2.0"),
    )
    .expect("a rejected fluid never rejects the Luau manifest");

    for manifest in [&ts, &luau] {
        assert_eq!(manifest.fluids.names(), ["sludge"]);
        assert!(
            swim_of(manifest, "player").is_some(),
            "the descriptor stands"
        );
    }
    let rejections: Vec<String> = capture
        .records()
        .into_iter()
        .filter(|record| {
            record.level == log::Level::Warn
                && record
                    .message
                    .contains("fluids[0].viscosity must be a finite value >= 0")
                && record.message.contains("fluid 'water' was rejected")
        })
        .map(|record| record.message)
        .collect();
    assert_eq!(rejections.len(), 2, "one per runtime: {rejections:?}");
}

// At the fixture: a swim block whose `exitDepth` is not below `enterDepth` is
// dropped with a warning naming the field, in both runtimes; the descriptor
// and the mod load, so the player wades.
#[test]
fn fluids_swim_twin_with_exit_depth_at_enter_depth_drops_the_swim_block_naming_exit_depth() {
    let capture = LogCapture::start();
    let ts = run_manifest(
        "start-script.js",
        &mutate(&bundled_ts_twin(), "exitDepth: 0.35", "exitDepth: 0.5"),
    )
    .expect("a bad swim block never rejects the TS manifest");
    let luau = run_manifest(
        "start-script.luau",
        &mutate(&luau_twin(), "exitDepth = 0.35", "exitDepth = 0.5"),
    )
    .expect("a bad swim block never rejects the Luau manifest");
    for manifest in [&ts, &luau] {
        assert_eq!(
            swim_of(manifest, "player"),
            None,
            "the swim block is dropped"
        );
        assert_eq!(
            manifest.fluids.names(),
            ["water", "sludge"],
            "the rest of the mod stands"
        );
    }
    let dropped: Vec<String> = capture
        .records()
        .into_iter()
        .filter(|record| {
            record.level == log::Level::Warn
                && record.message.contains("`movement.swim.exitDepth`")
                && record
                    .message
                    .contains("the movement.swim block was dropped")
        })
        .map(|record| record.message)
        .collect();
    assert_eq!(dropped.len(), 2, "one per runtime: {dropped:?}");
    assert_eq!(dropped[0], dropped[1], "the runtimes' diagnostics diverged");
}

/// Every `"fluid" "<name>"` key a map authors.
pub(super) fn fluids_named_by(map: &str) -> std::collections::BTreeSet<String> {
    let source = std::fs::read_to_string(dev_content().join("maps").join(map))
        .unwrap_or_else(|error| panic!("{map} reads: {error}"));
    source
        .lines()
        .filter_map(|line| line.trim().strip_prefix("\"fluid\" \""))
        .map(|rest| rest.trim_end_matches('"').to_string())
        .collect()
}

// The playtest and stress maps load wet: the host resolves each volume's fluid
// by name against its manifest, and an undeclared name loads dry.
#[test]
fn dev_maps_name_only_fluids_the_dev_manifest_declares() {
    let dev = dev_manifest();
    for map in ["e24-environment-test.map", "stress-env-volumes.map"] {
        let named = fluids_named_by(map);
        assert_eq!(
            named.iter().map(String::as_str).collect::<Vec<_>>(),
            ["sludge", "water"],
            "{map} names both dev fluids"
        );
        for fluid in &named {
            assert!(
                dev.fluids.index_of(fluid).is_some(),
                "{map}: content/dev declares `{fluid}`"
            );
        }
    }
}
