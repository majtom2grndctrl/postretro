// The statuses Scripting surface example (content/dev/scripts/
// statuses-demo.{ts,luau}) run as mod manifests through the production
// script runtime, then driven through the fixed tick: each of `burning`,
// `scalding`, `corroding` and `wet` behaves as the example declares it, in
// both runtimes, and its igniting policy's `impact.status(burning)` guard
// lets a burn expire.
// See: docs/scripting-reference.md (Statuses) · context/lib/scripting.md §2

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use glam::Vec3;
use postretro_entities::components::health::{
    DamageContext, DamageProducer, apply_damage_with_context,
};
use postretro_entities::components::statuses::StatusApplier;
use postretro_entities::{EntityId, EntityRegistry, NamedReaction, ScriptCtx};
use postretro_foundation::DamagePayload;
use postretro_scripting_core::primitives_registry::PrimitiveRegistry;
use postretro_scripting_core::runtime::{ModManifestResult, ScriptRuntime, ScriptRuntimeConfig};

use super::status_fluid_tests::{IN_LAVA, pools};
use super::status_tick_tests::{World, spawn};
use super::teleport_tick_tests::{Rig, classic};
use crate::impact_policy::ImpactPolicyRuntime;

fn dev_scripts() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/dev/scripts")
}

/// Run `source` as a mod root's start script through a fresh runtime, as
/// mod init does at boot.
fn run_manifest(file_name: &str, source: &str) -> ModManifestResult {
    let ctx = ScriptCtx::new();
    let mut primitives = PrimitiveRegistry::new();
    crate::scripting::primitives::register_all(&mut primitives, ctx.clone());
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
        .unwrap_or_else(|error| panic!("{file_name} initializes: {error}"));
    runtime
        .mod_manifest()
        .expect("mod init commits a manifest")
        .clone()
}

/// The example's twins as start scripts: the TS twin bundled as a debug
/// launch compiles a start script, and the Luau twin as written.
fn sources() -> [(&'static str, &'static str, String); 2] {
    let ts = postretro_script_compiler::bundle_entry(&dev_scripts().join("statuses-demo.ts"))
        .expect("statuses-demo.ts bundles");
    let luau = std::fs::read_to_string(dev_scripts().join("statuses-demo.luau"))
        .expect("statuses-demo.luau reads");
    [
        ("TypeScript", "start-script.js", ts),
        ("Luau", "start-script.luau", luau),
    ]
}

/// The example in each runtime.
fn manifests() -> Vec<(&'static str, ModManifestResult)> {
    sources()
        .into_iter()
        .map(|(runtime, file, source)| (runtime, run_manifest(file, &source)))
        .collect()
}

/// Replace exactly one occurrence, so a mutation that no longer matches the
/// twin fails loudly instead of testing the unmutated manifest.
fn mutate(source: &str, from: &str, to: &str) -> String {
    assert_eq!(source.matches(from).count(), 1, "`{from}` must occur once");
    source.replacen(from, to, 1)
}

/// One world per check, with the example's vocabulary, fluid statuses and
/// impact policies installed as a level install does, and a 1000-health
/// enemy.
struct Demo {
    world: World,
    policies: RefCell<ImpactPolicyRuntime>,
    shooter: EntityId,
    burning: u16,
    scalding: u16,
    corroding: u16,
    wet: u16,
    fire: u16,
    acid: u16,
}

