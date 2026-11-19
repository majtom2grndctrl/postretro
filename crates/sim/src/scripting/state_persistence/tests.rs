use super::*;
use postretro_entities::data_registry::FactionDescriptor;
use postretro_entities::slot_table::{NumericRange, ReplicationScope, SlotSchema};
use tempfile::tempdir;

#[test]
fn lifecycle_requires_successful_manifest_and_one_restore_attempt_before_save() {
    let mut lifecycle = StateStoreLifecycle::default();
    assert!(!lifecycle.should_restore_after_mod_init(false));
    assert!(!lifecycle.can_save());

    assert!(lifecycle.should_restore_after_mod_init(true));
    assert!(!lifecycle.can_save());
    lifecycle.mark_restore_completed();

    assert!(lifecycle.can_save());
    assert!(!lifecycle.should_restore_after_mod_init(true));
    assert!(!lifecycle.should_restore_after_mod_init(false));
}

fn mod_slot(
    slot_type: SlotType,
    default: SlotValue,
    persist: bool,
    range: Option<NumericRange>,
) -> SlotRecord {
    SlotRecord::new(SlotSchema {
        slot_type,
        default: Some(default),
        range,
        persist,
        readonly: false,
        ownership: SlotOwnership::Mod,
        network: ReplicationScope::None,
        per_owner: false,
        accumulate: None,
    })
}

fn per_owner_mod_slot(persist: bool) -> SlotRecord {
    per_owner_mod_slot_with_type(SlotType::Number, SlotValue::Number(0.0), persist, None)
}

fn per_owner_mod_slot_with_type(
    slot_type: SlotType,
    default: SlotValue,
    persist: bool,
    range: Option<NumericRange>,
) -> SlotRecord {
    let mut record = mod_slot(slot_type, default, persist, range);
    record.schema.per_owner = true;
    record
}

fn declare_fixture(table: &mut SlotTable) {
    table
        .insert_namespace(
            "game",
            vec![
                (
                    "score".to_string(),
                    mod_slot(
                        SlotType::Number,
                        SlotValue::Number(10.0),
                        true,
                        Some(NumericRange {
                            min: 0.0,
                            max: 100.0,
                        }),
                    ),
                ),
                (
                    "mode".to_string(),
                    mod_slot(
                        SlotType::Enum {
                            values: vec!["normal".to_string(), "hard".to_string()],
                        },
                        SlotValue::Enum("normal".to_string()),
                        true,
                        None,
                    ),
                ),
                (
                    "enabled".to_string(),
                    mod_slot(SlotType::Boolean, SlotValue::Boolean(false), true, None),
                ),
                (
                    "label".to_string(),
                    mod_slot(
                        SlotType::String,
                        SlotValue::String("default".to_string()),
                        true,
                        None,
                    ),
                ),
                (
                    "curve".to_string(),
                    mod_slot(
                        SlotType::Array,
                        SlotValue::Array(vec![0.0, 1.0]),
                        true,
                        None,
                    ),
                ),
                (
                    "scratch".to_string(),
                    mod_slot(SlotType::Boolean, SlotValue::Boolean(false), false, None),
                ),
                (
                    "xp".to_string(),
                    per_owner_mod_slot_with_type(
                        SlotType::Number,
                        SlotValue::Number(0.0),
                        true,
                        Some(NumericRange {
                            min: 0.0,
                            max: 100.0,
                        }),
                    ),
                ),
                (
                    "rank".to_string(),
                    per_owner_mod_slot_with_type(
                        SlotType::Enum {
                            values: vec!["rookie".to_string(), "veteran".to_string()],
                        },
                        SlotValue::Enum("rookie".to_string()),
                        true,
                        None,
                    ),
                ),
                (
                    "session_xp".to_string(),
                    per_owner_mod_slot_with_type(
                        SlotType::Number,
                        SlotValue::Number(0.0),
                        false,
                        None,
                    ),
                ),
            ],
        )
        .unwrap();
}

fn fixture_identity() -> StoreIdentityLedger {
    StoreIdentityLedger {
        version: 1,
        slots: BTreeMap::from([
            ("game.score".to_string(), "k0000000000000001".to_string()),
            ("game.mode".to_string(), "k0000000000000002".to_string()),
            ("game.enabled".to_string(), "k0000000000000003".to_string()),
            ("game.label".to_string(), "k0000000000000004".to_string()),
            ("game.curve".to_string(), "k0000000000000005".to_string()),
            ("game.xp".to_string(), "k0000000000000010".to_string()),
            ("game.rank".to_string(), "k0000000000000011".to_string()),
            (
                "game.session_xp".to_string(),
                "k0000000000000012".to_string(),
            ),
        ]),
    }
}

fn player_id(byte: u8) -> [u8; 16] {
    [byte; 16]
}

fn identity_membership(identity: Option<&StoreIdentityLedger>) -> BTreeSet<String> {
    identity
        .into_iter()
        .flat_map(|identity| identity.slots.keys().cloned())
        .collect()
}

fn faction_fixture() -> FactionRegistry {
    FactionRegistry::from_descriptors(vec![
        FactionDescriptor {
            name: "cabal".to_string(),
        },
        FactionDescriptor {
            name: "resistance".to_string(),
        },
    ])
    .expect("fixture faction declarations are valid")
}

#[test]
fn persisted_state_roundtrips_per_owner_entries() {
    let persisted = PersistedState {
        version: CURRENT_STATE_VERSION,
        slots: BTreeMap::from([(
            "kglobal0000000001".to_string(),
            PersistedValue::Boolean(true),
        )]),
        per_owner: BTreeMap::from([(
            "kplayer0000000001".to_string(),
            BTreeMap::from([(
                "00112233445566778899aabbccddeeff".to_string(),
                PersistedValue::Number(42.0),
            )]),
        )]),
        faction_sentiment: BTreeMap::new(),
    };

    let serialized = serde_json::to_vec(&persisted).unwrap();
    let restored: PersistedState = serde_json::from_slice(&serialized).unwrap();

    assert_eq!(restored, persisted);
}

#[test]
fn version_two_document_defaults_per_owner_and_restores_global_slots() {
    let persisted: PersistedState = serde_json::from_value(serde_json::json!({
        "version": 2,
        "slots": { "k0000000000000001": 42.0 }
    }))
    .unwrap();
    assert!(persisted.per_owner.is_empty());
    assert!(persisted.faction_sentiment.is_empty());

    let mut table = SlotTable::new();
    declare_fixture(&mut table);
    assert!(
        overlay_persisted_state(
            &mut table,
            &persisted,
            Some(&fixture_identity()),
            &identity_membership(Some(&fixture_identity())),
            None,
            Seat(0),
        )
        .is_empty()
    );
    assert_eq!(
        table.get("game.score").unwrap().value,
        Some(SlotValue::Number(42.0))
    );
}

