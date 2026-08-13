// Dual-runtime parity: the same behavior-IR expression authored in TypeScript
// (QuickJS) and Luau must produce byte-identical IR once canonicalized through
// the `IrNode` serde form. See: context/lib/scripting.md §11.
//
// Crossing path: each runtime authors the expression with the `runtime` builder
// vocabulary installed by the SDK prelude and returns the node through the
// existing `run_script` / `run_source` value path — no new collection sink.
// The QuickJS side returns `JSON.stringify(node)` (deserialized with
// `serde_json`); the Luau side returns the node table (deserialized via the
// `conv`/mlua serde bridge with `Lua::from_value`). Both land in `IrNode`, are
// re-serialized through one canonical `serde_json::to_string`, and compared.
// Canonicalizing Rust-side sidesteps cross-runtime float-formatting and
// key-order differences.

use mlua::LuaSerdeExt as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ir::IrNode;
use crate::luau::{LuauConfig, LuauSubsystem, Which, build_lua_state};
use crate::primitives_registry::PrimitiveRegistry;
use crate::quickjs::{QuickJsConfig, QuickJsSubsystem, run_script};

/// Author `expr_src` in QuickJS as a bare expression evaluating to an IR node,
/// return it as a JSON string, and canonicalize through `IrNode`.
fn quickjs_canonical(expr_src: &str) -> String {
    let registry = PrimitiveRegistry::new();
    let subsys = QuickJsSubsystem::new(&registry, &QuickJsConfig::default()).unwrap();

    let json = subsys.definition_ctx().with(|ctx| {
        let src = format!("JSON.stringify({expr_src})");
        run_script::<String>(&ctx, &src, "parity.js").expect("quickjs eval")
    });

    let node: IrNode = serde_json::from_str(&json).expect("quickjs json -> IrNode");
    serde_json::to_string(&node).expect("canonicalize quickjs node")
}

/// Author `expr_src` in Luau as a bare expression evaluating to an IR node,
/// return the table, and canonicalize through `IrNode` via the mlua serde
/// bridge.
fn luau_canonical(expr_src: &str) -> String {
    let registry = PrimitiveRegistry::new();
    let subsys = LuauSubsystem::new(&registry, &LuauConfig::default()).unwrap();

    let value: mlua::Value = subsys
        .run_source(
            Which::Definition,
            &format!("return {expr_src}"),
            "parity.luau",
        )
        .expect("luau eval");

    let node: IrNode = subsys
        .definition_lua()
        .from_value(value)
        .expect("luau value -> IrNode");
    serde_json::to_string(&node).expect("canonicalize luau node")
}

/// A short-lived authored TypeScript fixture module. The compiler consumes a
/// real file, just as `scripts-build` does for a mod entry, and the directory
/// is removed when the fixture leaves scope.
struct TypeScriptFixture {
    directory: PathBuf,
    entry: PathBuf,
}

impl TypeScriptFixture {
    fn new(source: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos() as u64)
            .unwrap_or(0);
        let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "postretro-ir-parity-{nanos}-{counter}-{}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).expect("create TypeScript parity fixture directory");

        let entry = directory.join("fixture.ts");
        fs::write(&entry, source).expect("write TypeScript parity fixture");
        let entry = fs::canonicalize(&entry).expect("canonicalize TypeScript parity fixture");

        Self { directory, entry }
    }

    fn entry(&self) -> &Path {
        &self.entry
    }
}

impl Drop for TypeScriptFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

/// Bundle and run one authored TypeScript fixture module through the production
/// `scripts-build` library path, then return its descriptor-shaped JSON.
/// Bare SDK imports are intentionally stripped by that compiler; the QuickJS
/// SDK prelude installs their matching runtime bindings before the emitted JS
/// runs.
fn quickjs_fixture_value(source: &str) -> serde_json::Value {
    let fixture = TypeScriptFixture::new(source);
    let bundled = postretro_script_compiler::bundle_entry(fixture.entry())
        .expect("TypeScript parity fixture bundles through scripts-build");

    let registry = PrimitiveRegistry::new();
    let subsys = QuickJsSubsystem::new(&registry, &QuickJsConfig::default()).unwrap();

    let json = subsys.definition_ctx().with(|ctx| {
        run_script::<String>(&ctx, &bundled, "increment-predicate.js")
            .expect("quickjs fixture eval")
    });

    serde_json::from_str(&json).expect("quickjs fixture json")
}

/// Run one authored Luau fixture and bridge its descriptor-shaped return value
/// through serde. The fixture uses the public `require("postretro/ui")`
/// module, matching the Luau authoring surface rather than internal globals.
fn luau_fixture_value(source: &str) -> serde_json::Value {
    // A mod-rooted state installs the virtual `postretro/ui` module. The
    // long-lived definition-only state deliberately leaves `require` absent.
    let lua = build_lua_state(&[], None, Some(Path::new(env!("CARGO_MANIFEST_DIR"))))
        .expect("build mod-rooted luau fixture state");
    let value: mlua::Value = lua
        .load(source)
        .set_name("increment-predicate.luau")
        .eval()
        .expect("luau fixture eval");

    lua.from_value(value).expect("luau fixture value -> json")
}

/// Asserts the TS and Luau spellings of the same expression canonicalize to
/// byte-identical IR. `expr_src` must be valid as both a JS and a Luau
/// expression — the `runtime.*` builders share an identical surface, so a
/// string using only `runtime.<op>(...)` and numeric/boolean/string literals
/// works in both.
fn assert_parity(expr_src: &str) {
    let ts = quickjs_canonical(expr_src);
    let luau = luau_canonical(expr_src);
    assert_eq!(
        ts, luau,
        "IR parity drift for `{expr_src}`:\n  ts:   {ts}\n  luau: {luau}"
    );
}

#[test]
fn nested_arithmetic_expression_is_byte_identical_across_runtimes() {
    // shield = clamp(base + charges * 10, 0, 100) authored against named inputs.
    assert_parity(
        "runtime.clamp(runtime.add(runtime.read(\"base\"), runtime.mul(runtime.read(\"charges\"), runtime.constant(10))), runtime.constant(0), runtime.constant(100))",
    );
}

#[test]
fn select_with_comparison_is_byte_identical_across_runtimes() {
    // select(speed > threshold, lerp(a, b, t), const) — exercises select,
    // comparison, lerp, const, and a boolean literal leaf.
    assert_parity(
        "runtime.select(runtime.gt(runtime.read(\"speed\"), runtime.constant(5)), runtime.lerp(runtime.constant(0), runtime.constant(1), runtime.read(\"t\")), runtime.constant(true))",
    );
}