impl Demo {
    fn new(manifest: &ModManifestResult) -> Self {
        let mut rig = Rig::new(vec![classic(
            Vec3::new(0.0, -50.0, 0.0),
            1.0,
            Vec3::ZERO,
            false,
        )]);
        rig.bind(Vec::new(), &[], Vec::new());
        let vocabulary = Arc::new(manifest.defense.clone());
        {
            let mut registry = rig.ctx.registry.borrow_mut();
            registry.set_defense_vocabulary(Arc::clone(&vocabulary));
            let fluids: Vec<(String, Option<String>)> = manifest
                .fluids
                .descriptors()
                .iter()
                .map(|fluid| {
                    let status = fluid.status.as_ref().map(|status| {
                        status
                            .handle("status")
                            .unwrap()
                            .expect_status()
                            .unwrap()
                            .to_string()
                    });
                    (fluid.name.clone(), status)
                })
                .collect();
            registry.set_fluid_statuses(
                fluids
                    .iter()
                    .map(|(fluid, status)| (fluid.as_str(), status.as_deref())),
            );
        }
        let enemy = spawn(&rig, 1000.0);
        let shooter = spawn(&rig, 100.0);
        let world = World { rig, enemy };
        let mut policies = ImpactPolicyRuntime::new(world.rig.ctx.clone());
        policies.replace_global_events(manifest.events.clone());
        let status = |name: &str| vocabulary.status_index(name).unwrap() as u16;
        let damage_type = |name: &str| {
            vocabulary
                .damage_types
                .iter()
                .position(|declaration| declaration.name == name)
                .unwrap() as u16
        };
        Self {
            policies: RefCell::new(policies),
            shooter,
            burning: status("burning"),
            scalding: status("scalding"),
            corroding: status("corroding"),
            wet: status("wet"),
            fire: damage_type("fire"),
            acid: damage_type("acid"),
            world,
        }
    }

    fn enemy(&self) -> EntityId {
        self.world.enemy
    }

    fn registry(&self) -> std::cell::RefMut<'_, EntityRegistry> {
        self.world.registry()
    }

    fn host_tick(&self) -> u64 {
        self.world.host_tick()
    }

    /// One tick, evaluating impact policies in-tick; `ai` runs at the AI
    /// stage.
    fn tick_with(&mut self, mut ai: impl FnMut(&mut EntityRegistry)) {
        let policies = &self.policies;
        self.world.rig.tick_evaluating(
            &[],
            |registry| ai(registry),
            |registry| policies.borrow_mut().evaluate_pending_in_registry(registry),
        );
    }

    fn tick(&mut self) {
        self.tick_with(|_| {});
    }

    /// One tick on which the shooter hits the enemy for 1 raw point of
    /// `damage_type`, evaluated in-tick as the AI host does after a hit.
    fn shoot(&mut self, damage_type: u16) {
        let (enemy, shooter) = (self.enemy(), self.shooter);
        let policies = &self.policies;
        self.world.rig.tick_evaluating(
            &[],
            |registry| {
                let mut context =
                    DamageContext::new("weapon.incendiary_rifle", DamageProducer::InTick);
                context.attacker = Some(shooter);
                apply_damage_with_context(
                    registry,
                    enemy,
                    &DamagePayload {
                        amount: 1.0,
                        impulse: Vec3::ZERO,
                        damage_type: Some(damage_type),
                    },
                    context,
                );
                policies.borrow_mut().evaluate_pending_in_registry(registry);
            },
            |registry| policies.borrow_mut().evaluate_pending_in_registry(registry),
        );
    }

    /// Ticks `n` times, returning `(host tick, health lost)` for each tick on
    /// which `target` lost health.
    fn losses(&mut self, target: EntityId, n: usize) -> Vec<(u64, f32)> {
        let mut landed = Vec::new();
        for _ in 0..n {
            let before = self.world.health(target);
            self.tick();
            let lost = before - self.world.health(target);
            if lost > 0.0 {
                landed.push((self.host_tick(), lost));
            }
        }
        landed
    }

    fn apply_off_tick(&self, target: EntityId, status: u16) {
        let mut registry = self.registry();
        let source = registry.intern_status_source("script.applyStatus");
        registry.apply_status(target, status, StatusApplier::Source(source), source);
    }
}

