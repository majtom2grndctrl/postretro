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
        import { defineReaction, defineStore, runtime } from "postretro";
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
          updateState(store.state.active, runtime.select(on.rising, true, false))
        );
        const crossing = onStateCrossing(
          store.state.countdown,
          { below: 0, edge: "both" },
          [toggle],
        );
        const predicate = onStateCrossing(
          runtime.le(store.state.countdown, 0),
          [toggle],
          { edge: "both" },
        );
        const unknownEdge = onStateCrossing(
          store.state.countdown,
          { above: 10, edge: "future-edge" },
          [toggle],
        );
        const value = {
          reaction: toggle,
          crossing,
          predicate,
          unknownEdge,
          schema: store.declaration.schema,
          refRead: runtime.read(store.state.countdown),
          bareRef: runtime.add(store.state.countdown, 1),
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
          return Ui.updateState(store.state.active, Postretro.runtime.select(on.rising, true, false))
        end)
        local crossing = Ui.onStateCrossing(
          store.state.countdown,
          { below = 0, edge = "both" },
          { toggle }
        )
        local predicate = Ui.onStateCrossing(
          Postretro.runtime.le(store.state.countdown, 0),
          { toggle },
          { edge = "both" }
        )
        local unknownEdge = Ui.onStateCrossing(
          store.state.countdown,
          { above = 10, edge = "future-edge" },
          { toggle }
        )
        local value = {
          reaction = toggle,
          crossing = crossing,
          predicate = predicate,
          unknownEdge = unknownEdge,
          schema = store.declaration.schema,
          refRead = Postretro.runtime.read(store.state.countdown),
          bareRef = Postretro.runtime.add(store.state.countdown, 1),
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
        import { defineImpactEvent, defineStore, slot } from "postretro";
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
                slot(counters.state.broken).add(1),
                impact.target.despawn(),
              ],
            },
          ];
        };
        const base = defineImpactEvent("salvage:crate-break", { tag: "crate", levels: ["campaign"] }, breakable);
        const override = base.override({ tag: "reinforced_crate" }, (impact) => [
          impact.target.despawn({ afterMs: 15 }),
        ]);
        const independent = defineImpactEvent("salvage:vase-break", { tag: "vase" }, (impact) => [
          { when: impact.amount.gt(0), do: [] },
          impact.target.despawn(),
        ]);
        const empty = defineImpactEvent("salvage:empty", { tag: "empty" }, () => []);
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
                Postretro.slot(counters.state.broken):add(1),
                impact.target:despawn(),
              },
            },
          }
        end
        local base = Postretro.defineImpactEvent("salvage:crate-break", { tag = "crate", levels = { "campaign" } }, breakable)
        local override = base:override({ tag = "reinforced_crate" }, function(impact)
          return { impact.target:despawn({ afterMs = 15 }) }
        end)
        local independent = Postretro.defineImpactEvent("salvage:vase-break", { tag = "vase" }, function(impact)
          return {
            { when = impact.amount:gt(0), ["do"] = {} },
            impact.target:despawn(),
          }
        end)
        local empty = Postretro.defineImpactEvent("salvage:empty", { tag = "empty" }, function()
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
        assert_eq!(base_id, "salvage:crate-break");
        assert_eq!(independent_id, "salvage:vase-break");
    }

    assert_eq!(
        typescript, luau,
        "impact ids and policy lowering must match across runtimes"
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
            "primitive": "slot.add",
            "args": {
                "slot": "impact.broken",
                "delta": { "op": "const", "value": 1 },
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
fn impact_event_builder_id_diagnostics_match_across_runtimes() {
    const TYPESCRIPT_FIXTURE: &str = r#"
        import { defineImpactEvent } from "postretro";
        let message = "";
        try { defineImpactEvent("not namespaced", {}, () => []); }
        catch (error) { message = String((error as Error).message); }
        JSON.stringify({ message });
    "#;
    const LUAU_FIXTURE: &str = r#"
        local Postretro = require("postretro")
        local ok, message = pcall(function()
          Postretro.defineImpactEvent("not namespaced", {}, function() return {} end)
        end)
        return { message = if ok then "" else tostring(message):gsub("^.-:%d+: ", "") }
    "#;

    let typescript = quickjs_fixture_value(TYPESCRIPT_FIXTURE);
    let luau = luau_fixture_value(LUAU_FIXTURE);
    assert_eq!(typescript, luau);
    assert!(
        typescript["message"]
            .as_str()
            .is_some_and(|message| message.contains("namespaced ASCII string"))
    );
}