#[test]
fn bare_literal_operands_canonicalize_to_explicit_constant_form() {
    // The literal-wrap sugar must canonicalize byte-identically to the
    // explicit-`constant` spelling — and identically across runtimes. A bare
    // `5` / `true` operand auto-wraps into `{ op: "const", value }`, the same
    // node `runtime.constant(...)` emits. Each pairing asserts both halves:
    // the sugared form equals the explicit form (within a runtime), and both
    // forms agree across runtimes (`assert_parity`).
    let explicit = "runtime.clamp(runtime.add(runtime.read(\"speed\"), runtime.constant(1)), runtime.constant(0), runtime.constant(100))";
    let sugared = "runtime.clamp(runtime.add(runtime.read(\"speed\"), 1), 0, 100)";

    assert_parity(explicit);
    assert_parity(sugared);
    assert_eq!(
        quickjs_canonical(explicit),
        quickjs_canonical(sugared),
        "bare-literal sugar diverged from explicit `constant` form (QuickJS)"
    );
    assert_eq!(
        luau_canonical(explicit),
        luau_canonical(sugared),
        "bare-literal sugar diverged from explicit `constant` form (Luau)"
    );

    // A bare boolean operand wraps the same way.
    let explicit_bool =
        "runtime.select(runtime.constant(true), runtime.constant(10), runtime.constant(20))";
    let sugared_bool = "runtime.select(true, 10, 20)";
    assert_eq!(
        quickjs_canonical(explicit_bool),
        quickjs_canonical(sugared_bool),
        "bare boolean sugar diverged from explicit `constant` form (QuickJS)"
    );
    assert_eq!(
        luau_canonical(explicit_bool),
        luau_canonical(sugared_bool),
        "bare boolean sugar diverged from explicit `constant` form (Luau)"
    );
}

#[test]
fn increment_and_predicate_crossing_fixtures_match_across_authoring_runtimes() {
    // These fixtures deliberately use the public UI authoring surfaces. The
    // TypeScript module imports the UI helpers before `scripts-build` strips
    // those bare SDK imports; Luau reads the corresponding virtual module.
    // Both author one increment reaction and one predicate crossing over the
    // same slot, exercising updateState, onStateCrossing, and literal wrapping.
    const TYPESCRIPT_FIXTURE: &str = r#"
        import { runtime } from "postretro";
        import { onStateCrossing, updateState } from "postretro/ui";

        const ref = { slot: "counter.charge" };
        const increment = updateState(
          ref,
          runtime.add(runtime.read(ref.slot), 1),
        );
        const crossing = onStateCrossing(
          runtime.ge(runtime.read(ref.slot), 2),
          ["chargeReady"],
        );
        JSON.stringify({ increment, crossing });
    "#;
    const LUAU_FIXTURE: &str = r#"
        local Ui = require("postretro/ui")
        local ref = { slot = "counter.charge" }
        local increment = Ui.updateState(
          ref,
          runtime.add(runtime.read(ref.slot), 1)
        )
        local crossing = Ui.onStateCrossing(
          runtime.ge(runtime.read(ref.slot), 2),
          { "chargeReady" }
        )
        return { increment = increment, crossing = crossing }
    "#;

    let typescript = quickjs_fixture_value(TYPESCRIPT_FIXTURE);
    let luau = luau_fixture_value(LUAU_FIXTURE);
    assert_eq!(
        typescript, luau,
        "TS and Luau increment/predicate descriptors diverged"
    );

    let expected = serde_json::json!({
        "increment": {
            "primitive": "setState",
            "args": {
                "slot": "counter.charge",
                "value": {
                    "op": "add",
                    "a": { "op": "input", "name": "counter.charge" },
                    "b": { "op": "const", "value": 1 },
                },
            },
        },
        "crossing": {
            "predicate": {
                "op": "ge",
                "a": { "op": "input", "name": "counter.charge" },
                "b": { "op": "const", "value": 2 },
            },
            "fire": ["chargeReady"],
        },
    });
    assert_eq!(
        typescript, expected,
        "fixture must emit the pinned setState/crossing wire shapes"
    );
}

#[test]
fn enemy_group_update_descriptors_match_across_authoring_runtimes() {
    // This fixture intentionally uses the public root module/bare-global
    // surfaces. In particular, the Luau spelling proves `enemies` is present
    // in DATA_SCRIPT_FIELDS and therefore lifted from data_script.luau.
    const TYPESCRIPT_FIXTURE: &str = r#"
        import { enemies } from "postretro";
        JSON.stringify(enemies({ tag: "closet_a" }).update({ aggro: true }));
    "#;
    const LUAU_FIXTURE: &str = r#"
        return enemies({ tag = "closet_a" }):update({ aggro = true })
    "#;

    let typescript = quickjs_fixture_value(TYPESCRIPT_FIXTURE);
    let luau = luau_fixture_value(LUAU_FIXTURE);
    assert_eq!(
        serde_json::to_vec(&typescript).expect("serialize TypeScript descriptor"),
        serde_json::to_vec(&luau).expect("serialize Luau descriptor"),
        "TS and Luau enemy-group descriptors diverged"
    );
    assert_eq!(
        typescript,
        serde_json::json!({
            "primitive": "updateEnemyState",
            "tag": "closet_a",
            "args": { "aggro": true },
        }),
        "enemy-group handle must be sugar for the raw primitive descriptor"
    );
}

#[test]
fn spawner_fire_descriptors_match_across_authoring_runtimes() {
    // This fixture uses the public root module/bare-global surfaces. The
    // Luau spelling proves `spawner` is lifted from data_script.luau and the
    // byte comparison pins the exact no-args descriptor contract.
    const TYPESCRIPT_FIXTURE: &str = r#"
        import { spawner } from "postretro";
        JSON.stringify(spawner({ tag: "closet_a" }).fire());
    "#;
    const LUAU_FIXTURE: &str = r#"
        return spawner({ tag = "closet_a" }):fire()
    "#;

    let typescript = quickjs_fixture_value(TYPESCRIPT_FIXTURE);
    let luau = luau_fixture_value(LUAU_FIXTURE);
    assert_eq!(
        serde_json::to_vec(&typescript).expect("serialize TypeScript descriptor"),
        serde_json::to_vec(&luau).expect("serialize Luau descriptor"),
        "TS and Luau spawner descriptors diverged"
    );
    assert_eq!(
        typescript,
        serde_json::json!({
            "primitive": "spawnFromSpawner",
            "tag": "closet_a",
        }),
        "spawner handle must be sugar for the raw primitive descriptor"
    );
}