// The example's four statuses, in TypeScript and Luau: `burning` lit by a
// fire hit stacks to three and burns 4 fire (×1.25) per stack every 500 ms;
// a `wet` target does not light, takes no damage from `wet`, and an acid hit
// dries it; `corroding` deals 2 acid (×0.75) every 1000 ms until cleared, and
// `cure` clears a player's; lava scalds a wading pawn 10 fire every 250 ms.
#[test]
fn dev_statuses_fixture_behaves_as_declared() {
    for (runtime, manifest) in manifests() {
        // Three fire hits on three ticks: three stacks, each burn tick 15.
        let mut demo = Demo::new(&manifest);
        let enemy = demo.enemy();
        demo.shoot(demo.fire);
        let lit = demo.host_tick();
        demo.shoot(demo.fire);
        demo.shoot(demo.fire);
        demo.shoot(demo.fire);
        let stacks: Vec<u8> = demo
            .registry()
            .statuses()
            .sets()
            .iter()
            .filter(|set| set.entity == enemy)
            .flat_map(|set| set.instances())
            .filter(|instance| instance.status == demo.burning)
            .map(|instance| instance.stacks)
            .collect();
        assert_eq!(stacks, [3], "{runtime}: one applier, capped at maxStacks");
        let burns = demo.losses(enemy, 30);
        assert_eq!(burns, [(lit + 30, 15.0)], "{runtime}: 3 × 4 × 1.25");

        // `wet`: fire does not light it and it deals nothing; acid dries it,
        // and fire then lights.
        let mut demo = Demo::new(&manifest);
        let enemy = demo.enemy();
        demo.apply_off_tick(enemy, demo.wet);
        demo.shoot(demo.fire);
        assert!(demo.registry().has_status(enemy, demo.wet));
        assert!(
            !demo.registry().has_status(enemy, demo.burning),
            "{runtime}: a wet target does not light"
        );
        assert!(
            demo.losses(enemy, 120).is_empty(),
            "{runtime}: `wet` deals nothing"
        );
        demo.shoot(demo.acid);
        assert!(
            !demo.registry().has_status(enemy, demo.wet),
            "{runtime}: acid dries it"
        );
        demo.shoot(demo.fire);
        assert!(demo.registry().has_status(enemy, demo.burning));

        // `wet` lapses after its 5000 ms.
        let mut demo = Demo::new(&manifest);
        let enemy = demo.enemy();
        demo.apply_off_tick(enemy, demo.wet);
        demo.tick();
        for _ in 0..299 {
            demo.tick();
        }
        assert!(demo.registry().has_status(enemy, demo.wet));
        demo.tick();
        assert!(
            !demo.registry().has_status(enemy, demo.wet),
            "{runtime}: lapses at 5000 ms"
        );

        // `corroding`: 1.5 every 60 ticks, still on after ten seconds, and
        // nothing once cleared.
        let mut demo = Demo::new(&manifest);
        let enemy = demo.enemy();
        demo.apply_off_tick(enemy, demo.corroding);
        demo.tick();
        let applied = demo.host_tick();
        let corrosion = demo.losses(enemy, 600);
        let expected: Vec<(u64, f32)> = (1..=10).map(|k| (applied + 60 * k, 1.5)).collect();
        assert_eq!(corrosion, expected, "{runtime}: 2 × 0.75 every second");
        assert!(demo.registry().has_status(enemy, demo.corroding));
        demo.registry().clear_status(enemy, demo.corroding);
        assert!(demo.losses(enemy, 120).is_empty());

        // `cure` clears a corroding player's corrosion when it fires.
        let mut demo = Demo::new(&manifest);
        let pawn = demo.world.rig.local_pawn(Vec3::new(0.0, 0.0, 20.0));
        demo.apply_off_tick(pawn, demo.corroding);
        demo.tick();
        assert!(demo.registry().has_status(pawn, demo.corroding));
        let cure = manifest
            .reactions
            .iter()
            .find(|scoped| scoped.reaction.name == "statuses.cure")
            .unwrap_or_else(|| panic!("{runtime}: `cure` registers"));
        demo.world.rig.trigger(Vec3::new(0.0, 0.0, 20.0), 2.0);
        demo.world.rig.bind(
            vec![NamedReaction {
                name: "edge".into(),
                descriptor: cure.reaction.descriptor.clone(),
            }],
            &[],
            Vec::new(),
        );
        demo.tick();
        assert!(
            !demo.registry().has_status(pawn, demo.corroding),
            "{runtime}: `cure` clears it"
        );

        // `scalding`: lava scalds a wading pawn 12.5 every 15 ticks.
        let mut demo = Demo::new(&manifest);
        demo.world.rig.environment = Some((pools(), vec![0.6, 2.0]));
        let pawn = demo.world.rig.local_pawn(IN_LAVA);
        demo.tick();
        let entered = demo.host_tick();
        let scalds = demo.losses(pawn, 30);
        assert_eq!(
            scalds,
            [(entered + 15, 12.5), (entered + 30, 12.5)],
            "{runtime}: 10 × 1.25 every 250 ms"
        );
        assert!(demo.registry().has_status(pawn, demo.scalding));
    }
}

