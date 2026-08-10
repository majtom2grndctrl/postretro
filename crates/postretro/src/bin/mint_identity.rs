// Authoring tool for assigning durable identities to a mod's declared slots.
// See: context/lib/scripting.md §5

#![deny(unsafe_code)]

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use postretro_entities::ctx::ScriptCtx;
use postretro_scripting_core::primitives_registry::PrimitiveRegistry;
use postretro_scripting_core::runtime::{ScriptRuntime, ScriptRuntimeConfig};
use postretro_scripting_core::store_identity::{
    IDENTITY_FILE_NAME, StoreIdentityLedger, generate_durable_key, requires_durable_key,
};

#[path = "../scripting"]
mod scripting {
    #![allow(dead_code, unused_imports)]

    pub(crate) mod entity_world_primitives;
    pub(crate) mod primitives;
    pub(crate) mod state_store;
}

use scripting::primitives::register_all;

fn main() -> ExitCode {
    env_logger::try_init().ok();

    match try_main(std::env::args_os()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("mint-identity: {error}");
            ExitCode::from(1)
        }
    }
}

fn try_main<I>(args: I) -> Result<(), String>
where
    I: IntoIterator<Item = OsString>,
{
    let mod_root = parse_mod_root(args)?;
    let added = mint_identity(&mod_root)?;
    println!(
        "mint-identity: added {added} durable slot {} to {}",
        if added == 1 { "entry" } else { "entries" },
        mod_root.join(IDENTITY_FILE_NAME).display(),
    );
    Ok(())
}

fn parse_mod_root<I>(args: I) -> Result<PathBuf, String>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args = args.into_iter();
    let _program = args.next();
    let Some(mod_root) = args.next() else {
        return Err("usage: mint-identity <mod-root>".to_string());
    };
    if args.next().is_some() {
        return Err("usage: mint-identity <mod-root>".to_string());
    }
    Ok(PathBuf::from(mod_root))
}

/// Execute a mod's real declaration path, then append missing durable-slot
/// identities to its author-owned ledger. This is deliberately the only runtime
/// construction that bypasses identity enforcement: it needs to observe an
/// incomplete ledger before it can mint the missing entries.
fn mint_identity(mod_root: &Path) -> Result<usize, String> {
    let script_ctx = ScriptCtx::new();
    let mut primitive_registry = PrimitiveRegistry::new();
    register_all(&mut primitive_registry, script_ctx.clone());
    let runtime_config = ScriptRuntimeConfig {
        skip_identity_enforcement: true,
        ..ScriptRuntimeConfig::default()
    };
    let mut script_runtime = ScriptRuntime::new(&primitive_registry, &runtime_config, &script_ctx)
        .map_err(|error| format!("construct script runtime: {error}"))?;

    script_runtime
        .run_mod_init(mod_root)
        .map_err(|error| format!("run mod-init for {}: {error}", mod_root.display()))?;
    let manifest = script_runtime.mod_manifest().ok_or_else(|| {
        format!(
            "no mod manifest found at {}; expected start-script.{{ts,js,luau}}",
            mod_root.display()
        )
    })?;

    reconcile_ledger(mod_root, &manifest.store_declarations)
}

fn reconcile_ledger(
    mod_root: &Path,
    declarations: &postretro_entities::slot_table::StoreDeclarationSet,
) -> Result<usize, String> {
    let ledger_path = mod_root.join(IDENTITY_FILE_NAME);
    let mut ledger = StoreIdentityLedger::read_from_mod_root(mod_root)
        .map_err(|error| format!("read identity ledger {}: {error}", ledger_path.display()))?
        .unwrap_or_else(StoreIdentityLedger::empty);
    let mut existing_keys = ledger.slots.values().cloned().collect::<BTreeSet<_>>();
    let mut added = 0;

    for declaration in declarations.iter() {
        for (slot_name, record) in &declaration.records {
            if !requires_durable_key(record) {
                continue;
            }

            let authored_name = format!("{}.{}", declaration.namespace, slot_name);
            if ledger.slots.contains_key(&authored_name) {
                continue;
            }

            let durable_key = next_unassigned_key(&existing_keys)?;
            existing_keys.insert(durable_key.clone());
            ledger.slots.insert(authored_name, durable_key);
            added += 1;
        }
    }

    // A complete ledger is a no-write path: it preserves an author's exact
    // formatting and never leaves a temporary sibling behind.
    if added == 0 {
        return Ok(0);
    }

    let serialized = ledger.serialize_pretty().map_err(|error| {
        format!(
            "serialize identity ledger {}: {error}",
            ledger_path.display()
        )
    })?;
    write_ledger_atomically(&ledger_path, &serialized)?;
    Ok(added)
}

