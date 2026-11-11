// The dev mod's shipped enemies under its own attack budget (row S1): the
// `content/dev` start script initialized as at boot, its reference enemy, rifle
// troopers and burst rifle spawned from the committed descriptors, and its
// tokens and `arena` profile installed as the manifest commits them.
// See: context/lib/entity_model.md §7c (Attack budget, Attack pools)

use std::path::Path;
use std::sync::Arc;

use postretro_entities::ScriptCtx;
use postretro_entities::components::attack_pool::AttackPoolComponent;
use postretro_entities::components::brain::AttackPoolId;
use postretro_foundation::attack_budget::TokenBudget as DeclaredBudget;
use postretro_scripting_core::primitives_registry::PrimitiveRegistry;
use postretro_scripting_core::runtime::{ModManifestResult, ScriptRuntime, ScriptRuntimeConfig};
use postretro_sim::attack_pools::{
    AttackPoolVerb, apply_attack_pool_requests, issue_attack_pool_request,
};

use super::*;

/// Long enough that the shipped attacks never run the target down.
const DURABLE_HP: f32 = 1.0e6;

/// `content/dev/start-script.ts`, bundled and initialized as at boot.
fn dev_manifest() -> ModManifestResult {
    let content = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/dev");
    let bundle = postretro_script_compiler::bundle_entry(&content.join("start-script.ts"))
        .expect("content/dev start-script.ts bundles");
    let mod_root = std::env::temp_dir().join(format!(
        "postretro-ai-dev-manifest-{}-{:?}",
        std::process::id(),
        std::thread::current().id(),
    ));
    std::fs::create_dir_all(&mod_root).expect("temp mod root");
    std::fs::write(mod_root.join("start-script.js"), bundle).expect("start script writes");

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
    let initialized = runtime.run_mod_init(&mod_root);
    let _ = std::fs::remove_dir_all(&mod_root);
    initialized.expect("content/dev initializes");
    runtime
        .mod_manifest()
        .expect("mod init commits a manifest")
        .clone()
}

fn descriptor<'a>(manifest: &'a ModManifestResult, name: &str) -> &'a EntityTypeDescriptor {
    manifest
        .entities
        .iter()
        .find(|entity| entity.canonical_name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("content/dev declares `{name}`"))
}

/// A registry carrying the dev mod's committed tokens and profiles.
fn dev_registry(manifest: &ModManifestResult) -> EntityRegistry {
    let mut reg = EntityRegistry::new();
    reg.set_attack_budgets(Arc::new(manifest.attack_budgets.clone()));
    reg
}

/// The player, too sturdy for the attacks to bring down: a dead target ends
/// every attack, and these proofs are about pacing.
fn durable_player(reg: &mut EntityRegistry, position: Vec3) -> EntityId {
    let pawn = spawn_player(reg, position);
    let mut health = reg.get_component::<HealthComponent>(pawn).unwrap().clone();
    health.max = DURABLE_HP;
    health.current = DURABLE_HP;
    reg.set_component(pawn, health).unwrap();
    pawn
}

/// One of `manifest`'s archetypes, spawned with its shipped behavior graph.
fn spawn_shipped(
    reg: &mut EntityRegistry,
    manifest: &ModManifestResult,
    name: &str,
    position: Vec3,
) -> EntityId {
    let graph = descriptor(manifest, name)
        .behavior
        .as_ref()
        .unwrap_or_else(|| panic!("`{name}` ships a behavior graph"));
    spawn_enemy(reg, position, BrainComponent::from_graph(graph), 100.0)
}

fn shipped_reference_enemy(
    reg: &mut EntityRegistry,
    manifest: &ModManifestResult,
    position: Vec3,
) -> EntityId {
    let enemy = spawn_shipped(reg, manifest, "reference_enemy", position);
    // Its own armor broken sends it to retreat; the shipped archetype spawns
    // with 120.
    give_reference_armor(reg, enemy);
    enemy
}

fn slam_cooldown(reg: &EntityRegistry, enemy: EntityId) -> f32 {
    reg.get_component::<BrainComponent>(enemy)
        .unwrap()
        .attack_cooldown_remaining_ms
        .get("slam")
        .copied()
        .unwrap_or(0.0)
}