// The scripting-reference recipe: `ignite`'s `impact.status(burning)` guard
// keeps a burn tick — itself fire — from relighting the burn, so one fire
// hit burns six times over its 3000 ms and then expires. Without the guard,
// the same burn never ends.
#[test]
fn guarded_ignite_recipe_burn_expires() {
    for (runtime, manifest) in manifests() {
        let mut demo = Demo::new(&manifest);
        let enemy = demo.enemy();
        demo.shoot(demo.fire);
        let lit = demo.host_tick();
        let burns = demo.losses(enemy, 600);
        let expected: Vec<(u64, f32)> = (1..=6).map(|k| (lit + 30 * k, 5.0)).collect();
        assert_eq!(burns, expected, "{runtime}: six ticks, then out");
        assert!(!demo.registry().has_status(enemy, demo.burning));
    }

    // The same policy without its guard relights from its own ticks.
    for (runtime, file, source) in sources() {
        let unguarded = match file {
            "start-script.js" => mutate(&source, ".and(impact.status(burning).not())", ""),
            _ => mutate(
                &source,
                r#"isFire["and"](isFire, burnTick["not"](burnTick))"#,
                "isFire",
            ),
        };
        let mut demo = Demo::new(&run_manifest(file, &unguarded));
        let enemy = demo.enemy();
        demo.shoot(demo.fire);
        let burns = demo.losses(enemy, 600);
        assert_eq!(burns.len(), 20, "{runtime}: burns every 500 ms");
        assert!(demo.registry().has_status(enemy, demo.burning));
    }
}

/// The templates presented since the last call, by id, with their values.
fn presented(demo: &Demo) -> Vec<(String, f32)> {
    demo.registry()
        .take_presentation_spawns()
        .into_iter()
        .map(|spawn| {
            let value = match spawn.facts.get("value") {
                Some(postretro_entities::PresentationFact::Number(value)) => *value,
                other => panic!("a damage number carries its value: {other:?}"),
            };
            (spawn.template.0, value)
        })
        .collect()
}

// content/dev floats every enemy hit's damage: a fire or acid hit in its
// type's color, and a status tick in its status's, never both.
// See: content/dev/scripts/combat-presentation.ts
#[test]
fn dev_mod_floats_status_ticks_in_their_status_color() {
    let bundle = postretro_script_compiler::bundle_entry(&dev_scripts().join("../start-script.ts"))
        .expect("the dev start script bundles");
    let manifest = run_manifest("start-script.js", &bundle);
    let demo_for = || {
        let demo = Demo::new(&manifest);
        demo.policies
            .borrow_mut()
            .replace_presentation_templates(manifest.presentation_templates.clone());
        demo
    };

    let mut demo = demo_for();
    demo.shoot(demo.fire);
    assert_eq!(presented(&demo), [("fireDamageNumber".to_string(), 1.25)]);
    let mut burns = Vec::new();
    for _ in 0..30 {
        demo.tick();
        burns.extend(presented(&demo));
    }
    assert_eq!(burns, [("burningDamageNumber".to_string(), 5.0)]);

    let mut demo = demo_for();
    demo.shoot(demo.acid);
    assert_eq!(presented(&demo), [("acidDamageNumber".to_string(), 0.75)]);

    for (status, template, period) in [
        (demo.scalding, "scaldingDamageNumber", 15),
        (demo.corroding, "corrodingDamageNumber", 60),
    ] {
        let mut demo = demo_for();
        demo.apply_off_tick(demo.enemy(), status);
        let mut ticks = Vec::new();
        for _ in 0..=period {
            demo.tick();
            ticks.extend(presented(&demo).into_iter().map(|(id, _)| id));
        }
        assert_eq!(ticks, [template]);
    }
}