#[test]
fn faction_sentiment_roundtrips_authored_and_reserved_directional_pairs() {
    let factions = faction_fixture();
    let cabal = factions.index_for_name("cabal").unwrap();
    let resistance = factions.index_for_name("resistance").unwrap();
    let mut source = FactionSentimentState::default();
    source
        .set(
            cabal,
            resistance,
            -0.25,
            factions.sentiment(cabal, resistance),
        )
        .unwrap();
    source
        .set(
            resistance,
            cabal,
            0.5,
            factions.sentiment(resistance, cabal),
        )
        .unwrap();
    source
        .set(
            PLAYER_FACTION_INDEX,
            DEFAULT_ENEMY_FACTION_INDEX,
            -0.5,
            factions.sentiment(PLAYER_FACTION_INDEX, DEFAULT_ENEMY_FACTION_INDEX),
        )
        .unwrap();
    source
        .set(
            DEFAULT_ENEMY_FACTION_INDEX,
            PLAYER_FACTION_INDEX,
            -0.25,
            factions.sentiment(DEFAULT_ENEMY_FACTION_INDEX, PLAYER_FACTION_INDEX),
        )
        .unwrap();

    let mut persisted = PersistedState {
        version: CURRENT_STATE_VERSION,
        slots: BTreeMap::new(),
        per_owner: BTreeMap::new(),
        faction_sentiment: BTreeMap::new(),
    };
    assert!(collect_persisted_faction_sentiment(&mut persisted, &source, &factions).is_empty());
    assert_eq!(persisted.faction_sentiment.len(), 4);
    for (from, to, expected) in [
        (
            PERSISTED_DEFAULT_ENEMY_FACTION_KEY,
            PERSISTED_PLAYER_FACTION_KEY,
            -0.25,
        ),
        (
            PERSISTED_PLAYER_FACTION_KEY,
            PERSISTED_DEFAULT_ENEMY_FACTION_KEY,
            -0.5,
        ),
        ("cabal", "resistance", -0.25),
        ("resistance", "cabal", 0.5),
    ] {
        let values = persisted
            .faction_sentiment
            .get(from)
            .expect("persisted faction sentiment contains expected source faction");
        assert_eq!(values.len(), 1);
        let actual = values
            .get(to)
            .expect("persisted faction sentiment contains expected target faction");
        assert!((actual - expected).abs() <= f32::EPSILON);
    }

    let persisted: PersistedState =
        serde_json::from_slice(&serde_json::to_vec(&persisted).unwrap()).unwrap();
    let mut restored = FactionSentimentState::default();
    assert!(overlay_persisted_faction_sentiment(&mut restored, &factions, &persisted).is_empty());
    for (from, to, expected) in [
        (cabal, resistance, -0.25),
        (resistance, cabal, 0.5),
        (PLAYER_FACTION_INDEX, DEFAULT_ENEMY_FACTION_INDEX, -0.5),
        (DEFAULT_ENEMY_FACTION_INDEX, PLAYER_FACTION_INDEX, -0.25),
    ] {
        let actual = restored
            .get(from, to)
            .expect("restored faction sentiment contains expected pair");
        assert!((actual - expected).abs() <= f32::EPSILON);
    }
}

#[test]
fn faction_sentiment_decayed_to_baseline_is_absent_from_save() {
    let factions = faction_fixture();
    let cabal = factions.index_for_name("cabal").unwrap();
    let resistance = factions.index_for_name("resistance").unwrap();
    let baseline = factions.sentiment(cabal, resistance);
    let mut sentiment = FactionSentimentState::default();
    sentiment.set(cabal, resistance, 0.25, baseline).unwrap();
    sentiment.decay_step(1.0, |_, _| 2.0, |_, _| baseline);
    assert!(sentiment.iter().next().is_none());

    let mut persisted = PersistedState {
        version: CURRENT_STATE_VERSION,
        slots: BTreeMap::new(),
        per_owner: BTreeMap::new(),
        faction_sentiment: BTreeMap::new(),
    };
    assert!(collect_persisted_faction_sentiment(&mut persisted, &sentiment, &factions).is_empty());
    assert!(persisted.faction_sentiment.is_empty());
    let serialized = serde_json::to_value(&persisted).unwrap();
    assert!(serialized.get("faction_sentiment").is_none());
}

#[test]
fn faction_sentiment_restore_drops_unresolved_names_and_keeps_other_pairs() {
    let factions = faction_fixture();
    let cabal = factions.index_for_name("cabal").unwrap();
    let resistance = factions.index_for_name("resistance").unwrap();
    let persisted = PersistedState {
        version: CURRENT_STATE_VERSION,
        slots: BTreeMap::new(),
        per_owner: BTreeMap::new(),
        faction_sentiment: BTreeMap::from([
            (
                "cabal".to_string(),
                BTreeMap::from([("renamed-faction".to_string(), -0.25)]),
            ),
            (
                "resistance".to_string(),
                BTreeMap::from([("cabal".to_string(), 0.5)]),
            ),
        ]),
    };

    let mut restored = FactionSentimentState::default();
    let warnings = overlay_persisted_faction_sentiment(&mut restored, &factions, &persisted);
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("renamed-faction"));
    assert_eq!(restored.get(cabal, resistance), None);
    let actual = restored
        .get(resistance, cabal)
        .expect("restored faction sentiment contains the resolved pair");
    assert!((actual - 0.5).abs() <= f32::EPSILON);
}

#[test]
fn prior_state_version_without_faction_sentiment_restores_empty_overlay() {
    let persisted: PersistedState = serde_json::from_value(serde_json::json!({
        "version": CURRENT_STATE_VERSION - 1,
        "slots": {}
    }))
    .unwrap();
    assert!(persisted_state_version_is_supported(&persisted));
    assert!(persisted.faction_sentiment.is_empty());

    let factions = faction_fixture();
    let mut restored = FactionSentimentState::default();
    assert!(overlay_persisted_faction_sentiment(&mut restored, &factions, &persisted).is_empty());
    assert!(restored.iter().next().is_none());
}