#[test]
fn resource_reaction_builders_match_across_root_sdk_surfaces() {
    const TYPESCRIPT_FIXTURE: &str = r#"
        import { addSlot, defineStore, grantAmmo, grantHealth } from "postretro";
        const currency = defineStore("currency", {
          xp: { type: "number", default: 0, perOwner: true },
        });
        JSON.stringify({
          health: grantHealth("players", 12.5),
          ammo: grantAmmo("players", "bullets.light", 8),
          slot: addSlot("players", currency.xp, 3),
        });
    "#;
    const LUAU_FIXTURE: &str = r#"
        local Postretro = require("postretro")
        local currency = Postretro.defineStore("currency", {
          xp = { type = "number", default = 0, perOwner = true },
        })
        return {
          module = {
            health = Postretro.grantHealth("players", 12.5),
            ammo = Postretro.grantAmmo("players", "bullets.light", 8),
            slot = Postretro.addSlot("players", currency.xp, 3),
          },
          globals = {
            health = grantHealth("players", 12.5),
            ammo = grantAmmo("players", "bullets.light", 8),
            slot = addSlot("players", currency.xp, 3),
          },
        }
    "#;

    let typescript = quickjs_fixture_value(TYPESCRIPT_FIXTURE);
    let luau = luau_fixture_value(LUAU_FIXTURE);
    assert_eq!(
        typescript, luau["module"],
        "TypeScript imports and Luau `require(\"postretro\")` grants diverged"
    );
    assert_eq!(
        typescript, luau["globals"],
        "TypeScript imports and Luau bare-global grants diverged"
    );
    assert_eq!(
        typescript,
        serde_json::json!({
            "health": {
                "primitive": "grantHealth",
                "tag": "players",
                "args": { "amount": 12.5 },
            },
            "ammo": {
                "primitive": "grantAmmo",
                "tag": "players",
                "args": { "type": "bullets.light", "amount": 8 },
            },
            "slot": {
                "primitive": "addSlot",
                "tag": "players",
                "args": { "slot": "currency.xp", "delta": 3 },
            },
        })
    );
}

#[test]
fn trigger_pool_manifest_data_is_byte_identical_across_authoring_runtimes() {
    // This exercises both public root-module spellings: the level-local count
    // form and the mod-global percentage form with a map-tag selector.
    const TYPESCRIPT_FIXTURE: &str = r#"
        import { defineMod, defineTriggerPool } from "postretro";

        const level = {
          triggerPools: [defineTriggerPool({ tag: "closet_trap", arm: 2 })],
        };
        const mod = defineMod({
          name: "Trap Pools",
          triggerPools: [defineTriggerPool({
            tag: "ambush_trap",
            armPercentage: 50,
            levels: ["trap-pools"],
          })],
        });
        JSON.stringify({ level, mod });
    "#;
    const LUAU_FIXTURE: &str = r#"
        local Postretro = require("postretro")

        local level = {
          triggerPools = { Postretro.defineTriggerPool({ tag = "closet_trap", arm = 2 }) },
        }
        local mod = Postretro.defineMod({
          name = "Trap Pools",
          triggerPools = { Postretro.defineTriggerPool({
            tag = "ambush_trap",
            armPercentage = 50,
            levels = { "trap-pools" },
          }) },
        })
        return { level = level, mod = mod }
    "#;

    let typescript = quickjs_fixture_value(TYPESCRIPT_FIXTURE);
    let luau = luau_fixture_value(LUAU_FIXTURE);
    assert_eq!(
        serde_json::to_vec(&typescript).expect("serialize TypeScript manifest data"),
        serde_json::to_vec(&luau).expect("serialize Luau manifest data"),
        "TS and Luau trigger-pool manifest data diverged"
    );
    assert_eq!(
        typescript,
        serde_json::json!({
            "level": {
                "triggerPools": [{ "tag": "closet_trap", "arm": 2 }],
            },
            "mod": {
                "name": "Trap Pools",
                "triggerPools": [{
                    "tag": "ambush_trap",
                    "armPercentage": 50,
                    "levels": ["trap-pools"],
                }],
            },
        }),
        "trigger-pool builders must preserve the authored manifest data"
    );
}

#[test]
fn dispatch_tracers_accumulators_and_state_refs_match_across_runtimes() {
    const TYPESCRIPT_FIXTURE: &str = r#"
        import { defineMod, defineReaction, defineStore, runtime } from "postretro";
        import { onStateCrossing, updateState } from "postretro/ui";

        const store = defineStore("dispatch", {
          countdown: {
            type: "number",
            default: 60,
            range: [0, 60],
            accumulate: (t) => runtime.mul(t.dt, -1),
          },
          active: { type: "boolean", default: false },
        });
        const toggle = defineReaction("toggle", (on) =>
          updateState(store.active, runtime.select(on.rising, true, false))
        );
        const crossing = onStateCrossing(
          store.countdown,
          { below: 0, edge: "both" },
          [toggle],
        );
        const predicate = onStateCrossing(
          runtime.le(store.countdown, 0),
          [toggle],
          { edge: "both" },
        );
        const unknownEdge = onStateCrossing(
          store.countdown,
          { above: 10, edge: "future-edge" },
          [toggle],
        );
        const manifest = defineMod({ stores: [store] });
        const value = {
          reaction: toggle,
          crossing,
          predicate,
          unknownEdge,
          schema: manifest.stores[0].schema,
          refRead: runtime.read(store.countdown),
          bareRef: runtime.add(store.countdown, 1),
        };
        const roundTrip = JSON.parse(JSON.stringify(value));
        JSON.stringify({ value, roundTrip, hasFunction: typeof toggle.tracer === "function" });
    "#;
    const LUAU_FIXTURE: &str = r#"
        local Postretro = require("postretro")
        local Ui = require("postretro/ui")
        local store = Postretro.defineStore("dispatch", {
          countdown = {
            type = "number",
            default = 60,
            range = { 0, 60 },
            accumulate = function(t)
              return Postretro.runtime.mul(t.dt, -1)
            end,
          },
          active = { type = "boolean", default = false },
        })
        local toggle = Postretro.defineReaction("toggle", function(on)
          return Ui.updateState(store.active, Postretro.runtime.select(on.rising, true, false))
        end)
        local crossing = Ui.onStateCrossing(
          store.countdown,
          { below = 0, edge = "both" },
          { toggle }
        )
        local predicate = Ui.onStateCrossing(
          Postretro.runtime.le(store.countdown, 0),
          { toggle },
          { edge = "both" }
        )
        local unknownEdge = Ui.onStateCrossing(
          store.countdown,
          { above = 10, edge = "future-edge" },
          { toggle }
        )
        local manifest = Postretro.defineMod({ stores = { store } })
        local value = {
          reaction = toggle,
          crossing = crossing,
          predicate = predicate,
          unknownEdge = unknownEdge,
          schema = manifest.stores[1].schema,
          refRead = Postretro.runtime.read(store.countdown),
          bareRef = Postretro.runtime.add(store.countdown, 1),
        }
        return { value = value, roundTrip = value, hasFunction = false }
    "#;

    let typescript = quickjs_fixture_value(TYPESCRIPT_FIXTURE);
    let luau = luau_fixture_value(LUAU_FIXTURE);
    assert_eq!(typescript, luau, "dispatch authoring surfaces diverged");
    assert_eq!(typescript["value"], typescript["roundTrip"]);
    assert_eq!(typescript["hasFunction"], false);
    assert_eq!(typescript["value"]["unknownEdge"]["edge"], "future-edge");
    assert_eq!(
        typescript["value"]["reaction"]["args"]["value"]["cond"],
        serde_json::json!({ "op": "input", "name": "@rising" })
    );
    assert_eq!(
        typescript["value"]["schema"]["countdown"]["accumulate"],
        serde_json::json!({
            "op": "mul",
            "a": { "op": "input", "name": "@dt" },
            "b": { "op": "const", "value": -1 },
        })
    );
    assert_eq!(
        typescript["value"]["refRead"],
        typescript["value"]["bareRef"]["a"]
    );
}

