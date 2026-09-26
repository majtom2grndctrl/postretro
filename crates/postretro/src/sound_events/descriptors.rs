// Descriptor-authored sounds: the weapon sound table, what each gameplay
// emission plays, and the unknown-key check run after the sound registry loads.
// See: context/lib/audio.md §4

use std::collections::{BTreeMap, HashMap};

use postretro_audio::SoundRequest;
use postretro_entities::components::brain::activity_at_path;
use postretro_entities::{
    AiCue, AiEmission, ComponentKind, ComponentValue, EntityTypeDescriptor, MovementEmission,
    ReactionDescriptor, ScriptCtx, WeaponEmission,
};
use postretro_foundation::{
    BehaviorActivityDescriptor, BehaviorGraphEnvelope, BehaviorLayerDescriptor,
    PlayerMovementComponent, WeaponSounds,
};

use super::anchors::AnchorScene;

/// Weapon sound keys by weapon descriptor canonical name. Built when a level
/// installs and rebuilt on every committed hot reload, so the next event plays
/// the reloaded key.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct DescriptorSoundTable {
    weapons: HashMap<String, WeaponSounds>,
}

impl DescriptorSoundTable {
    pub(crate) fn build(descriptors: &[EntityTypeDescriptor]) -> Self {
        let weapons = descriptors
            .iter()
            .filter_map(|descriptor| {
                let name = descriptor.canonical_name.clone()?;
                let sounds = descriptor.weapon.as_ref()?.sounds.clone()?;
                Some((name, sounds))
            })
            .collect();
        Self { weapons }
    }

    fn weapon(&self, name: Option<&str>) -> Option<&WeaponSounds> {
        self.weapons.get(name?)
    }
}

/// Which of a weapon's sounds an event plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WeaponCue {
    Fire,
    DryFire,
    Impact,
    ReloadStart,
    ReloadShell,
    ReloadComplete,
}

impl WeaponCue {
    /// The cue a weapon event address plays; spawn and the other reload
    /// outcomes play nothing.
    pub(crate) fn for_address(address: &str) -> Option<Self> {
        match address {
            "activate" => Some(Self::Fire),
            "dry_fire" => Some(Self::DryFire),
            "impact" => Some(Self::Impact),
            "reload_started" => Some(Self::ReloadStart),
            "reload_shell_loaded" => Some(Self::ReloadShell),
            "reload_completed" => Some(Self::ReloadComplete),
            _ => None,
        }
    }

    fn key(self, sounds: &WeaponSounds) -> Option<&str> {
        match self {
            Self::Fire => sounds.fire.as_deref(),
            Self::DryFire => sounds.dry_fire.as_deref(),
            Self::Impact => sounds.impact.as_deref(),
            Self::ReloadStart => sounds.reload_start.as_deref(),
            Self::ReloadShell => sounds.reload_shell.as_deref(),
            Self::ReloadComplete => sounds.reload_complete.as_deref(),
        }
    }
}

fn sfx(
    sound: &str,
    emitter: &postretro_entities::Emitter,
    scene: &mut AnchorScene<'_>,
) -> SoundRequest {
    SoundRequest {
        bus: "sfx".to_string(),
        sound: sound.to_string(),
        looping: false,
        anchor: Some(scene.sound_anchor(emitter)),
    }
}

/// The sound a weapon event plays: its weapon's key for the event's cue.
pub(crate) fn weapon_sound(
    table: &DescriptorSoundTable,
    address: &str,
    weapon: Option<&str>,
    emitter: &postretro_entities::Emitter,
    scene: &mut AnchorScene<'_>,
) -> Option<SoundRequest> {
    let cue = WeaponCue::for_address(address)?;
    let key = cue.key(table.weapon(weapon)?)?;
    Some(sfx(key, emitter, scene))
}

/// [`weapon_sound`] for a weapon emission.
pub(crate) fn weapon_emission_sound(
    table: &DescriptorSoundTable,
    emission: &WeaponEmission,
    scene: &mut AnchorScene<'_>,
) -> Option<SoundRequest> {
    weapon_sound(
        table,
        emission.address,
        emission.weapon.as_deref(),
        &emission.emitter,
        scene,
    )
}