#[test]
fn player_claim_id_hex_roundtrips() {
    let player_id = PlayerClaimId([
        0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
        0xff,
    ]);

    let encoded = encode_player_claim_id(&player_id);
    assert_eq!(encoded, "00112233445566778899aabbccddeeff");
    assert_eq!(decode_player_claim_id(&encoded), Some(player_id));
    assert_eq!(
        decode_player_claim_id("00112233445566778899AABBCCDDEEFF"),
        Some(player_id)
    );
    assert!(decode_player_claim_id("00112233445566778899aabbccddee").is_none());
    assert!(decode_player_claim_id("00112233445566778899aabbccddeefg").is_none());
}

#[test]
fn persistence_filters_keep_per_owner_slots_out_of_global_state() {
    let global_persistent = mod_slot(SlotType::Boolean, SlotValue::Boolean(false), true, None);
    let per_owner_persistent = per_owner_mod_slot(true);
    let per_owner_runtime_only = per_owner_mod_slot(false);

    assert!(is_persisted_mod_slot(&global_persistent));
    assert!(!is_persisted_per_owner_slot(&global_persistent));
    assert!(!is_persisted_mod_slot(&per_owner_persistent));
    assert!(is_persisted_per_owner_slot(&per_owner_persistent));
    assert!(!is_persisted_mod_slot(&per_owner_runtime_only));
    assert!(!is_persisted_per_owner_slot(&per_owner_runtime_only));
}

#[test]
fn persisted_slots_roundtrip_over_fresh_declarations() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("state.json");

    let mut source = SlotTable::new();
    declare_fixture(&mut source);
    let identity = fixture_identity();
    source.get_mut("game.score").unwrap().value = Some(SlotValue::Number(42.0));
    source.get_mut("game.mode").unwrap().value = Some(SlotValue::Enum("hard".to_string()));
    source.get_mut("game.enabled").unwrap().value = Some(SlotValue::Boolean(true));
    source.get_mut("game.label").unwrap().value = Some(SlotValue::String("continued".to_string()));
    source.get_mut("game.curve").unwrap().value = Some(SlotValue::Array(vec![0.25, 0.75, 1.0]));
    source.get_mut("game.scratch").unwrap().value = Some(SlotValue::Boolean(true));

    let membership = identity_membership(Some(&identity));
    let collected = collect_persisted_state(&source, Some(&identity), &membership);
    assert!(collected.warnings.is_empty());
    assert_eq!(
        collected.state.slots.get("k0000000000000002"),
        Some(&PersistedValue::String("hard".to_string()))
    );
    assert!(!collected.state.slots.contains_key("game.scratch"));
    save_persisted_state(&path, &collected.state).unwrap();

    let mut fresh = SlotTable::new();
    declare_fixture(&mut fresh);
    let loaded = load_persisted_state(&path).unwrap().unwrap();
    assert!(
        overlay_persisted_state(
            &mut fresh,
            &loaded,
            Some(&identity),
            &membership,
            None,
            Seat(0)
        )
        .is_empty()
    );

    assert_eq!(
        fresh.get("game.score").unwrap().value,
        Some(SlotValue::Number(42.0))
    );
    assert_eq!(
        fresh.get("game.mode").unwrap().value,
        Some(SlotValue::Enum("hard".to_string()))
    );
    assert_eq!(
        fresh.get("game.enabled").unwrap().value,
        Some(SlotValue::Boolean(true))
    );
    assert_eq!(
        fresh.get("game.label").unwrap().value,
        Some(SlotValue::String("continued".to_string()))
    );
    assert_eq!(
        fresh.get("game.curve").unwrap().value,
        Some(SlotValue::Array(vec![0.25, 0.75, 1.0]))
    );
    assert_eq!(
        fresh.get("game.scratch").unwrap().value,
        Some(SlotValue::Boolean(false))
    );
}

#[test]
fn overlay_ignores_unknown_mismatched_and_invalid_enum_entries() {
    let mut table = SlotTable::new();
    declare_fixture(&mut table);
    let identity = fixture_identity();
    let persisted = PersistedState {
        version: CURRENT_STATE_VERSION,
        slots: BTreeMap::from([
            (
                "k0000000000000001".to_string(),
                PersistedValue::String("many".into()),
            ),
            (
                "k0000000000000002".to_string(),
                PersistedValue::String("nightmare".into()),
            ),
            ("kffffffffffffffff".to_string(), PersistedValue::Number(1.0)),
        ]),
        per_owner: BTreeMap::new(),
        faction_sentiment: BTreeMap::new(),
    };

    let warnings = overlay_persisted_state(
        &mut table,
        &persisted,
        Some(&identity),
        &identity_membership(Some(&identity)),
        None,
        Seat(0),
    );
    assert_eq!(warnings.len(), 3);
    assert_eq!(
        table.get("game.score").unwrap().value,
        Some(SlotValue::Number(10.0))
    );
    assert_eq!(
        table.get("game.mode").unwrap().value,
        Some(SlotValue::Enum("normal".to_string()))
    );
}

#[test]
fn overlay_ignores_bad_version_and_non_finite_rust_values() {
    let mut table = SlotTable::new();
    declare_fixture(&mut table);
    let identity = fixture_identity();
    let bad_version = PersistedState {
        version: CURRENT_STATE_VERSION + 1,
        slots: BTreeMap::from([(
            "k0000000000000001".to_string(),
            PersistedValue::Number(99.0),
        )]),
        per_owner: BTreeMap::from([(
            "k0000000000000010".to_string(),
            BTreeMap::from([(
                encode_player_claim_id(&PlayerClaimId(player_id(0x77))),
                PersistedValue::Number(77.0),
            )]),
        )]),
        faction_sentiment: BTreeMap::new(),
    };
    assert_eq!(
        overlay_persisted_state(
            &mut table,
            &bad_version,
            Some(&identity),
            &identity_membership(Some(&identity)),
            None,
            Seat(0),
        )
        .len(),
        1
    );
    assert_eq!(
        table.get("game.score").unwrap().value,
        Some(SlotValue::Number(10.0))
    );
    // Regression: rejected documents remained available to seed a host join.
    assert!(join_seed_from_persisted_state(Some(&bad_version), Some(player_id(0x77))).is_empty());

    let non_finite = PersistedState {
        version: CURRENT_STATE_VERSION,
        slots: BTreeMap::from([(
            "k0000000000000001".to_string(),
            PersistedValue::Number(f64::NAN),
        )]),
        per_owner: BTreeMap::new(),
        faction_sentiment: BTreeMap::new(),
    };
    assert_eq!(
        overlay_persisted_state(
            &mut table,
            &non_finite,
            Some(&identity),
            &identity_membership(Some(&identity)),
            None,
            Seat(0),
        )
        .len(),
        1
    );
    assert_eq!(
        table.get("game.score").unwrap().value,
        Some(SlotValue::Number(10.0))
    );

    let non_finite_array = PersistedState {
        version: CURRENT_STATE_VERSION,
        slots: BTreeMap::from([(
            "k0000000000000005".to_string(),
            PersistedValue::Array(vec![0.0, f64::INFINITY]),
        )]),
        per_owner: BTreeMap::new(),
        faction_sentiment: BTreeMap::new(),
    };
    assert_eq!(
        overlay_persisted_state(
            &mut table,
            &non_finite_array,
            Some(&identity),
            &identity_membership(Some(&identity)),
            None,
            Seat(0),
        )
        .len(),
        1
    );
    assert_eq!(
        table.get("game.curve").unwrap().value,
        Some(SlotValue::Array(vec![0.0, 1.0]))
    );
}

