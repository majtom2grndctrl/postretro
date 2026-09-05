// Evaluator-owned, bind-time-flattened behavior-statechart programs.
//
// The retained descriptor is recursive; this side table is not. Every
// envelope receives one stable vector index during `sync`, so the tick walks
// only existing slices and fixed brain-path entries.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use postretro_entities::{ComponentKind, ComponentValue, EntityId, EntityRegistry};
use postretro_foundation::{
    BakedIr, BehaviorGraphDescriptor, BehaviorGraphEnvelope, BehaviorLayerDescriptor,
    BehaviorSelectorEntry, BoundProgram, CURRENT_IR_VERSION, GuardedRow, IrType,
    ProjectileDescriptor, ResolutionMode, bind,
};
use postretro_scripting_core::data_descriptors::EntityTypeDescriptor;

use super::brain_scope::BrainScope;
use super::candidate_scope::CandidateScope;

/// Descriptor-owned projectile tuning resolved once for a weapon-referencing
/// attack. This stays beside bound programs rather than on `BrainComponent` so
/// it is derived-only and never serialized with the retained graph.
/// Evaluator-owned derived state keeps the tick path from re-resolving weapon
/// descriptor data for each firing attempt.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedProjectileAttack {
    canonical_weapon_name: String,
    range: f32,
    damage: f32,
    cooldown_ms: f32,
    credit_source: Option<String>,
    projectile: ProjectileDescriptor,
}

impl ResolvedProjectileAttack {
    pub(crate) fn canonical_weapon_name(&self) -> &str {
        &self.canonical_weapon_name
    }

    pub(crate) fn range(&self) -> f32 {
        self.range
    }

    pub(crate) fn damage(&self) -> f32 {
        self.damage
    }

    pub(crate) fn cooldown_ms(&self) -> f32 {
        self.cooldown_ms
    }

    pub(crate) fn credit_source(&self) -> Option<&str> {
        self.credit_source.as_deref()
    }

    pub(crate) fn projectile(&self) -> &ProjectileDescriptor {
        &self.projectile
    }
}

/// Guards for one descriptor envelope. `wildcard` and every activity's `rows`
/// remain in their authored declaration order. `None` is a disabled edge.
pub(crate) struct BoundEnvelope {
    pub(crate) wildcard: Vec<Option<BoundProgram<BrainScope>>>,
    pub(crate) activities: Vec<BoundActivity>,
}

pub(crate) struct BoundActivity {
    pub(crate) rows: Vec<Option<BoundProgram<BrainScope>>>,
    /// Parallel to the descriptor activity's `layers` BTreeMap. Selector guards
    /// bind here for the AI-owned `move`/`offense` names; other selectors keep
    /// `None` slots for alignment. A nested graph stores its flattened
    /// child-envelope index.
    pub(crate) layers: Vec<BoundLayer>,
}

pub(crate) enum BoundLayer {
    Selector(Vec<Option<BoundProgram<BrainScope>>>),
    Graph(usize),
}

pub(crate) struct BrainEntityPrograms {
    graph: Arc<BehaviorGraphDescriptor>,
    descriptor_generation: u64,
    envelopes: Vec<BoundEnvelope>,
    candidate_filter: Option<BoundProgram<CandidateScope>>,
    resolved_projectile_attacks: HashMap<String, ResolvedProjectileAttack>,
}

impl BrainEntityPrograms {
    pub(crate) fn envelope(&self, index: usize) -> Option<&BoundEnvelope> {
        self.envelopes.get(index)
    }

    pub(crate) fn candidate_filter(&self) -> Option<&BoundProgram<CandidateScope>> {
        self.candidate_filter.as_ref()
    }

    /// Lookup is by the graph's authored attack name, so the fire evaluator
    /// never repeats cross-descriptor resolution in its per-tick path.
    pub(crate) fn resolved_projectile_attack(
        &self,
        attack_name: &str,
    ) -> Option<&ResolvedProjectileAttack> {
        self.resolved_projectile_attacks.get(attack_name)
    }
}

