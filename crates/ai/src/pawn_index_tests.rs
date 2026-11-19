// The faction-first pawn index through the AI pass: rebuilt every pass (P1),
// refreshing only the candidates a due selection must inspect, and walking the
// registry the same number of times whatever the enemy count.
// See: context/lib/entity_model.md §7c

use std::cell::RefCell;

use super::*;
use crate::candidate_scope::REFRESHES;
use crate::pawn_index::PawnIndex;
use crate::targeting::{self, TargetPawn};
use postretro_entities::components::brain::{RECENT_ATTACKER_LEDGER_CAPACITY, RecentAttacker};
use postretro_entities::registry::column_walks;

fn set_faction(registry: &mut EntityRegistry, entity: EntityId, faction: f32) {
    registry
        .entity_state_mut(entity)
        .expect("live entity")
        .set(FACTION_STATE_FIELD, faction);
}

fn chase_graph() -> BehaviorGraphDescriptor {
    engaged_graph(
        "engage",
        vec![("engage", activity("walk", MotionVerb::ChaseTarget, None))],
    )
}

fn peer(registry: &mut EntityRegistry, position: Vec3, faction: f32) -> EntityId {
    let entity = spawn_enemy(
        registry,
        position,
        brain_with(test_graph_with(18.0, 18.0), TEST_IDLE_STATE),
        50.0,
    );
    set_faction(registry, entity, faction);
    entity
}

fn tick(
    registry: &mut EntityRegistry,
    runtime: &mut AiRuntime,
    factions: &FactionRegistry,
    sentiment: &RefCell<FactionSentimentState>,
) {
    run_ai_tick_with_navigation_and_impact(
        registry,
        runtime,
        STEER_DT,
        AiTickInputs {
            nav_graph: None,
            mover_nav: crate::nav::MoverNav::none(),
            collision_world: None,
            descriptors: &[],
            descriptor_generation: 0,
            factions,
            faction_sentiment: sentiment,
            clips: None,
            schedule: Default::default(),
            cpu: None,
        },
        |_| {},
    );
}

fn acquired(registry: &EntityRegistry, enemy: EntityId) -> (Option<EntityId>, Option<EntityId>) {
    let brain = registry.get_component::<BrainComponent>(enemy).unwrap();
    (brain.acquired_target, brain.retaliation_acquired_target)
}

/// The reference scan's fresh selection for `enemy` over the registry as it
/// stands, with every candidate visible: what tick 2 must select.
fn reference_fresh_selection(
    registry: &EntityRegistry,
    factions: &FactionRegistry,
    sentiment: &FactionSentimentState,
    enemy: EntityId,
) -> Option<targeting::TargetSelection> {
    let live = LiveFactionSentiment::new(factions, sentiment);
    let from = registry.get_component::<Transform>(enemy).unwrap().position;
    let faction = crate::compute::entity_faction(registry, enemy);
    let brain = registry.get_component::<BrainComponent>(enemy).unwrap();
    let offers =
        targeting::reference::target_offers(registry, &live, from, faction, Some(enemy), None);
    let mut perception = |target: TargetPawn| {
        Some(perception::RawTargetPerception {
            target: target.entity,
            visible: true,
            enemy_eye: from,
            target_aim: target.position,
        })
    };
    targeting::reference::select_target_with_attacker_ledger(
        None,
        &offers,
        registry,
        &live,
        Some(enemy),
        faction,
        None,
        &mut CandidateScope::for_validation(),
        &brain.recent_attackers,
        brain.retaliation_acquired_target,
        brain.graph.retaliation(),
        STEER_DT * 1_000.0,
        &mut perception,
    )
}