#[test]
fn crossing_fire_rejects_map_shaped_and_sparse_sequences_across_runtimes() {
    // Regression: the Luau SDK used `ipairs`, silently accepting map-shaped
    // and sparse `fire` tables while the TypeScript SDK rejected non-arrays.
    // Both authoring surfaces must reject malformed fire sequences before a
    // crossing descriptor can be produced.
    const TYPESCRIPT_FIXTURE: &str = r#"
        import { runtime } from "postretro";
        import { onStateCrossing } from "postretro/ui";

        const ref = { slot: "counter.charge" };
        const message = (call) => {
          try { call(); return null; } catch (error) { return String(error); }
        };
        JSON.stringify({
          predicateMap: message(() => onStateCrossing(runtime.constant(true), { ready: "chargeReady" })),
          predicateSparse: message(() => onStateCrossing(runtime.constant(true), ["chargeReady", , "other"])),
          thresholdMap: message(() => onStateCrossing(ref, { above: 2 }, { ready: "chargeReady" })),
          thresholdSparse: message(() => onStateCrossing(ref, { above: 2 }, ["chargeReady", , "other"])),
        });
    "#;
    const LUAU_FIXTURE: &str = r#"
        local Ui = require("postretro/ui")
        local ref = { slot = "counter.charge" }
        local function message(call)
          local ok, value = pcall(call)
          if ok then
            return nil
          end
          return tostring(value)
        end
        return {
          predicateMap = message(function()
            return Ui.onStateCrossing(runtime.constant(true), { ready = "chargeReady" })
          end),
          predicateSparse = message(function()
            return Ui.onStateCrossing(runtime.constant(true), { "chargeReady", [3] = "other" })
          end),
          thresholdMap = message(function()
            return Ui.onStateCrossing(ref, { above = 2 }, { ready = "chargeReady" })
          end),
          thresholdSparse = message(function()
            return Ui.onStateCrossing(ref, { above = 2 }, { "chargeReady", [3] = "other" })
          end),
        }
    "#;

    for (runtime, values) in [
        ("TypeScript", quickjs_fixture_value(TYPESCRIPT_FIXTURE)),
        ("Luau", luau_fixture_value(LUAU_FIXTURE)),
    ] {
        for key in [
            "predicateMap",
            "predicateSparse",
            "thresholdMap",
            "thresholdSparse",
        ] {
            let message = values
                .get(key)
                .and_then(serde_json::Value::as_str)
                .unwrap_or_else(|| {
                    panic!("{runtime} accepted malformed crossing `fire` sequence: {key}")
                });
            assert!(
                message.contains("onStateCrossing"),
                "{runtime} rejected {key} unclearly: {message}"
            );
        }
    }
}

#[test]
fn every_opcode_round_trips_identically_across_runtimes() {
    // One assertion per opcode so a divergence names the offending builder.
    let n = "runtime.constant(1)";
    let leaves = format!("{n}, {n}");
    for expr in [
        "runtime.constant(3.5)".to_string(),
        "runtime.constant(true)".to_string(),
        "runtime.read(\"speed\")".to_string(),
        format!("runtime.add({leaves})"),
        format!("runtime.sub({leaves})"),
        format!("runtime.mul({leaves})"),
        format!("runtime.div({leaves})"),
        format!("runtime.clamp({n}, {n}, {n})"),
        format!("runtime.lerp({n}, {n}, {n})"),
        format!("runtime.lt({leaves})"),
        format!("runtime.le({leaves})"),
        format!("runtime.gt({leaves})"),
        format!("runtime.ge({leaves})"),
        format!("runtime.eq({leaves})"),
        format!("runtime.ne({leaves})"),
        format!("runtime.select(runtime.constant(true), {n}, {n})"),
    ] {
        assert_parity(&expr);
    }
}