#[test]
fn empty_persist_set_writes_empty_slots_map_and_missing_file_is_ok() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("missing.json");
    let table = SlotTable::new();
    let collected = collect_persisted_state(&table, None, &BTreeSet::new());

    assert!(collected.state.slots.is_empty());
    assert!(load_persisted_state(&path).unwrap().is_none());

    save_persisted_state(&path, &collected.state).unwrap();
    let json: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(json["version"], CURRENT_STATE_VERSION);
    assert_eq!(json["slots"], serde_json::json!({}));
    assert!(json.get("per_owner").is_none());
    assert!(json.get("faction_sentiment").is_none());
}

#[test]
fn overlay_clamps_finite_out_of_range_numbers() {
    let mut table = SlotTable::new();
    declare_fixture(&mut table);
    let identity = fixture_identity();
    let persisted = PersistedState {
        version: CURRENT_STATE_VERSION,
        slots: BTreeMap::from([(
            "k0000000000000001".to_string(),
            PersistedValue::Number(500.0),
        )]),
        per_owner: BTreeMap::new(),
        faction_sentiment: BTreeMap::new(),
    };

    let warnings = overlay_persisted_state(
        &mut table,
        &persisted,
        Some(&identity),
        &identity_membership(Some(&identity)),
        None,
        Seat(0),
    );
    assert_eq!(warnings.len(), 1);
    assert_eq!(
        table.get("game.score").unwrap().value,
        Some(SlotValue::Number(100.0))
    );
}

#[test]
fn per_owner_roundtrip_restores_only_the_matching_player_and_preserves_others() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("state.json");
    let identity = fixture_identity();
    let membership = identity_membership(Some(&identity));
    let local_id = player_id(0x11);
    let other_id = player_id(0x22);
    let local_seat = Seat(7);

    let mut source = SlotTable::new();
    declare_fixture(&mut source);
    let xp = source.get_mut("game.xp").unwrap();
    xp.set_per_seat_value(local_seat, SlotValue::Number(42.0));
    xp.set_per_seat_value(Seat(8), SlotValue::Number(99.0));

    let collected =
        collect_per_owner_state(&source, Some(&identity), &membership, local_seat, local_id);
    assert!(collected.warnings.is_empty());
    assert_eq!(
        collected.per_owner,
        BTreeMap::from([
            (
                "k0000000000000010".to_string(),
                BTreeMap::from([(
                    encode_player_claim_id(&PlayerClaimId(local_id)),
                    PersistedValue::Number(42.0),
                )]),
            ),
            (
                "k0000000000000011".to_string(),
                BTreeMap::from([(
                    encode_player_claim_id(&PlayerClaimId(local_id)),
                    PersistedValue::String("rookie".to_string()),
                )]),
            ),
        ]),
        "collection reads only the local session seat, keyed by the durable player id"
    );

    let retained = PersistedState {
        version: CURRENT_STATE_VERSION,
        slots: BTreeMap::from([(
            "k0000000000000001".to_string(),
            PersistedValue::Number(88.0),
        )]),
        per_owner: BTreeMap::from([(
            "k0000000000000010".to_string(),
            BTreeMap::from([(
                encode_player_claim_id(&PlayerClaimId(other_id)),
                PersistedValue::Number(17.0),
            )]),
        )]),
        faction_sentiment: BTreeMap::new(),
    };
    let client_document = collected_per_owner_only_state(Some(&retained), collected.per_owner);
    assert_eq!(
        client_document.slots, retained.slots,
        "a client saves its own boot world, never the host's"
    );
    assert_eq!(
        client_document.per_owner["k0000000000000010"]
            [&encode_player_claim_id(&PlayerClaimId(other_id))],
        PersistedValue::Number(17.0),
        "the retained document keeps another player's entry"
    );
    save_persisted_state(&path, &client_document).unwrap();

    let loaded = load_persisted_state(&path).unwrap().unwrap();
    let mut fresh = SlotTable::new();
    declare_fixture(&mut fresh);
    assert!(
        overlay_persisted_state(
            &mut fresh,
            &loaded,
            Some(&identity),
            &membership,
            Some(local_id),
            local_seat,
        )
        .is_empty()
    );
    assert_eq!(
        fresh.get("game.xp").unwrap().per_seat_value(local_seat),
        Some(&SlotValue::Number(42.0))
    );
    assert_eq!(
        fresh.get("game.xp").unwrap().per_seat_value(Seat(8)),
        Some(&SlotValue::Number(0.0)),
        "the other player's retained entry is not loaded into this session"
    );
}

// Regression: connected restore cached local-only progress under Seat(0),
// while periodic and exit saves read the host-assigned roster seat.
#[test]
fn connected_local_only_restore_uses_the_assigned_roster_seat() {
    let identity = fixture_identity();
    let membership = identity_membership(Some(&identity));
    let local_id = player_id(0x66);
    let local_key = encode_player_claim_id(&PlayerClaimId(local_id));
    let assigned_seat = Seat(7);
    let persisted = PersistedState {
        version: CURRENT_STATE_VERSION,
        slots: BTreeMap::new(),
        per_owner: BTreeMap::from([
            (
                "k0000000000000010".to_string(),
                BTreeMap::from([(local_key.clone(), PersistedValue::Number(42.0))]),
            ),
            (
                "k0000000000000011".to_string(),
                BTreeMap::from([(local_key, PersistedValue::String("rookie".to_string()))]),
            ),
        ]),
        faction_sentiment: BTreeMap::new(),
    };
    let mut table = SlotTable::new();
    declare_fixture(&mut table);
    table.get_mut("game.rank").unwrap().schema.network = ReplicationScope::OwnerPrivatePlayer;
    table
        .get_mut("game.rank")
        .unwrap()
        .set_per_seat_value(assigned_seat, SlotValue::Enum("veteran".to_string()));

    assert!(
        overlay_client_local_per_owner_state(
            &mut table,
            &persisted,
            Some(&identity),
            &membership,
            Some(local_id),
            assigned_seat,
        )
        .is_empty()
    );
    assert_eq!(
        table.get("game.xp").unwrap().per_seat_value(assigned_seat),
        Some(&SlotValue::Number(42.0))
    );
    assert_eq!(
        table.get("game.xp").unwrap().per_seat_value(Seat(0)),
        Some(&SlotValue::Number(0.0)),
        "connected restore does not use the single-player seat"
    );
    assert_eq!(
        table
            .get("game.rank")
            .unwrap()
            .per_seat_value(assigned_seat),
        Some(&SlotValue::Enum("veteran".to_string())),
        "local restore does not overwrite an owner-private host projection"
    );

    let collected = collect_per_owner_state(
        &table,
        Some(&identity),
        &membership,
        assigned_seat,
        local_id,
    );
    assert_eq!(
        collected.per_owner["k0000000000000010"][&encode_player_claim_id(&PlayerClaimId(local_id))],
        PersistedValue::Number(42.0)
    );
}

