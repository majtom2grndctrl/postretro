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
    let original = match fs::read_to_string(&ledger_path) {
        Ok(json) => Some(json),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(format!(
                "read identity ledger {}: {error}",
                ledger_path.display()
            ));
        }
    };
    let mut ledger = original
        .as_deref()
        .map(StoreIdentityLedger::parse)
        .transpose()
        .map_err(|error| format!("read identity ledger {}: {error}", ledger_path.display()))?
        .unwrap_or_else(StoreIdentityLedger::empty);
    let mut existing_keys = ledger.slots.values().cloned().collect::<BTreeSet<_>>();
    let mut additions = Vec::new();

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
            ledger
                .slots
                .insert(authored_name.clone(), durable_key.clone());
            additions.push((authored_name, durable_key));
        }
    }

    // A complete ledger is a no-write path: it preserves an author's exact
    // formatting and never leaves a temporary sibling behind.
    if additions.is_empty() {
        return Ok(0);
    }

    let serialized = match original {
        Some(original) => append_ledger_entries(&original, &additions)?,
        None => ledger.serialize_pretty().map_err(|error| {
            format!(
                "serialize identity ledger {}: {error}",
                ledger_path.display()
            )
        })?,
    };
    let reparsed = StoreIdentityLedger::parse(&serialized).map_err(|error| {
        format!(
            "validate updated identity ledger {}: {error}",
            ledger_path.display()
        )
    })?;
    if reparsed != ledger {
        return Err(format!(
            "validate updated identity ledger {}: appended document changed existing entries",
            ledger_path.display()
        ));
    }
    write_ledger_atomically(&ledger_path, &serialized)?;
    Ok(additions.len())
}

/// Append only new members at the validated `slots` object boundary. Every byte
/// from the author's input stays in the same order; the required separator is a
/// standalone insertion so even the preceding final-entry line stays untouched.
fn append_ledger_entries(original: &str, additions: &[(String, String)]) -> Result<String, String> {
    let close = slots_object_close(original)?;
    let had_entries = !StoreIdentityLedger::parse(original)
        .map_err(|error| format!("parse identity ledger before append: {error}"))?
        .slots
        .is_empty();
    let newline = if original.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let close_line_start = original[..close]
        .rfind('\n')
        .map_or(0, |line_break| line_break + 1);
    let close_indent = &original[close_line_start..close];
    let close_indent = if close_indent.chars().all(char::is_whitespace) {
        close_indent
    } else {
        ""
    };
    let entry_indent = format!("{close_indent}  ");

    let mut insertion = String::new();
    if had_entries {
        insertion.push(',');
    }
    for (index, (authored_name, durable_key)) in additions.iter().enumerate() {
        insertion.push_str(newline);
        insertion.push_str(&entry_indent);
        insertion.push_str(
            &serde_json::to_string(authored_name)
                .map_err(|error| format!("serialize authored slot name: {error}"))?,
        );
        insertion.push_str(": ");
        insertion.push_str(
            &serde_json::to_string(durable_key)
                .map_err(|error| format!("serialize durable key: {error}"))?,
        );
        if index + 1 != additions.len() {
            insertion.push(',');
        }
    }
    insertion.push_str(newline);
    insertion.push_str(close_indent);

    let mut updated = String::with_capacity(original.len() + insertion.len());
    updated.push_str(&original[..close]);
    updated.push_str(&insertion);
    updated.push_str(&original[close..]);
    Ok(updated)
}

fn slots_object_close(json: &str) -> Result<usize, String> {
    let bytes = json.as_bytes();
    let mut depth = 0_u32;
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'{' => {
                depth += 1;
                index += 1;
            }
            b'}' => {
                depth = depth.saturating_sub(1);
                index += 1;
            }
            b'"' => {
                let end = json_string_end(bytes, index)?;
                if depth == 1 {
                    let key: String = serde_json::from_str(&json[index..end])
                        .map_err(|error| format!("parse identity ledger object key: {error}"))?;
                    let mut cursor = skip_json_whitespace(bytes, end);
                    if key == "slots" && bytes.get(cursor) == Some(&b':') {
                        cursor = skip_json_whitespace(bytes, cursor + 1);
                        if bytes.get(cursor) != Some(&b'{') {
                            return Err("identity ledger `slots` must be an object".to_string());
                        }
                        return matching_object_close(bytes, cursor);
                    }
                }
                index = end;
            }
            _ => index += 1,
        }
    }
    Err("identity ledger has no top-level `slots` object".to_string())
}

fn matching_object_close(bytes: &[u8], open: usize) -> Result<usize, String> {
    let mut depth = 0_u32;
    let mut index = open;
    while index < bytes.len() {
        match bytes[index] {
            b'{' => {
                depth += 1;
                index += 1;
            }
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(index);
                }
                index += 1;
            }
            b'"' => index = json_string_end(bytes, index)?,
            _ => index += 1,
        }
    }
    Err("identity ledger `slots` object is not closed".to_string())
}

fn json_string_end(bytes: &[u8], start: usize) -> Result<usize, String> {
    let mut index = start + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 2,
            b'"' => return Ok(index + 1),
            _ => index += 1,
        }
    }
    Err("identity ledger contains an unterminated string".to_string())
}

fn skip_json_whitespace(bytes: &[u8], mut index: usize) -> usize {
    while bytes
        .get(index)
        .is_some_and(|byte| byte.is_ascii_whitespace())
    {
        index += 1;
    }
    index
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
                globalThis.__postretroModManifest = defineMod({{
                    name: "Mint fixture",
                    id: "mint-fixture",
                    version: "1",
                    stores: [store],
                }});
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
        let existing_line = format!("\t\t\"legacy.value\" : \"{EXISTING_KEY}\"");
        fs::write(
            root.join(IDENTITY_FILE_NAME),
            format!(
                "{{\r\n\t\"slots\" : {{\r\n{existing_line}\r\n\t}},\r\n\t\"version\" : 1\r\n}}\r\n"
            ),
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
        assert!(
            text.contains(&format!("{existing_line}\r\n\t,\r\n")),
            "the separator is inserted after, not into, the preceding final-entry line"
        );
        assert!(text.ends_with("\t\"version\" : 1\r\n}\r\n"));
        assert_eq!(
            durable_slot_names(root),
            BTreeSet::from(["computed.value".to_string(), "legacy.value".to_string()]),
        );
    }

    #[test]
    fn ledger_append_preserves_custom_order_and_adds_commas_between_new_entries() {
        let original = format!(
            "{{\n  \"slots\": {{\n    \"old.one\"  :  \"{EXISTING_KEY}\"\n  }},\n  \"version\": 1\n}}\n"
        );
        let additions = vec![
            ("new.alpha".to_string(), "k1111111111111111".to_string()),
            ("new.beta".to_string(), "k2222222222222222".to_string()),
        ];

        let updated = append_ledger_entries(&original, &additions).expect("append entries");

        assert!(updated.contains(&format!("    \"old.one\"  :  \"{EXISTING_KEY}\"\n  ,\n")));
        assert!(updated.contains(
            "\"new.alpha\": \"k1111111111111111\",\n    \"new.beta\": \"k2222222222222222\""
        ));
        assert!(updated.ends_with("  \"version\": 1\n}\n"));
        let parsed = StoreIdentityLedger::parse(&updated).expect("updated ledger validates");
        assert_eq!(parsed.slots.len(), 3);
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
                return defineMod({
                    name = "Mint fixture",
                    id = "mint-fixture",
                    version = "1",
                    stores = { store },
                })
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