#[test]
fn impact_policy_sdk_lowering_matches_across_authoring_runtimes() {
    // Exercise the shipped root SDK rather than raw runtime builders. The
    // assertion pins every cross-task leaf/token spelling and proves the
    // boolean sugar introduces only `select` nodes.
    const TYPESCRIPT_FIXTURE: &str = r#"
        import { defineImpactEvent, defineStore, update } from "postretro";
        import type { Impact, ImpactEvent } from "postretro";

        const counters = defineStore("impact", {
          broken: { type: "number", default: 0 },
        });
        const breakable = (impact: Impact) => {
          const hits = impact.target.state("hits");
          return [
            impact.target.setState("hits", hits.plus(1)),
            {
              when: hits.eq(2).and(impact.amount.gt(0).not()),
              do: [
                impact.target.setHealth(
                  impact.target.healthAfter.clamp(0, impact.target.maxHealth),
                  { afterMs: 30 },
                ),
                impact.source.grantHealth(impact.amount.plus(2)),
                impact.source.grantAmmo("cells", hits.plus(3)),
                impact.target.playAnim("shatter"),
                update(counters.broken, (current) => current.plus(1)),
                impact.target.despawn(),
              ],
            },
          ];
        };
        const base = defineImpactEvent("crate-break", { tag: "crate", levels: ["campaign"] }, breakable);
        const override = base.override({ tag: "reinforced_crate" }, (impact) => [
          impact.target.despawn({ afterMs: 15 }),
        ]);
        const independent = defineImpactEvent("vase-break", { tag: "vase" }, (impact) => [
          { when: impact.amount.gt(0), do: [] },
          impact.target.despawn(),
        ]);
        const empty = defineImpactEvent("empty", { tag: "empty" }, () => []);
        const wire = (event: ImpactEvent) => {
          const descriptor = event as unknown as {
            kind: "impact";
            id: string;
            isOverride: boolean;
            filter: { tag?: string };
            policy: unknown[];
            levels?: string[];
          };
          return {
            kind: descriptor.kind,
            id: descriptor.id,
            isOverride: descriptor.isOverride,
            filter: descriptor.filter,
            policy: descriptor.policy,
            levels: descriptor.levels,
          };
        };
        JSON.stringify({ base: wire(base), override: wire(override), independent: wire(independent), empty: wire(empty) });
    "#;
    const LUAU_FIXTURE: &str = r#"
        local Postretro = require("postretro")

        local counters = Postretro.defineStore("impact", {
          broken = { type = "number", default = 0 },
        })
        local function breakable(impact)
          local hits = impact.target:state("hits")
          local threshold = hits:eq(2)
          local positive = impact.amount:gt(0)
          return {
            impact.target:setState("hits", hits:plus(1)),
            {
              when = threshold["and"](threshold, positive["not"](positive)),
              ["do"] = {
                impact.target:setHealth(
                  impact.target.healthAfter:clamp(0, impact.target.maxHealth),
                  { afterMs = 30 }
                ),
                impact.source:grantHealth(impact.amount:plus(2)),
                impact.source:grantAmmo("cells", hits:plus(3)),
                impact.target:playAnim("shatter"),
                Postretro.update(counters.broken, function(current)
                  return current:plus(1)
                end),
                impact.target:despawn(),
              },
            },
          }
        end
        local base = Postretro.defineImpactEvent("crate-break", { tag = "crate", levels = { "campaign" } }, breakable)
        local override = base:override({ tag = "reinforced_crate" }, function(impact)
          return { impact.target:despawn({ afterMs = 15 }) }
        end)
        local independent = Postretro.defineImpactEvent("vase-break", { tag = "vase" }, function(impact)
          return {
            { when = impact.amount:gt(0), ["do"] = {} },
            impact.target:despawn(),
          }
        end)
        local empty = Postretro.defineImpactEvent("empty", { tag = "empty" }, function()
          return {}
        end)
        local function wire(event)
          return {
            kind = event.kind,
            id = event.id,
            isOverride = event.isOverride,
            filter = event.filter,
            policy = event.policy,
            levels = event.levels,
          }
        end
        return { base = wire(base), override = wire(override), independent = wire(independent), empty = wire(empty) }
    "#;

    let typescript = quickjs_fixture_value(TYPESCRIPT_FIXTURE);
    let luau = luau_fixture_value(LUAU_FIXTURE);

    for values in [&typescript, &luau] {
        let base_id = values["base"]["id"].as_str().expect("base impact id");
        let override_id = values["override"]["id"]
            .as_str()
            .expect("override impact id");
        let independent_id = values["independent"]["id"]
            .as_str()
            .expect("independent impact id");
        assert_eq!(
            base_id, override_id,
            "override must retain its base identity"
        );
        assert_ne!(
            base_id, independent_id,
            "independent author ids must remain distinct"
        );
        assert_eq!(base_id, "crate-break");
        assert_eq!(independent_id, "vase-break");
    }

    assert_eq!(
        serde_json::to_vec(&typescript).expect("serialize TypeScript wire"),
        serde_json::to_vec(&luau).expect("serialize Luau wire"),
        "impact ids and policy lowering must be byte-identical across runtimes"
    );

    let base = &typescript["base"];
    assert_eq!(base["kind"], "impact");
    assert_eq!(base["isOverride"], false);
    assert_eq!(typescript["override"]["isOverride"], true);
    assert_eq!(base["filter"], serde_json::json!({ "tag": "crate" }));
    assert_eq!(base["levels"], serde_json::json!(["campaign"]));
    assert_eq!(
        base["policy"][0],
        serde_json::json!({
            "primitive": "setState",
            "target": "@impact.target",
            "args": {
                "name": "hits",
                "value": {
                    "op": "add",
                    "a": { "op": "input", "name": "@state.hits" },
                    "b": { "op": "const", "value": 1 },
                },
            },
        })
    );
    assert_eq!(
        base["policy"][1]["when"],
        serde_json::json!({
            "op": "select",
            "cond": {
                "op": "eq",
                "a": { "op": "input", "name": "@state.hits" },
                "b": { "op": "const", "value": 2 },
            },
            "a": {
                "op": "select",
                "cond": {
                    "op": "gt",
                    "a": { "op": "input", "name": "@impact.amount" },
                    "b": { "op": "const", "value": 0 },
                },
                "a": { "op": "const", "value": false },
                "b": { "op": "const", "value": true },
            },
            "b": { "op": "const", "value": false },
        })
    );
    assert_eq!(
        base["policy"][1]["do"][0],
        serde_json::json!({
            "primitive": "setHealth",
            "target": "@impact.target",
            "args": {
                "value": {
                    "op": "clamp",
                    "x": { "op": "input", "name": "@impact.healthAfter" },
                    "lo": { "op": "const", "value": 0 },
                    "hi": { "op": "input", "name": "@impact.maxHealth" },
                },
                "afterMs": 30,
            },
        })
    );
    assert_eq!(
        base["policy"][1]["do"][1],
        serde_json::json!({
            "primitive": "grantHealth",
            "target": "@impact.source",
            "args": {
                "amount": {
                    "op": "add",
                    "a": { "op": "input", "name": "@impact.amount" },
                    "b": { "op": "const", "value": 2 },
                },
            },
        })
    );
    assert_eq!(
        base["policy"][1]["do"][2],
        serde_json::json!({
            "primitive": "grantAmmo",
            "target": "@impact.source",
            "args": {
                "type": "cells",
                "amount": {
                    "op": "add",
                    "a": { "op": "input", "name": "@state.hits" },
                    "b": { "op": "const", "value": 3 },
                },
            },
        })
    );
    assert_eq!(
        base["policy"][1]["do"][4],
        serde_json::json!({
            "primitive": "slot.set",
            "args": {
                "slot": "impact.broken",
                "value": {
                    "op": "add",
                    "a": { "op": "input", "name": "impact.broken" },
                    "b": { "op": "const", "value": 1 },
                },
            },
        })
    );
    assert_eq!(
        typescript["independent"]["policy"][0]["do"],
        serde_json::json!([]),
        "an empty gated group must stay an array in both SDKs"
    );
    assert_eq!(
        typescript["independent"]["policy"][1]["primitive"], "despawn",
        "a valid sibling effect must survive an empty group"
    );
    assert_eq!(
        typescript["independent"]["policy"][1]["args"],
        serde_json::json!({}),
        "an empty effect-argument map must not be normalized into an array"
    );
    assert_eq!(
        typescript["empty"]["policy"],
        serde_json::json!([]),
        "an empty impact policy must stay an array in both SDKs"
    );
}