// P1: tick 1's aftermath changes a pawn's faction, writes a sentiment
// override, despawns a pawn and spawns another into its slot. Tick 2's index
// is built after all of it; an index kept from tick 1 offers the despawned
// pawn and misses the newly hostile one.
#[test]
fn a_pass_after_pawn_changes_selects_what_the_reference_scan_selects() {
    let factions = FactionRegistry::default();
    let sentiment = RefCell::new(FactionSentimentState::default());
    let mut registry = EntityRegistry::new();
    let mut runtime = AiRuntime::deciding_every_tick();
    let enemy = spawn_engaged(&mut registry, Vec3::ZERO, &chase_graph(), "engage");
    set_faction(&mut registry, enemy, 1.0);
    let player = spawn_player(&mut registry, Vec3::new(6.0, 0.0, 0.0));
    set_faction(&mut registry, player, 0.0);
    let turncoat = peer(&mut registry, Vec3::new(0.0, 0.0, 2.0), 1.0);
    peer(&mut registry, Vec3::new(-4.0, 0.0, 0.0), 1.0);

    tick(&mut registry, &mut runtime, &factions, &sentiment);
    assert_eq!(
        acquired(&registry, enemy).0,
        Some(player),
        "tick 1 targets the player"
    );

    // The aftermath of tick 1's apply.
    registry.despawn(player).unwrap();
    let newcomer = peer(&mut registry, Vec3::new(1.0, 0.0, 0.0), 3.0);
    assert_ne!(newcomer, player, "a new id, wherever the registry seats it");
    set_faction(&mut registry, turncoat, 0.0);
    sentiment
        .borrow_mut()
        .set(1.0, 3.0, 1.0, factions.sentiment(1.0, 3.0))
        .unwrap();

    let expected = reference_fresh_selection(&registry, &factions, &sentiment.borrow(), enemy)
        .expect("the reference selects someone");
    assert_eq!(
        expected.target.entity, turncoat,
        "the turncoat is now the nearest hostile"
    );
    tick(&mut registry, &mut runtime, &factions, &sentiment);
    assert_eq!(
        acquired(&registry, enemy),
        (
            Some(expected.target.entity),
            expected.retaliation_acquired_target
        )
    );
}

/// Refresh a due selection over `registry`, returning the refreshed and the
/// line-of-sight-checked candidates in order.
fn due_selection_log(
    registry: &EntityRegistry,
    enemy: EntityId,
    ledger: &[Option<RecentAttacker>; RECENT_ATTACKER_LEDGER_CAPACITY],
    blind: &[EntityId],
) -> (Vec<EntityId>, Vec<EntityId>, Option<EntityId>) {
    let factions = FactionRegistry::default();
    let live = LiveFactionSentiment::with_empty_overlay(&factions);
    let index = PawnIndex::for_test(registry);
    let faction = crate::compute::entity_faction(registry, enemy);
    let offers = targeting::scanned_offers(&index, &live, Vec3::ZERO, faction, Some(enemy), None);
    let mut sighted = Vec::new();
    let mut perception = |target: TargetPawn| {
        sighted.push(target.entity);
        Some(perception::RawTargetPerception {
            target: target.entity,
            visible: !blind.contains(&target.entity),
            enemy_eye: Vec3::ZERO,
            target_aim: target.position,
        })
    };
    REFRESHES.with(|log| *log.borrow_mut() = Some(Vec::new()));
    let selected = targeting::select_target_with_attacker_ledger(
        None,
        &offers,
        &index,
        registry,
        &live,
        Some(enemy),
        faction,
        None,
        &mut CandidateScope::for_validation(),
        ledger,
        None,
        RetaliationDescriptor {
            window_ms: 1_500.0,
            damage_weight: 1.0,
            recency_weight: 0.001,
        },
        STEER_DT * 1_000.0,
        &mut perception,
    );
    let refreshed = REFRESHES.with(|log| log.borrow_mut().take()).unwrap();
    (
        refreshed,
        sighted,
        selected.map(|selection| selection.target.entity),
    )
}