fn holds(reg: &EntityRegistry, enemy: EntityId, attack: &str) -> bool {
    held(reg, enemy).iter().any(|name| name == attack)
}

// The dev mod declares the Scripting surface's tokens and `arena` profile.
#[test]
fn dev_mod_declares_the_surface_attack_tokens_and_arena_profile() {
    let manifest = dev_manifest();
    let tokens = &manifest.attack_budgets;
    let index = |name: &str| {
        usize::from(
            tokens
                .token_index(name)
                .unwrap_or_else(|| panic!("content/dev registers `{name}`")),
        )
    };
    let declared: Vec<DeclaredBudget> = tokens.declared_budgets().collect();
    let budget = |name: &str| {
        let budget = declared[index(name)];
        (budget.limit, budget.refresh_ms)
    };
    assert_eq!(budget("melee"), (3, 0.0));
    assert_eq!(budget("ranged"), (2, 0.0));
    assert_eq!(budget("heavy"), (1, 4000.0));
    let arena = tokens
        .profile_budgets("arena")
        .expect("content/dev declares the `arena` profile");
    let in_arena = |name: &str| {
        let budget = arena[index(name)];
        (budget.limit, budget.refresh_ms)
    };
    assert_eq!(in_arena("melee"), (1, 0.0));
    assert_eq!(in_arena("ranged"), (1, 0.0));
    assert_eq!(in_arena("heavy"), (1, 4000.0), "left out, so as declared");
}

// S1: two shipped reference enemies on one player each slam within 10 s,
// never within 4 s of each other, and while one slams the other's jab is
// granted. Three shipped rifle troopers on one player never hold more than
// two `ranged` turns at once, and each fires a burst and then repositions.
#[test]
fn dev_mod_budget_paces_reference_enemies_and_troopers() {
    let manifest = dev_manifest();

    // Two brutes in jab reach of one player, on opposite sides.
    let mut reg = dev_registry(&manifest);
    let mut runtime = AiRuntime::new();
    let player = durable_player(&mut reg, Vec3::ZERO);
    let brutes = [
        shipped_reference_enemy(&mut reg, &manifest, Vec3::new(-1.5, 0.0, 0.2)),
        shipped_reference_enemy(&mut reg, &manifest, Vec3::new(1.5, 0.0, -0.2)),
    ];
    const SECONDS: f32 = 16.0;
    let ticks = (SECONDS / DT) as u32;
    let mut slams: Vec<(usize, f32)> = Vec::new();
    let mut jab_beside_slam = false;
    let mut last_cooldown = [0.0_f32; 2];
    let mut held_jab = [false; 2];
    for tick in 1..=ticks {
        tick_with(&mut reg, &mut runtime, &manifest.entities);
        let now = tick as f32 * DT;
        for (index, &brute) in brutes.iter().enumerate() {
            let cooldown = slam_cooldown(&reg, brute);
            if cooldown > last_cooldown[index] {
                slams.push((index, now));
            }
            last_cooldown[index] = cooldown;
            let jab = holds(&reg, brute, "jab");
            if jab && !held_jab[index] && holds(&reg, brutes[1 - index], "slam") {
                jab_beside_slam = true;
            }
            held_jab[index] = jab;
        }
    }
    assert!(
        player_hp(&reg, player) < DURABLE_HP,
        "the brutes land their attacks"
    );
    for index in 0..2 {
        let first = slams.iter().find(|(brute, _)| *brute == index);
        assert!(
            first.is_some_and(|(_, at)| *at <= 10.0),
            "brute {index} slams within 10 s: {slams:?}"
        );
    }
    assert!(
        slams.windows(2).all(|pair| pair[1].1 - pair[0].1 >= 4.0),
        "no two slams land within 4 s of each other: {slams:?}"
    );
    assert!(
        jab_beside_slam,
        "while one brute holds its slam turn, the other's jab is granted"
    );

    // Three troopers at their firing standoff around one player.
    let mut reg = dev_registry(&manifest);
    let mut runtime = AiRuntime::new();
    durable_player(&mut reg, Vec3::ZERO);
    let troopers: Vec<EntityId> = (0..3)
        .map(|index| {
            let bearing = index as f32 * std::f32::consts::TAU / 3.0;
            let at = Vec3::new(bearing.sin(), 0.0, bearing.cos()) * 6.0;
            spawn_shipped(&mut reg, &manifest, "rifle_trooper", at)
        })
        .collect();
    let mut most_ranged = 0;
    let mut bursts = [0_u32; 3];
    let mut repositioned = [false; 3];
    let mut was_holding = [false; 3];
    for _ in 0..ticks {
        tick_with(&mut reg, &mut runtime, &manifest.entities);
        let mut ranged = 0;
        for (index, &trooper) in troopers.iter().enumerate() {
            let shooting = holds(&reg, trooper, "shoot");
            ranged += usize::from(shooting);
            bursts[index] += u32::from(shooting && !was_holding[index]);
            was_holding[index] = shooting;
            repositioned[index] |= enemy_state_path(&reg, trooper) == "engage/offense/move";
        }
        assert!(
            ranged <= 2,
            "at most two `ranged` turns on one player: {ranged}"
        );
        most_ranged = most_ranged.max(ranged);
    }
    assert_eq!(most_ranged, 2, "the troopers fill the `ranged` allowance");
    assert!(
        bursts.iter().all(|&count| count >= 2),
        "every trooper takes turns firing: {bursts:?}"
    );
    assert!(
        repositioned.iter().all(|&moved| moved),
        "every trooper enters its reposition after a burst"
    );
}

