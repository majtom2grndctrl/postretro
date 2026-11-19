// Shared save-slot fixtures: a session for the `dev` mod with a world
// global, a run-scoped global and a per-owner slot, saving under a temp dir.

use std::collections::BTreeSet;
use std::path::Path;

use postretro_entities::slot_table::{
    ReplicationScope, SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue,
};
use postretro_foundation::Seat;
use postretro_scripting_core::runtime::SavesManifest;

use crate::App;
use crate::startup::lifecycle::tests::{script_ctx, test_app};

pub(crate) const MOD_ID: &str = "dev";
pub(crate) const PLAYER: [u8; 16] = [0x7c; 16];
pub(crate) const OTHER: [u8; 16] = [0x11; 16];
pub(crate) const SCORE: &str = "game.score";
pub(crate) const GENERATOR: &str = "game.generator";
pub(crate) const XP: &str = "game.xp";

pub(crate) fn persisted(per_owner: bool) -> SlotRecord {
    SlotRecord::new(SlotSchema {
        slot_type: SlotType::Number,
        default: Some(SlotValue::Number(0.0)),
        range: None,
        persist: true,
        readonly: false,
        ownership: SlotOwnership::Mod,
        network: ReplicationScope::None,
        per_owner,
        accumulate: None,
    })
}

/// A session for the `dev` mod with a world global, a run-scoped global and a
/// per-owner slot, each with durable identity, saving under `data_dir`.
pub(crate) fn app_with_store(data_dir: &Path) -> App {
    let mut app = test_app();
    let session = app.session.as_mut().unwrap();
    session.data_dir = Some(data_dir.to_path_buf());
    session.player_options.player_id = Some(PLAYER);
    session
        .seat_table
        .get_or_insert_with(crate::netcode::SeatTable::local_only);
    session
        .scripting
        .script_ctx
        .slot_table
        .borrow_mut()
        .insert_namespace(
            "game",
            vec![
                ("score".into(), persisted(false)),
                ("generator".into(), persisted(false)),
                ("xp".into(), persisted(true)),
            ],
        )
        .unwrap();
    let mut ledger = postretro_scripting_core::store_identity::StoreIdentityLedger::empty();
    for (name, key) in [(SCORE, "k-score"), (GENERATOR, "k-gen"), (XP, "k-xp")] {
        ledger.slots.insert(name.into(), key.into());
    }
    let runtime = &mut session.scripting.script_runtime;
    runtime.commit_mod_identity_for_test(MOD_ID, "1");
    runtime.commit_store_identity_for_test(
        ledger,
        [SCORE.to_string(), GENERATOR.to_string(), XP.to_string()].into(),
    );
    runtime.commit_run_scoped_slots_for_test(BTreeSet::from([GENERATOR.to_string()]));
    app
}

pub(crate) fn boot(app: &mut App, declared: Option<SavesManifest>) {
    let session = app.session.as_mut().unwrap();
    super::boot_restore_with(session, declared);
    session.state_store_lifecycle.mark_restore_completed();
}

pub(crate) fn number(app: &App, name: &str) -> Option<SlotValue> {
    script_ctx(app).slot_table.borrow().get(name)?.value.clone()
}

pub(crate) fn xp(app: &App) -> Option<SlotValue> {
    script_ctx(app)
        .slot_table
        .borrow()
        .get(XP)?
        .per_seat_value(Seat(0))
        .cloned()
}

/// Register a frontend menu and present it, as boot does for a mod with a
/// frontend: the machine then stands at its frontend.
pub(crate) fn present_frontend(app: &mut App) {
    let session = app.session.as_mut().unwrap();
    session.modal_stack.registry_mut().register(
        "mainMenu",
        postretro_ui::demo::build_frontend_menu_descriptor(),
        postretro_ui::modal_stack::ScopeTier::Mod,
        false,
    );
    session.frontend = Some(postretro_scripting_core::runtime::Frontend {
        menu_tree: "mainMenu".to_string(),
        background_level: None,
        camera: postretro_scripting_core::runtime::MenuCamera {
            position: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
        },
    });
    app.present_frontend_menu();
}