/// The evaluator's programs and reusable live binding scopes.
pub(crate) struct BrainPrograms {
    scope: BrainScope,
    candidate_scope: CandidateScope,
    entries: HashMap<EntityId, BrainEntityPrograms>,
    /// Graph replacement always restarts at the new root initial descent. An
    /// old numeric state would smuggle forbidden flat history into statecharts.
    pending_reseats: HashSet<EntityId>,
    live: HashSet<EntityId>,
}

impl BrainPrograms {
    pub(crate) fn new() -> Self {
        Self {
            scope: BrainScope::for_validation(),
            candidate_scope: CandidateScope::for_validation(),
            entries: HashMap::new(),
            pending_reseats: HashSet::new(),
            live: HashSet::new(),
        }
    }

    pub(crate) fn scope_mut(&mut self) -> &mut BrainScope {
        &mut self.scope
    }

    pub(crate) fn get(&self, entity: EntityId) -> Option<&BrainEntityPrograms> {
        self.entries.get(&entity)
    }

    /// Borrow the immutable bound table and the reusable mutable scope at once
    /// without cloning either. Every path evaluator uses this seam so selector
    /// and transition evaluation share the same refreshed facts.
    pub(crate) fn with_entry_scope<R>(
        &mut self,
        entity: EntityId,
        evaluate: impl FnOnce(&BrainEntityPrograms, &mut BrainScope) -> R,
    ) -> Option<R> {
        let (entries, scope) = (&self.entries, &mut self.scope);
        entries.get(&entity).map(|entry| evaluate(entry, scope))
    }

    pub(crate) fn take_reseat(&mut self, entity: EntityId) -> bool {
        self.pending_reseats.remove(&entity)
    }

    pub(crate) fn candidate_filter_context(
        &mut self,
        entity: EntityId,
    ) -> (Option<&BoundProgram<CandidateScope>>, &mut CandidateScope) {
        let (entries, candidate_scope) = (&self.entries, &mut self.candidate_scope);
        let filter = entries
            .get(&entity)
            .and_then(BrainEntityPrograms::candidate_filter);
        (filter, candidate_scope)
    }

    /// Bind changed graphs, refresh descriptor-derived attacks, and release
    /// dead entries. All growth happens here, outside the hot evaluator and
    /// its zero-allocation probe window.
    pub(crate) fn sync(
        &mut self,
        registry: &EntityRegistry,
        descriptors: &[EntityTypeDescriptor],
        descriptor_generation: u64,
        warned: &mut HashSet<String>,
    ) {
        self.live.clear();
        for (entity, value) in registry.iter_with_kind(ComponentKind::Brain) {
            let ComponentValue::Brain(brain) = value else {
                continue;
            };
            self.live.insert(entity);
            let graph_unchanged = self
                .entries
                .get(&entity)
                .is_some_and(|entry| Arc::ptr_eq(&entry.graph, &brain.graph));
            if !graph_unchanged {
                if self.entries.contains_key(&entity) {
                    self.pending_reseats.insert(entity);
                }
                self.entries.insert(
                    entity,
                    bind_graph(
                        &self.scope,
                        &self.candidate_scope,
                        Arc::clone(&brain.graph),
                        descriptors,
                        descriptor_generation,
                        warned,
                    ),
                );
            } else if let Some(entry) = self.entries.get_mut(&entity)
                && entry.descriptor_generation != descriptor_generation
            {
                entry.resolved_projectile_attacks =
                    resolve_projectile_attacks(&brain.graph, descriptors, warned);
                entry.descriptor_generation = descriptor_generation;
            }
        }
        self.entries.retain(|entity, _| self.live.contains(entity));
        self.pending_reseats
            .retain(|entity| self.live.contains(entity));
    }
}

pub(super) fn bind_graph(
    scope: &BrainScope,
    candidate_scope: &CandidateScope,
    graph: Arc<BehaviorGraphDescriptor>,
    descriptors: &[EntityTypeDescriptor],
    descriptor_generation: u64,
    warned: &mut HashSet<String>,
) -> BrainEntityPrograms {
    let mut envelopes = Vec::new();
    bind_envelope(scope, &graph.envelope, &mut envelopes, warned);
    let candidate_filter = graph
        .candidate_filter
        .as_ref()
        .and_then(|filter| bind_candidate_filter(scope, candidate_scope, filter, warned));
    let resolved_projectile_attacks = resolve_projectile_attacks(&graph, descriptors, warned);
    BrainEntityPrograms {
        graph,
        descriptor_generation,
        envelopes,
        candidate_filter,
        resolved_projectile_attacks,
    }
}