#[test]
fn per_owner_overlay_applies_global_validation_and_skips_runtime_only_slots() {
    let identity = fixture_identity();
    let membership = identity_membership(Some(&identity));
    let local_id = player_id(0x33);
    let local_key = encode_player_claim_id(&PlayerClaimId(local_id));
    let mut table = SlotTable::new();
    declare_fixture(&mut table);
    let persisted = PersistedState {
        version: CURRENT_STATE_VERSION,
        slots: BTreeMap::new(),
        per_owner: BTreeMap::from([
            (
                "k0000000000000010".to_string(),
                BTreeMap::from([(local_key.clone(), PersistedValue::Number(500.0))]),
            ),
            (
                "k0000000000000011".to_string(),
                BTreeMap::from([(local_key.clone(), PersistedValue::String("legend".into()))]),
            ),
            (
                "k0000000000000012".to_string(),
                BTreeMap::from([(local_key, PersistedValue::Number(99.0))]),
            ),
        ]),
        faction_sentiment: BTreeMap::new(),
    };

    let warnings = overlay_persisted_state(
        &mut table,
        &persisted,
        Some(&identity),
        &membership,
        Some(local_id),
        Seat(3),
    );
    assert_eq!(
        warnings.len(),
        3,
        "clamp, invalid enum, and runtime-only slot warn"
    );
    assert_eq!(
        table.get("game.xp").unwrap().per_seat_value(Seat(3)),
        Some(&SlotValue::Number(100.0)),
        "per-owner numeric restore clamps through the shared validator"
    );
    assert_eq!(
        table.get("game.rank").unwrap().per_seat_value(Seat(3)),
        Some(&SlotValue::Enum("rookie".to_string())),
        "invalid enum values retain the declared default"
    );
    assert_eq!(
        table
            .get("game.session_xp")
            .unwrap()
            .per_seat_value(Seat(3)),
        Some(&SlotValue::Number(0.0)),
        "per-owner but non-persistent slots never restore"
    );

    let wrong_type = PersistedState {
        version: CURRENT_STATE_VERSION,
        slots: BTreeMap::new(),
        per_owner: BTreeMap::from([(
            "k0000000000000010".to_string(),
            BTreeMap::from([(
                encode_player_claim_id(&PlayerClaimId(local_id)),
                PersistedValue::String("not-a-number".into()),
            )]),
        )]),
        faction_sentiment: BTreeMap::new(),
    };
    assert_eq!(
        overlay_persisted_state(
            &mut table,
            &wrong_type,
            Some(&identity),
            &membership,
            Some(local_id),
            Seat(3),
        )
        .len(),
        1
    );
    assert_eq!(
        table.get("game.xp").unwrap().per_seat_value(Seat(3)),
        Some(&SlotValue::Number(100.0)),
        "a type mismatch does not replace the validated live value"
    );
}

#[test]
// Regression: each connected-client save wrote an empty world section,
// erasing the single-player world the client would return to (pin P9).
fn client_save_document_keeps_its_boot_world_and_replaces_its_own_rows() {
    let local_id = player_id(0x44);
    let other_id = player_id(0x55);
    let retained = PersistedState {
        version: CURRENT_STATE_VERSION,
        slots: BTreeMap::from([("k0000000000000001".to_string(), PersistedValue::Number(6.0))]),
        per_owner: BTreeMap::from([(
            "k0000000000000010".to_string(),
            BTreeMap::from([
                (
                    encode_player_claim_id(&PlayerClaimId(local_id)),
                    PersistedValue::Number(3.0),
                ),
                (
                    encode_player_claim_id(&PlayerClaimId(other_id)),
                    PersistedValue::Number(9.0),
                ),
            ]),
        )]),
        faction_sentiment: BTreeMap::new(),
    };
    let saved = collected_per_owner_only_state(
        Some(&retained),
        BTreeMap::from([(
            "k0000000000000010".to_string(),
            BTreeMap::from([(
                encode_player_claim_id(&PlayerClaimId(local_id)),
                PersistedValue::Number(12.0),
            )]),
        )]),
    );

    assert_eq!(saved.slots, retained.slots, "the boot world stays on disk");
    assert_eq!(saved.faction_sentiment, retained.faction_sentiment);
    assert_eq!(
        saved.per_owner["k0000000000000010"][&encode_player_claim_id(&PlayerClaimId(local_id))],
        PersistedValue::Number(12.0)
    );
    assert_eq!(
        saved.per_owner["k0000000000000010"][&encode_player_claim_id(&PlayerClaimId(other_id))],
        PersistedValue::Number(9.0)
    );

    let mut retained = Some(retained);
    retain_saved_per_owner_state(&mut retained, saved);
    let retained = retained.unwrap();
    assert_eq!(
        retained.slots.get("k0000000000000001"),
        Some(&PersistedValue::Number(6.0)),
        "the retained boot document keeps its globals"
    );
}

/// The fixture as a connected client sees it: `score` and `xp` replicate,
/// `session_xp` replicates without persisting, `label` and `rank` never
/// replicate.
fn client_session_table() -> SlotTable {
    let mut table = SlotTable::new();
    declare_fixture(&mut table);
    table.get_mut("game.score").unwrap().schema.network = ReplicationScope::SharedGlobal;
    for name in ["game.xp", "game.session_xp"] {
        table.get_mut(name).unwrap().schema.network = ReplicationScope::OwnerPrivatePlayer;
    }
    table
}