// S1: after `arena.start` fires — its `start` step issues this request — a
// guard carrying the pool's `member_tag` takes turns in the pool under the
// dev mod's `arena` profile, one ranged turn at a time, while a trooper
// outside it keeps the default pool's two.
#[test]
fn arena_start_runs_tagged_guards_under_the_arena_profile() {
    let manifest = dev_manifest();
    let mut reg = dev_registry(&manifest);
    let mut runtime = AiRuntime::new();
    durable_player(&mut reg, Vec3::ZERO);
    let arena = reg
        .try_spawn(Transform::default(), &["arena".to_string()])
        .unwrap();
    reg.set_component(
        arena,
        AttackPoolComponent::new("arena_guard".to_string(), "arena".to_string()),
    )
    .unwrap();
    let mut spawn_ring = |index: usize, tag: Option<&str>| {
        let bearing = index as f32 * std::f32::consts::TAU / 4.0;
        let at = Vec3::new(bearing.sin(), 0.0, bearing.cos()) * 6.0;
        let trooper = spawn_shipped(&mut reg, &manifest, "rifle_trooper", at);
        if let Some(tag) = tag {
            reg.set_tags(trooper, vec![tag.to_string()]).unwrap();
        }
        trooper
    };
    let guards = [
        spawn_ring(0, Some("arena_guard")),
        spawn_ring(1, Some("arena_guard")),
    ];
    let outsiders = [spawn_ring(2, None), spawn_ring(3, None)];

    issue_attack_pool_request(&mut reg, arena, AttackPoolVerb::Start);
    let mut most_guards = 0;
    let mut most_outsiders = 0;
    for _ in 0..(8.0 / DT) as u32 {
        apply_attack_pool_requests(&mut reg);
        tick_with(&mut reg, &mut runtime, &manifest.entities);
        let shooting = |group: &[EntityId]| {
            group
                .iter()
                .filter(|&&trooper| holds(&reg, trooper, "shoot"))
                .count()
        };
        for &guard in &guards {
            assert_eq!(
                reg.get_component::<BrainComponent>(guard)
                    .unwrap()
                    .attack_pool,
                Some(arena),
                "the start assigned the tag holder"
            );
            assert!(
                turns(&reg, guard)
                    .held()
                    .all(|turn| turn.pool == AttackPoolId::Pool(arena)),
                "a guard's turn counts in the arena pool"
            );
        }
        let (in_arena, outside) = (shooting(&guards), shooting(&outsiders));
        assert!(in_arena <= 1, "`arena` allows one ranged turn: {in_arena}");
        most_guards = most_guards.max(in_arena);
        most_outsiders = most_outsiders.max(outside);
    }
    assert_eq!(most_guards, 1, "the guards fire, one at a time");
    assert_eq!(
        most_outsiders, 2,
        "the default pool beside it keeps its two ranged turns"
    );
}