fn resolve_projectile_attacks(
    graph: &BehaviorGraphDescriptor,
    descriptors: &[EntityTypeDescriptor],
    warned: &mut HashSet<String>,
) -> HashMap<String, ResolvedProjectileAttack> {
    graph
        .attacks
        .iter()
        .filter_map(|(attack_name, attack)| {
            let weapon_name = attack.weapon.as_deref()?;
            let resolved = crate::scripting::builtins::data_archetype::find_descriptor(
                descriptors,
                weapon_name,
            )
            .and_then(|descriptor| descriptor.weapon.as_ref())
            .filter(|weapon| weapon.resolution == ResolutionMode::Projectile)
            .and_then(|weapon| {
                weapon.projectile.as_ref().map(|projectile| ResolvedProjectileAttack {
                    canonical_weapon_name: weapon_name.to_string(),
                    range: weapon.range,
                    damage: weapon.damage,
                    cooldown_ms: weapon.cooldown_ms,
                    credit_source: weapon.credit_source.clone(),
                    projectile: projectile.clone(),
                })
            });

            match resolved {
                Some(resolved) => Some((attack_name.clone(), resolved)),
                None => {
                    let warning_key = format!("brain-attack-weapon:{attack_name}:{weapon_name}");
                    if warned.insert(warning_key) {
                        log::warn!(
                            "[AI] behavior attack `{attack_name}` references weapon `{weapon_name}` \
                             that is missing, has no weapon descriptor, or is not a projectile weapon; \
                             attack disabled"
                        );
                    }
                    None
                }
            }
        })
        .collect()
}

fn bind_envelope(
    scope: &BrainScope,
    envelope: &BehaviorGraphEnvelope,
    envelopes: &mut Vec<BoundEnvelope>,
    warned: &mut HashSet<String>,
) -> usize {
    let index = envelopes.len();
    envelopes.push(BoundEnvelope {
        wildcard: bind_rows(
            scope,
            envelope
                .transitions
                .get("*")
                .map(Vec::as_slice)
                .unwrap_or_default(),
            warned,
        ),
        activities: Vec::new(),
    });

    let mut activities = Vec::with_capacity(envelope.activities.len());
    for (name, activity) in &envelope.activities {
        let rows = bind_rows(
            scope,
            envelope
                .transitions
                .get(name)
                .map(Vec::as_slice)
                .unwrap_or_default(),
            warned,
        );
        let mut layers = Vec::with_capacity(activity.layers.len());
        for (layer_name, layer) in &activity.layers {
            match layer {
                BehaviorLayerDescriptor::Selector(entries) => {
                    let programs = if matches!(layer_name.as_str(), "move" | "offense") {
                        entries
                            .iter()
                            .map(|entry| match entry {
                                BehaviorSelectorEntry::Row(row) => row
                                    .when
                                    .as_ref()
                                    .and_then(|when| bind_guard(scope, when, warned)),
                                BehaviorSelectorEntry::Motion(_) => None,
                            })
                            .collect()
                    } else {
                        let mut programs = Vec::with_capacity(entries.len());
                        programs.resize_with(entries.len(), || None);
                        programs
                    };
                    layers.push(BoundLayer::Selector(programs));
                }
                BehaviorLayerDescriptor::Graph(child) => {
                    layers.push(BoundLayer::Graph(bind_envelope(
                        scope, child, envelopes, warned,
                    )));
                }
            }
        }
        activities.push(BoundActivity { rows, layers });
    }
    envelopes[index].activities = activities;
    index
}

fn bind_rows(
    scope: &BrainScope,
    rows: &[GuardedRow],
    warned: &mut HashSet<String>,
) -> Vec<Option<BoundProgram<BrainScope>>> {
    rows.iter()
        .map(|row| bind_guard(scope, &row.when, warned))
        .collect()
}