fn next_unassigned_key(existing_keys: &BTreeSet<String>) -> Result<String, String> {
    loop {
        let durable_key = generate_durable_key()
            .map_err(|error| format!("generate durable identity key: {error}"))?;
        if !existing_keys.contains(&durable_key) {
            return Ok(durable_key);
        }
    }
}

fn write_ledger_atomically(ledger_path: &Path, serialized: &str) -> Result<(), String> {
    let temporary_path = ledger_path.with_file_name(format!("{IDENTITY_FILE_NAME}.tmp"));
    if let Err(error) = fs::write(&temporary_path, serialized) {
        let _ = fs::remove_file(&temporary_path);
        return Err(format!(
            "write identity ledger {} via {}: {error}",
            ledger_path.display(),
            temporary_path.display(),
        ));
    }
    if let Err(error) = fs::rename(&temporary_path, ledger_path) {
        let _ = fs::remove_file(&temporary_path);
        return Err(format!(
            "replace identity ledger {} from {}: {error}",
            ledger_path.display(),
            temporary_path.display(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use postretro_scripting_core::store_identity::is_durable_key;
    use tempfile::tempdir;

    const EXISTING_KEY: &str = "k0123456789abcdef";

    fn quickjs_manifest(stores: &str) -> String {
        format!(
            r#"
                {stores}
                globalThis.__postretroModManifest = {{
                    name: "Mint fixture",
                    id: "mint-fixture",
                    version: "1",
                    stores: [store.declaration],
                }};
            "#
        )
    }

    fn write_quickjs_mod(root: &Path, stores: &str) {
        fs::write(root.join("start-script.js"), quickjs_manifest(stores))
            .expect("write QuickJS mod manifest");
    }

    fn durable_slot_names(root: &Path) -> BTreeSet<String> {
        StoreIdentityLedger::read_from_mod_root(root)
            .expect("read minted ledger")
            .expect("minted ledger exists")
            .slots
            .into_keys()
            .collect()
    }

    #[test]
    fn mint_identity_appends_missing_entries_once_and_is_idempotent() {
        let temp = tempdir().expect("temporary mod root");
        let root = temp.path();
        write_quickjs_mod(
            root,
            r#"
                const store = defineStore("story", {
                    score: { type: "number", default: 0, persist: true },
                    transient: { type: "number", default: 0 },
                });
            "#,
        );

        assert_eq!(mint_identity(root).expect("first mint succeeds"), 1);
        let ledger_path = root.join(IDENTITY_FILE_NAME);
        let first = fs::read_to_string(&ledger_path).expect("read first ledger");
        let ledger = StoreIdentityLedger::read_from_mod_root(root)
            .expect("parse ledger")
            .expect("ledger exists");
        assert_eq!(ledger.slots.len(), 1);
        assert!(is_durable_key(&ledger.slots["story.score"]));

        assert_eq!(mint_identity(root).expect("second mint succeeds"), 0);
        assert_eq!(
            fs::read_to_string(&ledger_path).expect("read idempotent ledger"),
            first,
        );
        assert!(!root.join(format!("{IDENTITY_FILE_NAME}.tmp")).exists());
    }

    #[test]
    fn mint_identity_keeps_existing_entries_and_mints_computed_name_and_schema() {
        let temp = tempdir().expect("temporary mod root");
        let root = temp.path();
        let existing_line = format!(r#"    "legacy.value": "{EXISTING_KEY}""#);
        fs::write(
            root.join(IDENTITY_FILE_NAME),
            format!("{{\n  \"version\": 1,\n  \"slots\": {{\n{existing_line}\n  }}\n}}"),
        )
        .expect("write existing ledger");
        write_quickjs_mod(
            root,
            r#"
                const namespace = "computed";
                const slotName = "value";
                const schema = {
                    [slotName]: { type: "number", default: 0, persist: true },
                };
                const store = defineStore(namespace, schema);
            "#,
        );

        assert_eq!(mint_identity(root).expect("mint succeeds"), 1);
        let text = fs::read_to_string(root.join(IDENTITY_FILE_NAME)).expect("read minted ledger");
        assert!(text.contains(&existing_line));
        assert_eq!(
            durable_slot_names(root),
            BTreeSet::from(["computed.value".to_string(), "legacy.value".to_string()]),
        );
    }

    #[test]
    fn mint_identity_mints_equivalent_quickjs_and_luau_declarations() {
        let quickjs = tempdir().expect("temporary QuickJS mod root");
        write_quickjs_mod(
            quickjs.path(),
            r#"
                const computedName = "shared";
                const schema = { score: { type: "number", default: 0, persist: true } };
                const store = defineStore(computedName, schema);
            "#,
        );
        mint_identity(quickjs.path()).expect("mint QuickJS mod");

        let luau = tempdir().expect("temporary Luau mod root");
        fs::write(
            luau.path().join("start-script.luau"),
            r#"
                local computedName = "shared"
                local schema = {
                    score = { type = "number", default = 0, persist = true },
                }
                local store = defineStore(computedName, schema)
                return {
                    name = "Mint fixture",
                    id = "mint-fixture",
                    version = "1",
                    stores = { store.declaration },
                }
            "#,
        )
        .expect("write Luau mod manifest");
        mint_identity(luau.path()).expect("mint Luau mod");

        assert_eq!(
            durable_slot_names(quickjs.path()),
            durable_slot_names(luau.path())
        );
    }

    #[test]
    fn mint_identity_write_failure_preserves_existing_ledger() {
        let temp = tempdir().expect("temporary mod root");
        let root = temp.path();
        let original = format!(r#"{{"version":1,"slots":{{"legacy.value":"{EXISTING_KEY}"}}}}"#);
        fs::write(root.join(IDENTITY_FILE_NAME), &original).expect("write existing ledger");
        fs::create_dir(root.join(format!("{IDENTITY_FILE_NAME}.tmp")))
            .expect("create blocking temporary path");
        write_quickjs_mod(
            root,
            r#"
                const store = defineStore("story", {
                    score: { type: "number", default: 0, persist: true },
                });
            "#,
        );

        let error = mint_identity(root).expect_err("temporary write must fail");
        assert!(error.contains(&root.join(IDENTITY_FILE_NAME).display().to_string()));
        assert_eq!(
            fs::read_to_string(root.join(IDENTITY_FILE_NAME)).expect("read original ledger"),
            original,
        );
    }

    #[test]
    fn mint_identity_rejects_mod_root_without_manifest() {
        let temp = tempdir().expect("temporary mod root");
        let error = mint_identity(temp.path()).expect_err("missing manifest must reject");

        assert!(error.contains("no mod manifest"));
        assert!(!temp.path().join(IDENTITY_FILE_NAME).exists());
    }

    #[test]
    fn parse_mod_root_requires_exactly_one_argument() {
        assert_eq!(
            parse_mod_root([
                OsString::from("mint-identity"),
                OsString::from("content/mod")
            ])
            .expect("one path is valid"),
            PathBuf::from("content/mod"),
        );
        assert!(parse_mod_root([OsString::from("mint-identity")]).is_err());
        assert!(
            parse_mod_root([
                OsString::from("mint-identity"),
                OsString::from("one"),
                OsString::from("two"),
            ])
            .is_err()
        );
    }
}