fn boot_world(local_id: [u8; 16], xp: f64) -> PersistedState {
    PersistedState {
        version: CURRENT_STATE_VERSION,
        slots: BTreeMap::from([("k0000000000000001".to_string(), PersistedValue::Number(6.0))]),
        per_owner: BTreeMap::from([(
            "k0000000000000010".to_string(),
            BTreeMap::from([(
                encode_player_claim_id(&PlayerClaimId(local_id)),
                PersistedValue::Number(xp),
            )]),
        )]),
        faction_sentiment: BTreeMap::new(),
    }
}

// Pins P9 and P19: the host's world leaves the store, the client's own
// writes stay, and its character lands on the single-player seat.
#[test]
fn restore_after_session_returns_the_host_world_to_the_boot_world() {
    let identity = fixture_identity();
    let membership = identity_membership(Some(&identity));
    let local_id = player_id(0x44);
    let session_seat = Seat(2);
    let mut table = client_session_table();
    let write = |table: &mut SlotTable, name: &str, value: SlotValue| {
        table.get_mut(name).unwrap().write_value(Some(value));
    };
    write(&mut table, "game.score", SlotValue::Number(50.0));
    write(
        &mut table,
        "game.label",
        SlotValue::String("mine".to_string()),
    );
    for (name, value) in [
        ("game.xp", SlotValue::Number(40.0)),
        ("game.rank", SlotValue::Enum("veteran".to_string())),
        ("game.session_xp", SlotValue::Number(5.0)),
    ] {
        table
            .get_mut(name)
            .unwrap()
            .set_per_seat_value(session_seat, value);
    }

    let warnings = restore_store_after_session(
        &mut table,
        Some(&boot_world(local_id, 3.0)),
        Some(&identity),
        &membership,
        Some(session_seat),
        true,
        Some(local_id),
    );

    assert!(warnings.is_empty(), "{warnings:?}");
    let value = |name: &str| table.get(name).unwrap().value.clone();
    assert_eq!(
        value("game.score"),
        Some(SlotValue::Number(6.0)),
        "boot world"
    );
    assert_eq!(
        value("game.label"),
        Some(SlotValue::String("mine".to_string()))
    );
    for (name, expected) in [
        ("game.xp", SlotValue::Number(40.0)),
        ("game.rank", SlotValue::Enum("veteran".to_string())),
        ("game.session_xp", SlotValue::Number(0.0)),
    ] {
        let record = table.get(name).unwrap();
        assert_eq!(record.per_seat_value(Seat(0)), Some(&expected), "{name}");
        assert_eq!(record.value.as_ref(), Some(&expected), "{name} scalar");
    }
}

// Pin P13: a client that never participated received no character, so
// its saved one returns to the single-player seat.
#[test]
fn restore_for_a_non_participant_takes_its_saved_character() {
    let identity = fixture_identity();
    let membership = identity_membership(Some(&identity));
    let local_id = player_id(0x44);
    let mut table = client_session_table();

    let _ = restore_store_after_session(
        &mut table,
        Some(&boot_world(local_id, 7.0)),
        Some(&identity),
        &membership,
        Some(Seat(2)),
        false,
        Some(local_id),
    );

    let xp = table.get("game.xp").unwrap();
    assert_eq!(xp.per_seat_value(Seat(0)), Some(&SlotValue::Number(7.0)));
    assert_eq!(xp.value, Some(SlotValue::Number(7.0)));
}

// Regression (review): a client that ends with no seat (every connect
// failed, refused at admission, lost before its first roster) never had
// its local-only rows overlaid, and the restore wrote their defaults,
// which the single-player exit save then put on disk.
#[test]
fn restore_without_a_session_seat_keeps_the_saved_local_only_values() {
    let identity = fixture_identity();
    let membership = identity_membership(Some(&identity));
    let local_id = player_id(0x44);
    let mut boot = boot_world(local_id, 7.0);
    boot.per_owner.insert(
        "k0000000000000011".to_string(),
        BTreeMap::from([(
            encode_player_claim_id(&PlayerClaimId(local_id)),
            PersistedValue::String("veteran".to_string()),
        )]),
    );
    let mut table = client_session_table();

    let warnings = restore_store_after_session(
        &mut table,
        Some(&boot),
        Some(&identity),
        &membership,
        None,
        false,
        Some(local_id),
    );

    assert!(warnings.is_empty(), "{warnings:?}");
    for (name, expected) in [
        ("game.xp", SlotValue::Number(7.0)),
        ("game.rank", SlotValue::Enum("veteran".to_string())),
    ] {
        let record = table.get(name).unwrap();
        assert_eq!(record.per_seat_value(Seat(0)), Some(&expected), "{name}");
        assert_eq!(record.value.as_ref(), Some(&expected), "{name} scalar");
    }
}

#[test]
fn final_character_carries_only_the_persisted_per_owner_values_the_host_replicates() {
    let identity = fixture_identity();
    let membership = identity_membership(Some(&identity));
    let mut table = client_session_table();
    for (name, value) in [
        ("game.xp", SlotValue::Number(25.0)),
        ("game.session_xp", SlotValue::Number(5.0)),
    ] {
        table
            .get_mut(name)
            .unwrap()
            .set_per_seat_value(Seat(3), value);
    }

    let (slots, warnings) = final_character_for_seat(&table, Some(&identity), &membership, Seat(3));

    assert!(warnings.is_empty(), "{warnings:?}");
    // `rank` never replicates: it is the client's own (pin P19), and
    // `session_xp` does not persist.
    assert_eq!(
        slots,
        BTreeMap::from([("k0000000000000010".to_string(), JoinSeedValue::Number(25.0))])
    );
}

#[test]
fn client_projection_populates_the_local_seat_cache_without_global_fallback() {
    let mut table = SlotTable::new();
    declare_fixture(&mut table);
    table.get_mut("game.xp").unwrap().schema.network = ReplicationScope::OwnerPrivatePlayer;
    table
        .get_mut("game.xp")
        .unwrap()
        .write_value(Some(SlotValue::Number(25.0)));
    table
        .get_mut("game.score")
        .unwrap()
        .write_value(Some(SlotValue::Number(88.0)));

    sync_client_per_owner_projection(&mut table, Seat(4));

    assert_eq!(
        table.get("game.xp").unwrap().per_seat_value(Seat(4)),
        Some(&SlotValue::Number(25.0))
    );
    assert_eq!(
        table.get("game.score").unwrap().value,
        Some(SlotValue::Number(88.0)),
        "the helper does not route globals through a seat map"
    );
}