#[test]
fn impact_expression_algebra_update_and_when_match_across_authoring_runtimes() {
    // The public bridges must accept raw runtime nodes as fluent operands and
    // lift a runtime predicate into BoolRef before `when` lowers it. The first
    // group names its condition; the second inlines the identical expression.
    const TYPESCRIPT_FIXTURE: &str = r#"
        import { defineImpactEvent, defineStore, fromRuntime, read, runtime, update, when } from "postretro";

        const counters = defineStore("impact", {
          count: { type: "number", default: 0 },
          enabled: { type: "boolean", default: false },
        });
        const namedGate = fromRuntime.bool(runtime.gt(runtime.read("impact.bonus"), 0))
          .and(read(counters.enabled));
        const event = defineImpactEvent("runtime-bridge", { tag: "crate" }, (impact) => {
          const rawRuntimeNumber = runtime.add(runtime.read("impact.bonus"), 1);
          return [
            when(namedGate, [
              update(counters.count, (cur) => cur.plus(impact.amount.plus(rawRuntimeNumber))),
            ]),
            when(
              fromRuntime.bool(runtime.gt(runtime.read("impact.bonus"), 0))
                .and(read(counters.enabled)),
              [],
            ),
          ];
        });
        JSON.stringify({ policy: (event as unknown as { policy: unknown[] }).policy });
    "#;
    const LUAU_FIXTURE: &str = r#"
        local Postretro = require("postretro")

        local counters = Postretro.defineStore("impact", {
          count = { type = "number", default = 0 },
          enabled = { type = "boolean", default = false },
        })
        local namedGate = Postretro.fromRuntime.bool(runtime.gt(runtime.read("impact.bonus"), 0))
        namedGate = namedGate["and"](namedGate, Postretro.read(counters.enabled))
        local event = Postretro.defineImpactEvent("runtime-bridge", { tag = "crate" }, function(impact)
          local rawRuntimeNumber = runtime.add(runtime.read("impact.bonus"), 1)
          local inlineGate = Postretro.fromRuntime.bool(runtime.gt(runtime.read("impact.bonus"), 0))
          return {
            Postretro.when(namedGate, {
              Postretro.update(counters.count, function(cur)
                return cur:plus(impact.amount:plus(rawRuntimeNumber))
              end),
            }),
            Postretro.when(
              inlineGate["and"](
                inlineGate,
                Postretro.read(counters.enabled)
              ),
              {}
            ),
          }
        end)
        return { policy = event.policy }
    "#;

    let typescript = quickjs_fixture_value(TYPESCRIPT_FIXTURE);
    let luau = luau_fixture_value(LUAU_FIXTURE);
    assert_eq!(
        typescript, luau,
        "expression-algebra lowering must match across runtimes"
    );

    let policy = &typescript["policy"];
    assert_eq!(
        policy[0]["when"], policy[1]["when"],
        "named and inline `when` conditions must lower identically"
    );
    assert_eq!(
        policy[0]["do"][0],
        serde_json::json!({
            "primitive": "slot.set",
            "args": {
                "slot": "impact.count",
                "value": {
                    "op": "add",
                    "a": { "op": "input", "name": "impact.count" },
                    "b": {
                        "op": "add",
                        "a": { "op": "input", "name": "@impact.amount" },
                        "b": {
                            "op": "add",
                            "a": { "op": "input", "name": "impact.bonus" },
                            "b": { "op": "const", "value": 1 },
                        },
                    },
                },
            },
        }),
        "update must lower directly to slot.set with its current-value input inlined"
    );
}