// A due selection refreshes no allied pawn without an attacker record, and
// no hostile pawn past the first eligible one; line of sight runs only up to
// that one, plus ledger attackers. An allied horde changes nothing.
#[test]
fn a_due_selection_refreshes_only_hostiles_up_to_the_first_eligible_and_ledger_attackers() {
    let mut counts = Vec::new();
    for allied_peers in [0, 128] {
        let mut registry = EntityRegistry::new();
        let enemy = peer(&mut registry, Vec3::ZERO, 1.0);
        let hostiles: Vec<EntityId> = [3.0, 5.0, 7.0]
            .into_iter()
            .map(|x| {
                let player = spawn_player(&mut registry, Vec3::new(x, 0.0, 0.0));
                set_faction(&mut registry, player, 0.0);
                player
            })
            .collect();
        let mut horde = Vec::new();
        for i in 0..allied_peers {
            let angle = i as f32 * 0.37;
            let radius = 1.0 + (i % 9) as f32;
            horde.push(peer(
                &mut registry,
                Vec3::new(angle.cos() * radius, 0.0, angle.sin() * radius),
                1.0,
            ));
        }
        // One allied attacker on the ledger, under tolerance.
        let attacker = peer(&mut registry, Vec3::new(-2.0, 0.0, 0.0), 1.0);
        let mut ledger = [None; RECENT_ATTACKER_LEDGER_CAPACITY];
        ledger[0] = Some(RecentAttacker {
            attacker,
            accumulated_damage: 1.0,
            time_since_damage_ms: 100.0,
        });

        // The nearest hostile is blind: the first eligible is the second.
        let (refreshed, sighted, selected) =
            due_selection_log(&registry, enemy, &ledger, &hostiles[..1]);
        assert_eq!(selected, Some(hostiles[1]));
        assert_eq!(
            refreshed,
            vec![hostiles[0], hostiles[1], attacker],
            "{allied_peers} peers"
        );
        assert_eq!(
            sighted,
            vec![hostiles[0], hostiles[1]],
            "LOS stops at the first eligible"
        );
        assert!(horde.iter().all(|peer| !refreshed.contains(peer)));
        counts.push(refreshed.len());
    }
    assert_eq!(
        counts[0], counts[1],
        "the refresh count ignores the allied horde"
    );
}

/// One acquisition check for `enemy` with every candidate visible, logging the
/// candidates refreshed and the line-of-sight tests run: with `retained`, the
/// engaged scan that stops at the first eligible hostile; without, the
/// noticing scan an NPC without a target runs.
fn check_log(
    registry: &EntityRegistry,
    enemy: EntityId,
    retained: Option<EntityId>,
) -> (Vec<EntityId>, Vec<EntityId>) {
    let factions = FactionRegistry::default();
    let live = LiveFactionSentiment::with_empty_overlay(&factions);
    let index = PawnIndex::for_test(registry);
    let faction = crate::compute::entity_faction(registry, enemy);
    let mut sighted = Vec::new();
    let mut perception = |target: TargetPawn| {
        sighted.push(target.entity);
        Some(perception::RawTargetPerception {
            target: target.entity,
            visible: true,
            enemy_eye: Vec3::ZERO,
            target_aim: target.position,
        })
    };
    let ledger = [None; RECENT_ATTACKER_LEDGER_CAPACITY];
    REFRESHES.with(|log| *log.borrow_mut() = Some(Vec::new()));
    match retained {
        Some(retained) => {
            let offers = targeting::target_offers(Vec3::ZERO, Some(retained));
            let retained = targeting::target_candidate(registry, retained, Vec3::ZERO);
            targeting::select_target_with_attacker_ledger(
                retained,
                &offers,
                &index,
                registry,
                &live,
                Some(enemy),
                faction,
                None,
                &mut CandidateScope::for_validation(),
                &ledger,
                None,
                RetaliationDescriptor::default(),
                STEER_DT * 1_000.0,
                &mut perception,
            );
        }
        None => {
            let mut meters = Default::default();
            targeting::notice_target(
                Vec3::ZERO,
                &index,
                registry,
                &live,
                Some(enemy),
                faction,
                None,
                &mut CandidateScope::for_validation(),
                &ledger,
                RetaliationDescriptor::default(),
                STEER_DT * 1_000.0,
                &mut meters,
                Default::default(),
                &mut perception,
            );
        }
    }
    let refreshed = REFRESHES.with(|log| log.borrow_mut().take()).unwrap();
    (refreshed, sighted)
}