#[test]
fn periodic_client_save_timer_is_participation_gated_and_resets_on_reconnect() {
    let mut timer = PerOwnerSaveTimer::default();
    timer.observe_connection(true);
    assert!(!timer.advance(Duration::from_secs(59), true));
    assert!(timer.advance(Duration::from_secs(1), true));
    assert!(timer.advance(Duration::from_secs(60), true));

    assert!(!timer.advance(Duration::from_secs(59), true));
    assert!(!timer.advance(Duration::from_secs(120), false));
    assert!(timer.advance(Duration::from_secs(1), true));

    assert!(!timer.advance(Duration::from_secs(30), true));
    timer.observe_connection(false);
    timer.observe_connection(true);
    assert!(!timer.advance(Duration::from_secs(59), true));
    assert!(timer.advance(Duration::from_secs(1), true));
}

#[test]
fn save_and_overlay_reject_readonly_or_non_persistent_targets() {
    let mut table = SlotTable::new();
    declare_fixture(&mut table);
    let mut identity = fixture_identity();
    table
        .insert(
            "game.locked".to_string(),
            SlotRecord::new(SlotSchema {
                slot_type: SlotType::String,
                default: Some(SlotValue::String("default".to_string())),
                range: None,
                persist: true,
                readonly: true,
                ownership: SlotOwnership::Mod,
                network: ReplicationScope::None,
                per_owner: false,
                accumulate: None,
            }),
        )
        .unwrap();

    identity
        .slots
        .insert("game.locked".to_string(), "k0000000000000006".to_string());
    identity
        .slots
        .insert("game.scratch".to_string(), "k0000000000000007".to_string());
    identity
        .slots
        .insert("player.health".to_string(), "k0000000000000008".to_string());
    let membership = identity_membership(Some(&identity));
    let collected = collect_persisted_state(&table, Some(&identity), &membership);
    assert!(!collected.state.slots.contains_key("game.locked"));
    assert!(!collected.state.slots.contains_key("game.scratch"));

    let persisted = PersistedState {
        version: CURRENT_STATE_VERSION,
        slots: BTreeMap::from([
            (
                "k0000000000000006".to_string(),
                PersistedValue::String("changed".to_string()),
            ),
            (
                "k0000000000000007".to_string(),
                PersistedValue::Boolean(true),
            ),
            ("k0000000000000008".to_string(), PersistedValue::Number(1.0)),
        ]),
        per_owner: BTreeMap::new(),
        faction_sentiment: BTreeMap::new(),
    };
    let warnings = overlay_persisted_state(
        &mut table,
        &persisted,
        Some(&identity),
        &membership,
        None,
        Seat(0),
    );

    assert_eq!(warnings.len(), 3);
    assert_eq!(
        table.get("game.locked").unwrap().value,
        Some(SlotValue::String("default".to_string()))
    );
    assert_eq!(
        table.get("game.scratch").unwrap().value,
        Some(SlotValue::Boolean(false))
    );
    assert_eq!(table.get("player.health").unwrap().value, None);
}

#[test]
fn preserved_durable_key_restores_after_an_authored_slot_rename() {
    let mut before_rename = SlotTable::new();
    before_rename
        .insert_namespace(
            "story",
            vec![(
                "old_score".to_string(),
                mod_slot(SlotType::Number, SlotValue::Number(0.0), true, None),
            )],
        )
        .unwrap();
    before_rename.get_mut("story.old_score").unwrap().value = Some(SlotValue::Number(42.0));
    let before_identity = StoreIdentityLedger {
        version: 1,
        slots: BTreeMap::from([(
            "story.old_score".to_string(),
            "k0123456789abcdef".to_string(),
        )]),
    };
    let persisted = collect_persisted_state(
        &before_rename,
        Some(&before_identity),
        &identity_membership(Some(&before_identity)),
    )
    .state;

    let mut after_rename = SlotTable::new();
    after_rename
        .insert_namespace(
            "story",
            vec![(
                "score".to_string(),
                mod_slot(SlotType::Number, SlotValue::Number(0.0), true, None),
            )],
        )
        .unwrap();
    let renamed_identity = StoreIdentityLedger {
        version: 1,
        slots: BTreeMap::from([("story.score".to_string(), "k0123456789abcdef".to_string())]),
    };

    assert!(
        overlay_persisted_state(
            &mut after_rename,
            &persisted,
            Some(&renamed_identity),
            &identity_membership(Some(&renamed_identity)),
            None,
            Seat(0),
        )
        .is_empty()
    );
    assert_eq!(
        after_rename.get("story.score").unwrap().value,
        Some(SlotValue::Number(42.0))
    );
}

#[test]
fn deleted_ledger_entry_discards_saved_value_by_its_durable_key() {
    let mut table = SlotTable::new();
    table
        .insert_namespace(
            "story",
            vec![(
                "score".to_string(),
                mod_slot(SlotType::Number, SlotValue::Number(0.0), true, None),
            )],
        )
        .unwrap();
    let persisted = PersistedState {
        version: CURRENT_STATE_VERSION,
        slots: BTreeMap::from([(
            "k0123456789abcdef".to_string(),
            PersistedValue::Number(42.0),
        )]),
        per_owner: BTreeMap::new(),
        faction_sentiment: BTreeMap::new(),
    };

    let warnings = overlay_persisted_state(
        &mut table,
        &persisted,
        None,
        &BTreeSet::from(["story.score".to_string()]),
        None,
        Seat(0),
    );
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("k0123456789abcdef"));
    assert_eq!(
        table.get("story.score").unwrap().value,
        Some(SlotValue::Number(0.0))
    );
}

#[test]
fn v1_document_is_ignored_after_the_durable_key_format_bump() {
    let mut table = SlotTable::new();
    declare_fixture(&mut table);
    let warnings = overlay_persisted_state(
        &mut table,
        &PersistedState {
            version: 1,
            slots: BTreeMap::from([("game.score".to_string(), PersistedValue::Number(42.0))]),
            per_owner: BTreeMap::new(),
            faction_sentiment: BTreeMap::new(),
        },
        Some(&fixture_identity()),
        &identity_membership(Some(&fixture_identity())),
        None,
        Seat(0),
    );

    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("version 1"));
    assert_eq!(
        table.get("game.score").unwrap().value,
        Some(SlotValue::Number(10.0))
    );
}