/// The sound a movement event plays: the pawn's own movement sounds.
pub(crate) fn movement_sound(
    emission: &MovementEmission,
    scene: &mut AnchorScene<'_>,
) -> Option<SoundRequest> {
    let postretro_entities::Emitter::Entity { id, .. } = emission.emitter else {
        return None;
    };
    let sounds = scene
        .registry
        .get_component::<PlayerMovementComponent>(id)
        .ok()?
        .sounds
        .clone()?;
    let key = match emission.address {
        "landed" => sounds.land?,
        "jumped" => sounds.jump?,
        _ => return None,
    };
    Some(sfx(&key, &emission.emitter, scene))
}

/// The sounds an enemy event plays. An attack plays its own `sound` and, when
/// it names a weapon, that weapon's fire sound, both at the enemy. An entered
/// activity plays its `sound`.
pub(crate) fn ai_sounds(
    table: &DescriptorSoundTable,
    emission: &AiEmission,
    scene: &mut AnchorScene<'_>,
) -> Vec<SoundRequest> {
    let mut requests = Vec::new();
    match &emission.cue {
        AiCue::Attack { graph, attack } => {
            let Some(params) = graph.attacks.get(attack) else {
                return requests;
            };
            if let Some(sound) = params.sound.as_deref() {
                requests.push(sfx(sound, &emission.emitter, scene));
            }
            if let Some(request) = weapon_sound(
                table,
                "activate",
                params.weapon.as_deref(),
                &emission.emitter,
                scene,
            ) {
                requests.push(request);
            }
        }
        AiCue::Entered { graph, path } => {
            if let Some(sound) =
                activity_at_path(graph, path).and_then(|(_, activity)| activity.sound.as_deref())
            {
                requests.push(sfx(sound, &emission.emitter, scene));
            }
        }
    }
    requests
}

/// Warn once for each sound key a descriptor, a mover, or a `playSound`
/// reaction names that `is_loaded` does not know. The level still loads; the
/// play-time drop remains the backstop. Returns the unknown keys, sorted.
pub(crate) fn warn_unknown_sound_keys(
    script_ctx: &ScriptCtx,
    is_loaded: impl Fn(&str) -> bool,
) -> Vec<String> {
    // Key -> first place it is named, for the warning.
    let mut named: BTreeMap<String, String> = BTreeMap::new();
    let mut name = |key: &str, origin: String| {
        named.entry(key.to_string()).or_insert(origin);
    };

    let data_registry = script_ctx.data_registry.borrow();
    for descriptor in &data_registry.entities {
        let owner = descriptor.canonical_name.as_deref().unwrap_or("<unnamed>");
        if let Some(sounds) = descriptor
            .weapon
            .as_ref()
            .and_then(|weapon| weapon.sounds.as_ref())
        {
            for (field, key) in sounds.keys() {
                name(key, format!("`{owner}` weapon.sounds.{field}"));
            }
        }
        if let Some(sounds) = descriptor
            .movement
            .as_ref()
            .and_then(|movement| movement.sounds.as_ref())
        {
            for (field, key) in sounds.keys() {
                name(key, format!("`{owner}` movement.sounds.{field}"));
            }
        }
        if let Some(graph) = descriptor.behavior.as_ref() {
            for (attack, params) in &graph.attacks {
                if let Some(key) = params.sound.as_deref() {
                    name(key, format!("`{owner}` behavior.attacks.{attack}.sound"));
                }
            }
            collect_activity_sounds(&graph.envelope, owner, "behavior", &mut name);
        }
    }
    for reaction in &data_registry.reactions {
        let mut play_sound = |primitive: &str, args: &serde_json::Value| {
            if primitive == "playSound"
                && let Some(key) = args.get("sound").and_then(serde_json::Value::as_str)
            {
                name(key, format!("reaction `{}` playSound", reaction.name));
            }
        };
        match &reaction.descriptor {
            ReactionDescriptor::Primitive(primitive) => {
                play_sound(&primitive.primitive, &primitive.args);
            }
            ReactionDescriptor::Sequence(steps) => {
                for step in steps {
                    play_sound(&step.primitive, &step.args);
                }
            }
            ReactionDescriptor::Progress(_) => {}
        }
    }
    drop(data_registry);

    let registry = script_ctx.registry.borrow();
    for (_, value) in registry.iter_with_kind(ComponentKind::KinematicMover) {
        let ComponentValue::KinematicMover(mover) = value else {
            continue;
        };
        for (field, key) in [
            ("open_sound", &mover.open_sound),
            ("close_sound", &mover.close_sound),
            ("blocked_sound", &mover.blocked_sound),
            ("crush_sound", &mover.crush_sound),
        ] {
            if let Some(key) = key {
                name(key, format!("mover {} `{field}`", mover.mover_id));
            }
        }
    }
    drop(registry);

    let mut unknown = Vec::new();
    for (key, origin) in named {
        if !is_loaded(&key) {
            log::warn!("[Audio] unknown sound key `{key}` (named by {origin}); it will not play");
            unknown.push(key);
        }
    }
    unknown
}