fn bind_guard(
    scope: &BrainScope,
    node: &postretro_foundation::IrNode,
    warned: &mut HashSet<String>,
) -> Option<BoundProgram<BrainScope>> {
    let baked = BakedIr {
        version: CURRENT_IR_VERSION,
        output: None,
        root: node.clone(),
    };
    match bind(&baked, scope) {
        Ok(program) if program.root_type == IrType::Bool => Some(program),
        Ok(program) => {
            let reason = format!("guard root is {:?}, not boolean", program.root_type);
            if warned.insert(format!("brain-guard:{reason}")) {
                log::warn!("[AI] behavior {reason}; the affected edge is disabled");
            }
            None
        }
        Err(error) => {
            let reason = error.to_string();
            if warned.insert(format!("brain-guard:{reason}")) {
                log::warn!(
                    "[AI] behavior guard could not be bound ({reason}); the affected edge is disabled"
                );
            }
            None
        }
    }
}

fn bind_candidate_filter(
    _brain_scope: &BrainScope,
    scope: &CandidateScope,
    filter: &postretro_foundation::IrNode,
    warned: &mut HashSet<String>,
) -> Option<BoundProgram<CandidateScope>> {
    let baked = BakedIr {
        version: CURRENT_IR_VERSION,
        output: None,
        root: filter.clone(),
    };
    match bind(&baked, scope) {
        Ok(program) if program.root_type == IrType::Bool => Some(program),
        Ok(program) => {
            let reason = format!(
                "candidate filter root is {:?}, not boolean",
                program.root_type
            );
            if warned.insert(format!("candidate-filter:{reason}")) {
                log::warn!("[AI] behavior {reason}; candidate filtering is disabled");
            }
            None
        }
        Err(error) => {
            let reason = error.to_string();
            if warned.insert(format!("candidate-filter:{reason}")) {
                log::warn!("[AI] candidate filter could not be bound ({reason}); it is disabled");
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use log::Level;
    use postretro_entities::Transform;
    use postretro_entities::components::brain::BrainComponent;
    use postretro_foundation::{
        AttackParams, BehaviorActivityDescriptor, BehaviorGraphEnvelope, FireMode,
        ProjectileBodyVisual, ProjectileVisual, WeaponDescriptor,
    };
    use postretro_test_log_capture::LogCapture;

    use super::*;

    const FLOAT_EPSILON: f32 = 1e-6;

    fn assert_float_eq(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() <= FLOAT_EPSILON,
            "expected {expected}, got {actual}"
        );
    }

    fn weapon_attack(weapon: &str) -> AttackParams {
        AttackParams {
            weapon: Some(weapon.to_string()),
            damage: None,
            max_range: None,
            cooldown_ms: None,
            engagement_radius: None,
            standoff_distance: None,
        }
    }

    fn graph_with_attacks(attacks: BTreeMap<String, AttackParams>) -> BehaviorGraphDescriptor {
        BehaviorGraphDescriptor {
            envelope: BehaviorGraphEnvelope {
                initial: "idle".to_string(),
                activities: BTreeMap::from([(
                    "idle".to_string(),
                    BehaviorActivityDescriptor {
                        animation: None,
                        motion: None,
                        action: None,
                        on_enter: None,
                        layers: BTreeMap::new(),
                    },
                )]),
                transitions: BTreeMap::new(),
            },
            candidate_filter: None,
            patrol: None,
            attacks,
            engagement_radius: None,
            move_speed: 0.0,
        }
    }

    fn projectile_descriptor() -> ProjectileDescriptor {
        ProjectileDescriptor {
            speed: 18.0,
            radius: 0.15,
            lifetime_ms: 1_500.0,
            visual: ProjectileVisual {
                body: ProjectileBodyVisual::Sprite {
                    sprite: "sprites/projectiles/test-bolt.png".to_string(),
                    size: 0.25,
                    opacity: 1.0,
                    rotation: 0.0,
                    tint: [1.0, 1.0, 1.0],
                    emissive: 0.0,
                    frame_duration_ms: None,
                },
                trail: None,
                light: None,
                impact_light: None,
            },
        }
    }

    fn weapon_descriptor(
        canonical_name: &str,
        range: f32,
        damage: f32,
        cooldown_ms: f32,
    ) -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            canonical_name: Some(canonical_name.to_string()),
            inventory: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: Some(WeaponDescriptor {
                damage,
                pellet_count: 1,
                spread_degrees: 0.0,
                range,
                cooldown_ms,
                fire_mode: FireMode::Semi,
                resolution: ResolutionMode::Projectile,
                projectile: Some(projectile_descriptor()),
                credit_source: Some("enemy.rifle".to_string()),
                third_person_model: None,
                viewmodel: None,
                placement: None,
                muzzle_offset: None,
                resource: None,
                lower_ms: 0,
                raise_ms: 0,
                block_during_reload: None,
            }),
            touchable: None,
            mesh: None,
            health: None,
            behavior: None,
        }
    }

    fn registry_with_brain(graph: &BehaviorGraphDescriptor) -> (EntityRegistry, EntityId) {
        let mut registry = EntityRegistry::new();
        let enemy = registry.spawn(Transform::default());
        registry
            .set_component(enemy, BrainComponent::from_graph(graph))
            .expect("fresh enemy is live");
        (registry, enemy)
    }

    #[test]
    fn sync_resolves_projectile_weapon_attack_stats_once() {
        let graph = graph_with_attacks(BTreeMap::from([(
            "shoot".to_string(),
            weapon_attack("enemy-rifle"),
        )]));
        let (registry, enemy) = registry_with_brain(&graph);
        let descriptors = [weapon_descriptor("enemy-rifle", 24.0, 13.0, 350.0)];
        let mut programs = BrainPrograms::new();
        let mut warned = HashSet::new();

        programs.sync(&registry, &descriptors, 1, &mut warned);

        let resolved = programs
            .get(enemy)
            .and_then(|entry| entry.resolved_projectile_attack("shoot"))
            .expect("projectile weapon attack resolves during sync");
        assert_eq!(resolved.canonical_weapon_name(), "enemy-rifle");
        assert_float_eq(resolved.range(), 24.0);
        assert_float_eq(resolved.damage(), 13.0);
        assert_float_eq(resolved.cooldown_ms(), 350.0);
        assert_eq!(resolved.credit_source(), Some("enemy.rifle"));
        assert_float_eq(resolved.projectile().speed, 18.0);
        assert!(warned.is_empty());
    }

    #[test]
    fn sync_refreshes_projectile_tuning_and_visual_when_descriptor_generation_changes() {
        // Regression: weapon-only hot reload left a live brain's derived
        // projectile stats and visual on the prior descriptor snapshot.
        let graph = graph_with_attacks(BTreeMap::from([(
            "shoot".to_string(),
            weapon_attack("enemy-rifle"),
        )]));
        let (registry, enemy) = registry_with_brain(&graph);
        let first = weapon_descriptor("enemy-rifle", 8.0, 3.0, 500.0);
        let mut next = weapon_descriptor("enemy-rifle", 30.0, 19.0, 200.0);
        let next_weapon = next.weapon.as_mut().expect("test descriptor has weapon");
        next_weapon.credit_source = Some("enemy.rifle.reloaded".to_string());
        let next_projectile = next_weapon
            .projectile
            .as_mut()
            .expect("test weapon is projectile-resolved");
        next_projectile.speed = 42.0;
        let ProjectileBodyVisual::Sprite { size, tint, .. } = &mut next_projectile.visual.body
        else {
            panic!("test projectile has a sprite body");
        };
        *size = 0.5;
        *tint = [0.2, 0.4, 1.0];
        let mut programs = BrainPrograms::new();
        let mut warned = HashSet::new();

        programs.sync(&registry, &[first], 1, &mut warned);
        let bound_graph = Arc::clone(&programs.get(enemy).expect("brain is bound").graph);

        programs.sync(&registry, &[next], 2, &mut warned);

        assert!(
            Arc::ptr_eq(
                &programs.get(enemy).expect("live brain remains bound").graph,
                &bound_graph,
            ),
            "descriptor refresh keeps the existing bound graph lifecycle"
        );
        assert!(
            !programs.take_reseat(enemy),
            "descriptor-only refresh does not restart graph activity"
        );
        let entry = programs.get(enemy).expect("live brain remains bound");
        let resolved = entry
            .resolved_projectile_attack("shoot")
            .expect("replacement descriptor refreshes the derived attack");
        assert_float_eq(resolved.range(), 30.0);
        assert_float_eq(resolved.damage(), 19.0);
        assert_float_eq(resolved.cooldown_ms(), 200.0);
        assert_eq!(resolved.credit_source(), Some("enemy.rifle.reloaded"));
        assert_float_eq(resolved.projectile().speed, 42.0);
        let ProjectileBodyVisual::Sprite { size, tint, .. } = &resolved.projectile().visual.body
        else {
            panic!("resolved projectile keeps its sprite body");
        };
        assert_float_eq(*size, 0.5);
        for (actual, expected) in tint.iter().zip([0.2, 0.4, 1.0]) {
            assert_float_eq(*actual, expected);
        }
        assert!(warned.is_empty());
    }

    #[test]
    fn sync_disables_attacks_when_reloaded_weapons_become_nonprojectile_or_missing() {
        // Regression: invalidated weapon descriptors left live enemies firing
        // the last resolved projectile after a descriptor-only hot reload.
        let graph = graph_with_attacks(BTreeMap::from([
            ("missing".to_string(), weapon_attack("missing-rifle")),
            ("hitscan".to_string(), weapon_attack("hitscan-rifle")),
        ]));
        let (registry, enemy) = registry_with_brain(&graph);
        let initially_valid = [
            weapon_descriptor("missing-rifle", 24.0, 13.0, 350.0),
            weapon_descriptor("hitscan-rifle", 24.0, 13.0, 350.0),
        ];
        let mut hitscan = weapon_descriptor("hitscan-rifle", 24.0, 13.0, 350.0);
        let weapon = hitscan
            .weapon
            .as_mut()
            .expect("test descriptor carries a weapon");
        weapon.resolution = ResolutionMode::Hitscan;
        weapon.projectile = None;
        let mut programs = BrainPrograms::new();
        let mut warned = HashSet::new();
        let capture = LogCapture::start();

        programs.sync(&registry, &initially_valid, 1, &mut warned);
        assert!(
            programs
                .get(enemy)
                .is_some_and(|entry| entry.resolved_projectile_attacks.len() == 2)
        );
        programs.sync(&registry, &[hitscan.clone()], 2, &mut warned);
        programs.sync(&registry, &[hitscan], 3, &mut warned);

        let entry = programs.get(enemy).expect("brain remains bound");
        assert!(entry.resolved_projectile_attack("missing").is_none());
        assert!(entry.resolved_projectile_attack("hitscan").is_none());
        capture.assert_logged_once(
            Level::Warn,
            "attack `missing` references weapon `missing-rifle`",
        );
        capture.assert_logged_once(
            Level::Warn,
            "attack `hitscan` references weapon `hitscan-rifle`",
        );
    }

    #[test]
    fn sync_rebuilds_resolved_weapon_stats_when_brain_graph_changes() {
        let first_graph = graph_with_attacks(BTreeMap::from([(
            "shoot".to_string(),
            weapon_attack("weak-rifle"),
        )]));
        let second_graph = graph_with_attacks(BTreeMap::from([(
            "shoot".to_string(),
            weapon_attack("strong-rifle"),
        )]));
        let (mut registry, enemy) = registry_with_brain(&first_graph);
        let descriptors = [
            weapon_descriptor("weak-rifle", 8.0, 3.0, 500.0),
            weapon_descriptor("strong-rifle", 30.0, 19.0, 200.0),
        ];
        let mut programs = BrainPrograms::new();
        let mut warned = HashSet::new();

        programs.sync(&registry, &descriptors, 1, &mut warned);
        assert_float_eq(
            programs
                .get(enemy)
                .and_then(|entry| entry.resolved_projectile_attack("shoot"))
                .expect("first graph resolves its attack")
                .range(),
            8.0,
        );

        registry
            .set_component(enemy, BrainComponent::from_graph(&second_graph))
            .expect("enemy remains live");
        programs.sync(&registry, &descriptors, 1, &mut warned);

        let resolved = programs
            .get(enemy)
            .and_then(|entry| entry.resolved_projectile_attack("shoot"))
            .expect("replacement graph rebuilds its derived table");
        assert_eq!(resolved.canonical_weapon_name(), "strong-rifle");
        assert_float_eq(resolved.range(), 30.0);
        assert_float_eq(resolved.damage(), 19.0);
        assert_float_eq(resolved.cooldown_ms(), 200.0);
    }
}