#[test]
fn state_convergence_sdk_wire_is_byte_identical_across_authoring_runtimes() {
    // This fixture exercises the converged surface through its public runtime
    // modules. `state` and `declaration` are deliberate slot names: flattening
    // must not reserve either name on the store handle. The returned manifest
    // and impact policy are the FFI wire, so SDK-only ref `kind` tags cannot
    // appear anywhere in the serialized result.
    const TYPESCRIPT_FIXTURE: &str = r#"
        import {
          defineImpactEvent,
          defineMod,
          defineStore,
          fromRuntime,
          read,
          runtime,
          set,
          update,
          when,
        } from "postretro";
        import { bindState } from "postretro/ui";

        const store = defineStore("converged", {
          count: { type: "number", default: 0 },
          enabled: { type: "boolean", default: true },
          state: { type: "number", default: 0 },
          declaration: { type: "number", default: 0 },
        });
        const manifest = defineMod({
          name: "Converged",
          id: "converged",
          version: "1",
          stores: [store],
        });
        const runtimeGate = fromRuntime.bool(runtime.gt(runtime.read("impact.bonus"), 0));
        const gate = runtimeGate.and(read(store.enabled));
        const boundCount = bindState(store.count, { format: "Count {}" });
        const kindMutationRejected = !Reflect.set(boundCount as object, "kind", "boolean");
        const kindPreserved = boundCount.kind === "number";
        const event = defineImpactEvent("converged", { tag: "crate" }, () => [
          set(store.count, read(boundCount).plus(5)),
          update(store.count, (current) => current.plus(1)),
          when(gate, [set(store.state, read(store.declaration).plus(1))]),
          when(fromRuntime.bool(runtime.constant(false)), []),
        ]);
        JSON.stringify({
          bind: boundCount,
          kindMutationRejected,
          kindPreserved,
          slots: {
            count: store.count.slot,
            enabled: store.enabled.slot,
            state: store.state.slot,
            declaration: store.declaration.slot,
          },
          stores: manifest.stores,
          policy: (event as unknown as { policy: unknown[] }).policy,
        });
    "#;
    const LUAU_FIXTURE: &str = r#"
        local Postretro = require("postretro")
        local UI = require("postretro/ui")

        local store = Postretro.defineStore("converged", {
          count = { type = "number", default = 0 },
          enabled = { type = "boolean", default = true },
          state = { type = "number", default = 0 },
          declaration = { type = "number", default = 0 },
        })
        local manifest = Postretro.defineMod({
          name = "Converged",
          id = "converged",
          version = "1",
          stores = { store },
        })
        local runtimeGate = Postretro.fromRuntime.bool(runtime.gt(runtime.read("impact.bonus"), 0))
        local gate = runtimeGate["and"](runtimeGate, Postretro.read(store.enabled))
        local boundCount = UI.bindState(store.count, { format = "Count {}" })
        local kindMutationRejected = not pcall(function()
          boundCount.kind = "boolean"
        end)
        local kindPreserved = boundCount.kind == "number"
        local event = Postretro.defineImpactEvent("converged", { tag = "crate" }, function()
          return {
            Postretro.set(store.count, Postretro.read(boundCount):plus(5)),
            Postretro.update(store.count, function(current)
              return current:plus(1)
            end),
            Postretro.when(gate, {
              Postretro.set(store.state, Postretro.read(store.declaration):plus(1)),
            }),
            Postretro.when(Postretro.fromRuntime.bool(runtime.constant(false)), {}),
          }
        end)
        return {
          bind = boundCount,
          kindMutationRejected = kindMutationRejected,
          kindPreserved = kindPreserved,
          slots = {
            count = store.count.slot,
            enabled = store.enabled.slot,
            state = store.state.slot,
            declaration = store.declaration.slot,
          },
          stores = manifest.stores,
          policy = event.policy,
        }
    "#;

    let typescript = quickjs_fixture_value(TYPESCRIPT_FIXTURE);
    let luau = luau_fixture_value(LUAU_FIXTURE);
    let typescript_wire = serde_json::to_vec(&typescript).expect("serialize TypeScript wire");
    let luau_wire = serde_json::to_vec(&luau).expect("serialize Luau wire");

    assert_eq!(
        typescript_wire, luau_wire,
        "flatten/read/set/update/when/runtime bridge wire diverged across runtimes"
    );
    assert!(
        !String::from_utf8(typescript_wire)
            .expect("wire JSON is UTF-8")
            .contains("\"kind\""),
        "SDK-only ref kind must not cross the descriptor wire",
    );
    assert_eq!(
        typescript["bind"],
        serde_json::json!({ "slot": "converged.count", "format": "Count {}" }),
        "bindState must retain kind for SDK composition without serializing it",
    );
    assert_eq!(
        typescript["kindMutationRejected"],
        serde_json::json!(true),
        "bindState kind metadata must reject author mutation",
    );
    assert_eq!(
        typescript["kindPreserved"],
        serde_json::json!(true),
        "rejected kind mutation must preserve numeric expression lowering",
    );
    assert_eq!(
        typescript["policy"][0]["args"]["value"],
        serde_json::json!({
            "op": "add",
            "a": { "op": "input", "name": "converged.count" },
            "b": { "op": "const", "value": 5 },
        }),
        "read(bindState(numberRef)) must stay in the numeric fluent algebra",
    );
    assert_eq!(
        typescript["slots"],
        serde_json::json!({
            "count": "converged.count",
            "enabled": "converged.enabled",
            "state": "converged.state",
            "declaration": "converged.declaration",
        }),
        "flattened handles must expose schema fields directly, including state/declaration",
    );
    assert_eq!(
        typescript["stores"],
        serde_json::json!([{
            "namespace": "converged",
            "schema": {
                "count": { "type": "number", "default": 0 },
                "enabled": { "type": "boolean", "default": true },
                "state": { "type": "number", "default": 0 },
                "declaration": { "type": "number", "default": 0 },
            },
        }]),
        "defineMod must resolve the handle to the unchanged store declaration wire",
    );
    assert_eq!(
        typescript["policy"],
        serde_json::json!([
            {
                "primitive": "slot.set",
                "args": {
                    "slot": "converged.count",
                    "value": {
                        "op": "add",
                        "a": { "op": "input", "name": "converged.count" },
                        "b": { "op": "const", "value": 5 },
                    },
                },
            },
            {
                "primitive": "slot.set",
                "args": {
                    "slot": "converged.count",
                    "value": {
                        "op": "add",
                        "a": { "op": "input", "name": "converged.count" },
                        "b": { "op": "const", "value": 1 },
                    },
                },
            },
            {
                "when": {
                    "op": "select",
                    "cond": {
                        "op": "gt",
                        "a": { "op": "input", "name": "impact.bonus" },
                        "b": { "op": "const", "value": 0 },
                    },
                    "a": { "op": "input", "name": "converged.enabled" },
                    "b": { "op": "const", "value": false },
                },
                "do": [{
                    "primitive": "slot.set",
                    "args": {
                        "slot": "converged.state",
                        "value": {
                            "op": "add",
                            "a": { "op": "input", "name": "converged.declaration" },
                            "b": { "op": "const", "value": 1 },
                        },
                    },
                }],
            },
            {
                "when": { "op": "const", "value": false },
                "do": [],
            },
        ]),
        "converged builders must lower only through slot.set and the shipped IR",
    );
}