#[test]
fn state_paths_are_per_mod_under_the_given_data_dir() {
    let data_dir = Path::new("/tmp/state-test-data/my-game");
    let first = state_path(Some(data_dir), "first.mod").unwrap();
    let second = state_path(Some(data_dir), "second.mod").unwrap();

    assert_ne!(first, second);
    assert_eq!(first, data_dir.join("first.mod/state.json"));
    assert_eq!(second, data_dir.join("second.mod/state.json"));
    assert_eq!(
        first
            .components()
            .filter(|component| component.as_os_str() == "my-game")
            .count(),
        1
    );
    assert!(
        state_path(None, "first.mod").is_none(),
        "an unavailable platform data directory must not fall back to cwd"
    );
}

// Regression: the add-only table and retained ledger kept saving a slot
// removed by the latest successful manifest commit.
#[test]
fn committed_declaration_membership_filters_retained_live_slots() {
    let mut table = SlotTable::new();
    table
        .insert_namespace(
            "old",
            vec![(
                "score".to_string(),
                mod_slot(SlotType::Number, SlotValue::Number(7.0), true, None),
            )],
        )
        .unwrap();
    table
        .insert_namespace(
            "current",
            vec![(
                "score".to_string(),
                mod_slot(SlotType::Number, SlotValue::Number(11.0), true, None),
            )],
        )
        .unwrap();
    let identity = StoreIdentityLedger {
        version: 1,
        slots: BTreeMap::from([
            ("old.score".to_string(), "k0123456789abcdef".to_string()),
            ("current.score".to_string(), "kfedcba9876543210".to_string()),
        ]),
    };
    let current_membership = BTreeSet::from(["current.score".to_string()]);

    let collected = collect_persisted_state(&table, Some(&identity), &current_membership).state;
    assert_eq!(collected.slots.len(), 1);
    assert_eq!(
        collected.slots.get("kfedcba9876543210"),
        Some(&PersistedValue::Number(11.0))
    );
    assert!(!collected.slots.contains_key("k0123456789abcdef"));

    let after_no_start = collect_persisted_state(&table, Some(&identity), &BTreeSet::new());
    assert!(
        after_no_start.state.slots.is_empty(),
        "an empty successful commit must not save through stale identity entries"
    );
}

#[test]
fn save_uses_a_sibling_temp_file_and_retained_snapshot_key() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("state.json");
    fs::write(&path, b"old-state").unwrap();
    let mut table = SlotTable::new();
    declare_fixture(&mut table);
    table.get_mut("game.score").unwrap().value = Some(SlotValue::Number(7.0));
    let snapshot = fixture_identity();
    let mut hand_edited = snapshot.clone();
    hand_edited
        .slots
        .insert("game.score".to_string(), "kffffffffffffffff".to_string());

    let collected = collect_persisted_state(
        &table,
        Some(&snapshot),
        &identity_membership(Some(&snapshot)),
    );
    assert!(collected.state.slots.contains_key("k0000000000000001"));
    assert!(
        !collected
            .state
            .slots
            .contains_key(hand_edited.durable_key("game.score").unwrap())
    );
    save_persisted_state(&path, &collected.state).unwrap();

    assert!(!tmp_path_for(&path).exists());
    let document: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(document["slots"]["k0000000000000001"], 7.0);
}

#[test]
fn join_seed_uses_only_the_local_players_retained_values() {
    let local_id = player_id(0x11);
    let other_id = player_id(0x22);
    let persisted = PersistedState {
        version: CURRENT_STATE_VERSION,
        slots: BTreeMap::new(),
        per_owner: BTreeMap::from([
            (
                "k0000000000000010".to_string(),
                BTreeMap::from([
                    (
                        encode_player_claim_id(&PlayerClaimId(local_id)),
                        PersistedValue::Number(42.0),
                    ),
                    (
                        encode_player_claim_id(&PlayerClaimId(other_id)),
                        PersistedValue::Number(99.0),
                    ),
                ]),
            ),
            (
                "k0000000000000011".to_string(),
                BTreeMap::from([(
                    encode_player_claim_id(&PlayerClaimId(local_id)),
                    PersistedValue::Unsupported(serde_json::json!({ "legacy": true })),
                )]),
            ),
        ]),
        faction_sentiment: BTreeMap::new(),
    };

    assert_eq!(
        join_seed_from_persisted_state(Some(&persisted), Some(local_id)),
        BTreeMap::from([("k0000000000000010".to_string(), JoinSeedValue::Number(42.0),)])
    );
    assert!(join_seed_from_persisted_state(None, Some(local_id)).is_empty());
    assert!(join_seed_from_persisted_state(Some(&persisted), None).is_empty());
}

#[test]
fn join_seed_applies_only_valid_persistent_per_owner_entries() {
    let mut table = SlotTable::new();
    declare_fixture(&mut table);
    let identity = fixture_identity();
    let membership = identity_membership(Some(&identity));

    let outcome = apply_join_seed(
        &mut table,
        Some(&identity),
        &membership,
        Seat(7),
        BTreeMap::from([
            ("k0000000000000010".to_string(), JoinSeedValue::Number(42.0)),
            ("k0000000000000011".to_string(), JoinSeedValue::Number(7.0)),
            ("k0000000000000012".to_string(), JoinSeedValue::Number(5.0)),
            ("kmadeup000000000".to_string(), JoinSeedValue::Boolean(true)),
        ]),
    );

    assert_eq!(outcome.warnings.len(), 3);
    assert_eq!(outcome.applied, 1);
    assert_eq!(
        table.get("game.xp").unwrap().per_seat_value(Seat(7)),
        Some(&SlotValue::Number(42.0))
    );
    assert_eq!(
        table.get("game.rank").unwrap().per_seat_value(Seat(7)),
        Some(&SlotValue::Enum("rookie".to_string()))
    );
    assert_eq!(
        table
            .get("game.session_xp")
            .unwrap()
            .per_seat_value(Seat(7)),
        Some(&SlotValue::Number(0.0))
    );
}

#[test]
fn join_seed_rejects_out_of_range_numbers_instead_of_clamping_them() {
    let mut table = SlotTable::new();
    declare_fixture(&mut table);
    let identity = fixture_identity();
    let membership = identity_membership(Some(&identity));

    let outcome = apply_join_seed(
        &mut table,
        Some(&identity),
        &membership,
        Seat(7),
        BTreeMap::from([(
            "k0000000000000010".to_string(),
            JoinSeedValue::Number(500.0),
        )]),
    );

    assert_eq!(outcome.warnings.len(), 1);
    assert_eq!(outcome.applied, 0);
    assert_eq!(
        table.get("game.xp").unwrap().per_seat_value(Seat(7)),
        Some(&SlotValue::Number(0.0))
    );
}