// AC 21: counted at the candidate-refresh seam, an NPC with a target and
// eight hostile pawns in reach (none on its attacker ledger) refreshes and
// tests line of sight for none past the first eligible one; the same NPC
// without a target refreshes and tests up to eight, its meters' bound.
#[test]
fn targeted_npc_stops_at_first_eligible_untargeted_scans_eight() {
    let mut registry = EntityRegistry::new();
    let enemy = peer(&mut registry, Vec3::ZERO, 1.0);
    let target = spawn_player(&mut registry, Vec3::new(1.0, 0.0, 0.0));
    set_faction(&mut registry, target, 0.0);
    let hostiles: Vec<EntityId> = (0..8)
        .map(|i| {
            let player = spawn_player(&mut registry, Vec3::new(3.0 + i as f32, 0.0, 0.0));
            set_faction(&mut registry, player, 0.0);
            player
        })
        .collect();
    // A ninth past the bound.
    let ninth = spawn_player(&mut registry, Vec3::new(20.0, 0.0, 0.0));
    set_faction(&mut registry, ninth, 0.0);

    let (refreshed, sighted) = check_log(&registry, enemy, Some(target));
    assert_eq!(refreshed, vec![hostiles[0]], "none past the first eligible");
    assert_eq!(sighted, vec![hostiles[0]]);

    // Without a target the retained pawn is a candidate too: the nearest
    // eight pass and fill the bound, and the ninth is never refreshed.
    let (refreshed, sighted) = check_log(&registry, enemy, None);
    let mut expected = vec![target];
    expected.extend_from_slice(&hostiles[..7]);
    assert_eq!(refreshed, expected, "up to eight");
    assert_eq!(sighted, expected);
    assert!(!refreshed.contains(&ninth));
}

fn chase_wave(brains: usize, props: usize) -> (EntityRegistry, Vec<EntityId>) {
    let mut registry = EntityRegistry::new();
    let player = spawn_player(&mut registry, Vec3::new(0.0, 0.0, 0.0));
    set_faction(&mut registry, player, 0.0);
    let graph = chase_graph();
    let enemies = (0..brains)
        .map(|i| {
            let angle = i as f32 * 0.41;
            let radius = 4.0 + (i % 5) as f32;
            let enemy = spawn_engaged(
                &mut registry,
                Vec3::new(angle.cos() * radius, 0.0, angle.sin() * radius),
                &graph,
                "engage",
            );
            set_faction(&mut registry, enemy, if i % 3 == 0 { 2.0 } else { 1.0 });
            enemy
        })
        .collect();
    for i in 0..props {
        registry.spawn(Transform {
            position: Vec3::new((i % 200) as f32, 0.0, (i / 200) as f32),
            ..Transform::default()
        });
    }
    (registry, enemies)
}

fn pass_with_walk_count(registry: &mut EntityRegistry) -> usize {
    let mut runtime = AiRuntime::deciding_every_tick();
    let factions = FactionRegistry::default();
    let sentiment = RefCell::new(FactionSentimentState::default());
    column_walks::reset();
    tick(registry, &mut runtime, &factions, &sentiment);
    column_walks::count()
}

// Registry size and enemy count leave targeting alone: every enemy selects
// the same target beside 60,000 props, and a pass walks the registry's
// columns as often for 64 brains as for one.
#[test]
fn targeting_ignores_non_pawn_entities_and_walks_columns_once_per_pass() {
    let (mut bare, enemies) = chase_wave(64, 0);
    let (mut crowded, crowded_enemies) = chase_wave(64, 60_000);
    assert_eq!(enemies, crowded_enemies, "props spawn after the pawns");
    let walks_64 = pass_with_walk_count(&mut bare);
    pass_with_walk_count(&mut crowded);
    for enemy in enemies {
        let target = acquired(&bare, enemy);
        assert!(target.0.is_some(), "every enemy targets someone");
        assert_eq!(target, acquired(&crowded, enemy));
    }

    let (mut single, _) = chase_wave(1, 0);
    let walks_1 = pass_with_walk_count(&mut single);
    assert_eq!(
        walks_1, walks_64,
        "column walks per pass ignore the brain count"
    );
}

// The index rebuilds every pass; once its buffers have grown to the level's
// pawns, a rebuild allocates nothing.
#[test]
fn a_warm_index_rebuild_allocates_nothing() {
    let (registry, _) = chase_wave(32, 100);
    let mut index = PawnIndex::default();
    index.rebuild(&registry);
    index.rebuild(&registry);
    let snapshot = crate::alloc_probe::AllocSnapshot::arm();
    index.rebuild(&registry);
    let allocations = snapshot.allocs_since();
    assert_eq!(allocations, 0, "a warm rebuild allocated");
    assert_eq!(index.entities().count(), 33, "the player and every brain");
}