#[test]
fn per_owner_store_refs_are_addressable_without_leaking_owner_metadata() {
    // `byPlayer` is SDK metadata, not declaration or widget wire. Both authoring
    // runtimes therefore expose it without changing the enumerable `{ slot,
    // kind }` shape of a bare ref or the `{ slot }` output of bindState.
    const TYPESCRIPT_FIXTURE: &str = r#"
        import { defineImpactEvent, defineStore, read, runtime, set } from "postretro";
        import { bindState } from "postretro/ui";

        const store = defineStore("currency", {
          credits: { type: "number", default: 0, perOwner: true, network: "ownerPrivate" },
          shared: { type: "number", default: 0, network: "shared" },
        });
        let owned: ReturnType<typeof store.credits.byPlayer> | undefined;
        let fresh = false;
        let globalRejected = false;
        const event = defineImpactEvent("award-credits", { tag: "pickup" }, (impact) => {
          const first = store.credits.byPlayer(impact.source);
          const second = store.credits.byPlayer(impact.source);
          owned = first;
          fresh = first !== second;
          try {
            store.shared.byPlayer(impact.source);
          } catch (_error) {
            globalRejected = true;
          }
          return [set(store.shared, read(first)), set(first, 5)];
        });
        if (owned === undefined) throw new Error("impact callback did not run");
        JSON.stringify({
          bareKeys: Object.keys(store.credits).sort(),
          byPlayerEnumerable: Object.getOwnPropertyDescriptor(store.credits, "byPlayer")?.enumerable === true,
          ownedKeys: Object.keys(owned).sort(),
          ownedFrozen: Object.isFrozen(owned),
          owner: owned.owner,
          fresh,
          globalRejected,
          bind: bindState(owned),
          read: (event as any).policy[0].args.value,
          ownerWrite: (event as any).policy[1],
          runtimeRead: runtime.read(owned),
        });
    "#;
    const LUAU_FIXTURE: &str = r#"
        local Postretro = require("postretro")
        local UI = require("postretro/ui")

        local store = Postretro.defineStore("currency", {
          credits = { type = "number", default = 0, perOwner = true, network = "ownerPrivate" },
          shared = { type = "number", default = 0, network = "shared" },
        })
        local owned: any = nil
        local fresh = false
        local globalRejected = false
        local event = Postretro.defineImpactEvent("award-credits", { tag = "pickup" }, function(impact)
          local first = store.credits:byPlayer(impact.source)
          local second = store.credits:byPlayer(impact.source)
          owned = first
          fresh = first ~= second
          globalRejected = not pcall(function()
            store.shared:byPlayer(impact.source)
          end)
          return { Postretro.set(store.shared, Postretro.read(first)), Postretro.set(first, 5) }
        end)
        assert(owned ~= nil)
        local bareKeys = {}
        for key in pairs(store.credits) do
          table.insert(bareKeys, key)
        end
        table.sort(bareKeys)
        local ownedKeys = {}
        for key in pairs(owned) do
          table.insert(ownedKeys, key)
        end
        table.sort(ownedKeys)
        local ownedFrozen = not pcall(function()
          owned.owner = "changed"
        end)
        return {
          bareKeys = bareKeys,
          byPlayerEnumerable = false,
          ownedKeys = ownedKeys,
          ownedFrozen = ownedFrozen,
          owner = owned.owner,
          fresh = fresh,
          globalRejected = globalRejected,
          bind = UI.bindState(owned),
          read = event.policy[1].args.value,
          ownerWrite = event.policy[2],
          runtimeRead = runtime.read(owned),
        }
    "#;

    let typescript = quickjs_fixture_value(TYPESCRIPT_FIXTURE);
    let luau = luau_fixture_value(LUAU_FIXTURE);
    assert_eq!(
        typescript, luau,
        "per-owner ref surface drifted across runtimes"
    );
    assert_eq!(
        typescript,
        serde_json::json!({
            "bareKeys": ["kind", "slot"],
            "byPlayerEnumerable": false,
            "ownedKeys": ["kind", "owner", "slot"],
            "ownedFrozen": true,
            "owner": "@impact.source",
            "fresh": true,
            "globalRejected": true,
            "bind": { "slot": "currency.credits" },
            "read": { "op": "input", "name": "currency.credits", "owner": "@impact.source" },
            "ownerWrite": {
                "primitive": "slot.set",
                "target": "@impact.source",
                "args": {
                    "slot": "currency.credits",
                    "value": { "op": "const", "value": 5 },
                },
            },
            "runtimeRead": { "op": "input", "name": "currency.credits", "owner": "@impact.source" },
        }),
        "byPlayer must produce fresh frozen owner refs without leaking metadata into wire consumers",
    );
}

#[test]
fn authored_name_validation_diagnostics_match_across_runtimes() {
    const TYPESCRIPT_FIXTURE: &str = r#"
        import { defineImpactEvent, defineStore } from "postretro";
        const message = (action: () => unknown) => {
          try { action(); return ""; }
          catch (error) { return String((error as Error).message); }
        };
        const computedStoreName = "computed-store";
        const computedImpactId = "computed-impact";
        JSON.stringify({
          impactColon: message(() => defineImpactEvent("has:colon", {}, () => [])),
          impactTooLong: message(() => defineImpactEvent("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", {}, () => [])),
          storeNamespaceColon: message(() => defineStore("has:colon", {})),
          storeSlotColon: message(() => defineStore("store", { "has:colon": { type: "number", default: 0 } })),
          storeBindingSugar: message(() => defineStore({ value: { type: "number", default: 0 } })),
          impactBindingSugar: message(() => defineImpactEvent({}, () => [])),
          computedStoreName: message(() => defineStore(computedStoreName, {})),
          computedImpactId: message(() => defineImpactEvent(computedImpactId, {}, () => [])),
        });
    "#;
    const LUAU_FIXTURE: &str = r#"
        local Postretro = require("postretro")
        local function message(action)
          local ok, value = pcall(action)
          return if ok then "" else tostring(value):gsub("^.-:%d+: ", "")
        end
        local computedStoreName = "computed-store"
        local computedImpactId = "computed-impact"
        return {
          impactColon = message(function() Postretro.defineImpactEvent("has:colon", {}, function() return {} end) end),
          impactTooLong = message(function() Postretro.defineImpactEvent("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", {}, function() return {} end) end),
          storeNamespaceColon = message(function() Postretro.defineStore("has:colon", {}) end),
          storeSlotColon = message(function() Postretro.defineStore("store", { ["has:colon"] = { type = "number", default = 0 } }) end),
          storeBindingSugar = message(function() Postretro.defineStore({ value = { type = "number", default = 0 } }) end),
          impactBindingSugar = message(function() Postretro.defineImpactEvent({}, function() return {} end) end),
          computedStoreName = message(function() Postretro.defineStore(computedStoreName, {}) end),
          computedImpactId = message(function() Postretro.defineImpactEvent(computedImpactId, {}, function() return {} end) end),
        }
    "#;

    let typescript = quickjs_fixture_value(TYPESCRIPT_FIXTURE);
    let luau = luau_fixture_value(LUAU_FIXTURE);
    assert_eq!(typescript, luau);
    assert_eq!(
        typescript["impactColon"],
        "impact-event `id` must be a single ASCII segment using only [A-Za-z0-9_.-], at most 64 bytes; the engine prefixes the mod id"
    );
    assert_eq!(
        typescript["impactColon"], typescript["impactTooLong"],
        "colon-bearing and oversized authored ids share one diagnostic"
    );
    assert_eq!(
        typescript["storeBindingSugar"],
        "defineStore/defineImpactEvent without an explicit name is binding-name sugar and must be used in a direct top-level binding declaration"
    );
    assert_eq!(
        typescript["storeBindingSugar"], typescript["impactBindingSugar"],
        "name-less store and impact declarations share one diagnostic"
    );
    assert_eq!(typescript["computedStoreName"], "");
    assert_eq!(typescript["computedImpactId"], "");
}