fn collect_activity_sounds(
    envelope: &BehaviorGraphEnvelope,
    owner: &str,
    path: &str,
    name: &mut impl FnMut(&str, String),
) {
    for (activity_name, activity) in &envelope.activities {
        let path = format!("{path}.activities.{activity_name}");
        collect_one_activity(activity, owner, &path, name);
    }
}

fn collect_one_activity(
    activity: &BehaviorActivityDescriptor,
    owner: &str,
    path: &str,
    name: &mut impl FnMut(&str, String),
) {
    if let Some(key) = activity.sound.as_deref() {
        name(key, format!("`{owner}` {path}.sound"));
    }
    for (layer_name, layer) in &activity.layers {
        if let BehaviorLayerDescriptor::Graph(envelope) = layer {
            collect_activity_sounds(
                envelope,
                owner,
                &format!("{path}.layers.{layer_name}"),
                name,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use glam::Vec3;
    use postretro_audio::SoundAnchor;
    use postretro_entities::{
        BehaviorGraphDescriptor, ContactHit, Emitter, EntityId, EntityRegistry, ImpactContact,
        KinematicMoverComponent, KinematicMoverConfig, KinematicMoverMode, NamedReaction,
        PrimitiveDescriptor, Transform,
    };
    use postretro_test_log_capture::LogCapture;

    use super::*;
    use crate::runtime_movers::KinematicMoverRenderCollector;
    use crate::sound_events::anchors::entity_key;

    fn weapon_descriptor(name: &str, sounds: WeaponSounds) -> EntityTypeDescriptor {
        let mut weapon: postretro_foundation::WeaponDescriptor =
            serde_json::from_value(serde_json::json!({
                "damage": 5.0, "range": 50.0, "fireRateMs": 100.0,
                "fireMode": "semi", "resolution": "hitscan"
            }))
            .expect("minimal weapon parses");
        weapon.sounds = Some(sounds);
        EntityTypeDescriptor {
            faction: None,
            tolerance: None,
            canonical_name: Some(name.to_string()),
            inventory: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: Some(weapon),
            touchable: None,
            mesh: None,
            health: None,
            behavior: None,
        }
    }

    fn shotgun_sounds() -> WeaponSounds {
        WeaponSounds {
            fire: Some("sfx/shotgun_fire".to_string()),
            dry_fire: Some("sfx/click".to_string()),
            impact: Some("sfx/pellet_hit".to_string()),
            reload_start: Some("sfx/shotgun_open".to_string()),
            reload_shell: Some("sfx/shell_in".to_string()),
            reload_complete: Some("sfx/shotgun_pump".to_string()),
        }
    }

    fn graph() -> Arc<BehaviorGraphDescriptor> {
        Arc::new(
            serde_json::from_value(serde_json::json!({
                "initial": "idle",
                "moveSpeed": 3.0,
                "attacks": {
                    "bite": { "damage": 10.0, "maxRange": 2.0, "cooldownMs": 800.0, "sound": "sfx/bite" },
                    "shoot": { "weapon": "enemy.rifle", "sound": "sfx/grunt" }
                },
                "activities": {
                    "idle": { "animation": "idle", "motion": "hold" },
                    "alerted": { "animation": "idle", "motion": "hold", "sound": "sfx/growl" }
                },
                "transitions": { "*": [] }
            }))
            .expect("fixture graph validates"),
        )
    }

    /// A registry holding one pawn at `position`, placed at its origin.
    fn scene_parts(position: Vec3) -> (EntityRegistry, EntityId) {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform {
            position,
            ..Transform::default()
        });
        (registry, pawn)
    }

    fn emitter(registry: &EntityRegistry, id: EntityId) -> Emitter {
        postretro_sim::emission::entity_emitter(registry, id)
    }

    fn entity_anchor(id: EntityId, point: Vec3) -> Option<SoundAnchor> {
        Some(SoundAnchor::Entity {
            key: entity_key(id),
            point: point.to_array(),
        })
    }

    #[test]
    fn each_weapon_event_plays_its_weapons_key_at_its_emitter() {
        let table = DescriptorSoundTable::build(&[weapon_descriptor("shotgun", shotgun_sounds())]);
        let (registry, pawn) = scene_parts(Vec3::new(1.0, 0.0, 2.0));
        let mut movers = KinematicMoverRenderCollector::new();
        let mut scene = AnchorScene {
            registry: &registry,
            world: None,
            movers: &mut movers,
        };
        let shooter = emitter(&registry, pawn);
        for (address, key) in [
            ("activate", "sfx/shotgun_fire"),
            ("dry_fire", "sfx/click"),
            ("reload_started", "sfx/shotgun_open"),
            ("reload_shell_loaded", "sfx/shell_in"),
            ("reload_completed", "sfx/shotgun_pump"),
        ] {
            let request = weapon_sound(&table, address, Some("shotgun"), &shooter, &mut scene)
                .unwrap_or_else(|| panic!("{address} plays"));
            assert_eq!(request.sound, key);
            assert_eq!(request.bus, "sfx");
            assert_eq!(
                request.anchor,
                entity_anchor(pawn, Vec3::new(1.0, 0.0, 2.0))
            );
        }
        for silent in ["spawned", "reload_cancelled", "reload_blocked_full"] {
            assert_eq!(
                weapon_sound(&table, silent, Some("shotgun"), &shooter, &mut scene),
                None
            );
        }

        let impact = WeaponEmission {
            address: "impact",
            emitter: Emitter::Contacts(vec![
                ImpactContact::new(Vec3::X, Vec3::Z, None),
                ImpactContact::new(Vec3::Y, Vec3::Z, None),
            ]),
            weapon: Some("shotgun".to_string()),
        };
        let request = weapon_emission_sound(&table, &impact, &mut scene).expect("impact plays");
        assert_eq!(request.sound, "sfx/pellet_hit");
        assert_eq!(
            request.anchor,
            Some(SoundAnchor::Contacts(vec![
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0]
            ])),
            "the impact carries every contact; audio picks the nearest",
        );
    }

    // Pin P3: a committed reload rebuilds the table, so the next event plays
    // the new key.
    #[test]
    fn a_rebuilt_table_plays_the_reloaded_key() {
        let before = DescriptorSoundTable::build(&[weapon_descriptor("shotgun", shotgun_sounds())]);
        let mut reloaded_sounds = shotgun_sounds();
        reloaded_sounds.fire = Some("sfx/shotgun_boom".to_string());
        let after = DescriptorSoundTable::build(&[weapon_descriptor("shotgun", reloaded_sounds)]);
        let (registry, pawn) = scene_parts(Vec3::ZERO);
        let mut movers = KinematicMoverRenderCollector::new();
        let mut scene = AnchorScene {
            registry: &registry,
            world: None,
            movers: &mut movers,
        };
        let shooter = emitter(&registry, pawn);
        let fire = |table: &DescriptorSoundTable, scene: &mut AnchorScene<'_>| {
            weapon_sound(table, "activate", Some("shotgun"), &shooter, scene).map(|r| r.sound)
        };
        assert_eq!(
            fire(&before, &mut scene).as_deref(),
            Some("sfx/shotgun_fire")
        );
        assert_eq!(
            fire(&after, &mut scene).as_deref(),
            Some("sfx/shotgun_boom")
        );
    }

    // An enemy attack naming a weapon plays that weapon's fire at the enemy,
    // and its own `sound` too; its projectile's impact plays the weapon's
    // impact at the contact.
    #[test]
    fn enemy_weapon_attack_plays_the_weapon_and_the_attack_sound() {
        let table = DescriptorSoundTable::build(&[weapon_descriptor(
            "enemy.rifle",
            WeaponSounds {
                fire: Some("sfx/rifle_fire".to_string()),
                impact: Some("sfx/rifle_hit".to_string()),
                ..WeaponSounds::default()
            },
        )]);
        let (registry, enemy) = scene_parts(Vec3::new(5.0, 0.0, 0.0));
        let mut movers = KinematicMoverRenderCollector::new();
        let mut scene = AnchorScene {
            registry: &registry,
            world: None,
            movers: &mut movers,
        };
        let attack = AiEmission {
            address: Some("enemyAttack".into()),
            emitter: emitter(&registry, enemy),
            cue: AiCue::Attack {
                graph: graph(),
                attack: "shoot".to_string(),
            },
        };
        let played: Vec<_> = ai_sounds(&table, &attack, &mut scene)
            .into_iter()
            .map(|request| (request.sound, request.anchor))
            .collect();
        let at_enemy = entity_anchor(enemy, Vec3::new(5.0, 0.0, 0.0));
        assert_eq!(
            played,
            vec![
                ("sfx/grunt".to_string(), at_enemy.clone()),
                ("sfx/rifle_fire".to_string(), at_enemy),
            ],
        );

        let contact = WeaponEmission {
            address: "impact",
            emitter: Emitter::Contacts(vec![ImpactContact {
                point: Vec3::new(0.0, 1.0, 0.0),
                normal: Vec3::Y,
                hit: ContactHit::World,
            }]),
            weapon: Some("enemy.rifle".to_string()),
        };
        assert_eq!(
            weapon_emission_sound(&table, &contact, &mut scene).map(|request| request.sound),
            Some("sfx/rifle_hit".to_string()),
        );

        let contact_attack = AiEmission {
            cue: AiCue::Attack {
                graph: graph(),
                attack: "bite".to_string(),
            },
            ..attack
        };
        let played: Vec<_> = ai_sounds(&table, &contact_attack, &mut scene)
            .into_iter()
            .map(|request| request.sound)
            .collect();
        assert_eq!(
            played,
            ["sfx/bite"],
            "a contact attack plays only its own sound"
        );
    }

    #[test]
    fn entering_an_activity_plays_its_sound_at_the_enemy() {
        let table = DescriptorSoundTable::default();
        let (registry, enemy) = scene_parts(Vec3::new(0.0, 0.0, 3.0));
        let mut movers = KinematicMoverRenderCollector::new();
        let mut scene = AnchorScene {
            registry: &registry,
            world: None,
            movers: &mut movers,
        };
        let graph = graph();
        let alerted = graph
            .envelope
            .activities
            .keys()
            .position(|name| name == "alerted")
            .expect("fixture declares alerted");
        let entry = AiEmission {
            address: None,
            emitter: emitter(&registry, enemy),
            cue: AiCue::Entered {
                graph: graph.clone(),
                path: vec![alerted],
            },
        };
        let played = ai_sounds(&table, &entry, &mut scene);
        assert_eq!(played.len(), 1);
        assert_eq!(played[0].sound, "sfx/growl");
        assert_eq!(
            played[0].anchor,
            entity_anchor(enemy, Vec3::new(0.0, 0.0, 3.0))
        );

        let idle = AiEmission {
            cue: AiCue::Entered {
                graph,
                path: vec![1 - alerted],
            },
            ..entry
        };
        assert!(ai_sounds(&table, &idle, &mut scene).is_empty());
    }

    #[test]
    fn landing_and_jumping_play_the_pawns_movement_sounds_at_its_eye() {
        let mut descriptor = crate::mod_digest::tests::movement_descriptor();
        descriptor.sounds = Some(postretro_foundation::MovementSounds {
            land: Some("sfx/land".to_string()),
            jump: None,
        });
        let eye_height = descriptor.capsule.eye_height;
        let (mut registry, pawn) = scene_parts(Vec3::new(0.0, 1.0, 0.0));
        registry
            .set_component(pawn, PlayerMovementComponent::from_descriptor(&descriptor))
            .expect("movement attaches");
        let mut movers = KinematicMoverRenderCollector::new();
        let mut scene = AnchorScene {
            registry: &registry,
            world: None,
            movers: &mut movers,
        };
        let event = |address| MovementEmission {
            address,
            emitter: emitter(&registry, pawn),
        };
        let landed = movement_sound(&event("landed"), &mut scene).expect("landing plays");
        assert_eq!(landed.sound, "sfx/land");
        assert_eq!(
            landed.anchor,
            entity_anchor(pawn, Vec3::new(0.0, 1.0 + eye_height, 0.0)),
            "a player pawn sounds from its eye",
        );
        assert_eq!(movement_sound(&event("jumped"), &mut scene), None);
        assert_eq!(movement_sound(&event("dash_started"), &mut scene), None);
    }

    #[test]
    fn an_event_whose_descriptor_names_no_sound_plays_nothing_and_warns_nothing() {
        let table =
            DescriptorSoundTable::build(&[weapon_descriptor("quiet", WeaponSounds::default())]);
        let (registry, pawn) = scene_parts(Vec3::ZERO);
        let mut movers = KinematicMoverRenderCollector::new();
        let mut scene = AnchorScene {
            registry: &registry,
            world: None,
            movers: &mut movers,
        };
        let capture = LogCapture::start();
        let shooter = emitter(&registry, pawn);
        assert_eq!(
            weapon_sound(&table, "activate", Some("quiet"), &shooter, &mut scene),
            None
        );
        assert_eq!(
            weapon_sound(&table, "activate", Some("unknown"), &shooter, &mut scene),
            None
        );
        assert_eq!(
            weapon_sound(&table, "activate", None, &shooter, &mut scene),
            None
        );
        capture.assert_not_logged(log::Level::Warn, "");
    }

    #[test]
    fn unknown_sound_keys_warn_once_and_known_keys_warn_nothing() {
        let ctx = ScriptCtx::new();
        let mut sounds = shotgun_sounds();
        sounds.impact = Some("sfx/missing_hit".to_string());
        ctx.data_registry.borrow_mut().entities = vec![weapon_descriptor("shotgun", sounds)];
        ctx.data_registry.borrow_mut().populate_level(
            vec![NamedReaction {
                name: "door.open".to_string(),
                descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                    primitive: "playSound".to_string(),
                    target: None,
                    tag: None,
                    on_complete: None,
                    args: serde_json::json!({ "sound": "sfx/missing_hit" }),
                }),
            }],
            Vec::new(),
            &[],
        );
        {
            let mut registry = ctx.registry.borrow_mut();
            let door = registry.spawn(Transform::default());
            let mut mover = KinematicMoverComponent::new(
                3,
                KinematicMoverConfig {
                    waypoints: vec![Vec3::ZERO],
                    waypoint_names: vec!["closed".to_string()],
                    speed_mps: 1.0,
                    wait_ms: 0.0,
                    mode: KinematicMoverMode::PingPong,
                    started: false,
                    spin_axis: Vec3::ZERO,
                    initial_spin_rate_rad_s: 0.0,
                    spin_accel_rad_s2: 0.0,
                    carry_yaw: false,
                },
            );
            mover.open_sound = Some("sfx/door_creak".to_string());
            registry.set_component(door, mover).expect("mover attaches");
        }
        let known = [
            "sfx/shotgun_fire",
            "sfx/click",
            "sfx/shotgun_open",
            "sfx/shell_in",
            "sfx/shotgun_pump",
        ];

        let capture = LogCapture::start();
        let unknown = warn_unknown_sound_keys(&ctx, |key| known.contains(&key));
        assert_eq!(unknown, ["sfx/door_creak", "sfx/missing_hit"]);
        capture.assert_logged_once(log::Level::Warn, "unknown sound key `sfx/missing_hit`");
        capture.assert_logged_once(log::Level::Warn, "unknown sound key `sfx/door_creak`");
        for key in known {
            capture.assert_not_logged(log::Level::Warn, &format!("`{key}`"));
        }
    }
}
