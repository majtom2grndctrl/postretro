// Scripting subsystem: Rust-owned entity/component APIs, engine-global typed state, and persistence.
// See: context/lib/scripting.md for governing scripting contracts and ownership.

// Renderer, audio, and input own their own data structures and are unaffected
// by anything in this module.

#![deny(unsafe_code)]
// Some component types not yet wired to their bridges — silence dead-code at
// the subsystem level rather than scattering item-level annotations.
#![allow(dead_code)]

pub(crate) mod builtins;
pub(crate) mod entity_world_primitives;
pub(crate) mod map_entity;
pub(crate) mod primitives;
pub(crate) mod reactions;
pub(crate) mod state_persistence;
pub(crate) mod state_store;
pub(crate) mod typedef;

#[cfg(test)]
mod rust_source_scan {
    pub(super) fn mask_comments_and_string_literals(source: &str) -> String {
        let mut masked = String::with_capacity(source.len());
        let mut chars = source.char_indices().peekable();
        let mut block_comment_depth = 0usize;
        let mut in_line_comment = false;
        let mut in_string = false;
        let mut string_escape = false;
        let mut raw_string_hashes: Option<usize> = None;

        while let Some((index, ch)) = chars.next() {
            let next = chars.peek().map(|(_, next)| *next);

            if in_line_comment {
                if ch == '\n' {
                    in_line_comment = false;
                    masked.push('\n');
                } else {
                    push_masked_char(&mut masked, ch);
                }
                continue;
            }

            if block_comment_depth > 0 {
                if ch == '/' && next == Some('*') {
                    block_comment_depth += 1;
                    push_masked_char(&mut masked, ch);
                    if let Some((_, next_ch)) = chars.next() {
                        push_masked_char(&mut masked, next_ch);
                    }
                } else if ch == '*' && next == Some('/') {
                    block_comment_depth -= 1;
                    push_masked_char(&mut masked, ch);
                    if let Some((_, next_ch)) = chars.next() {
                        push_masked_char(&mut masked, next_ch);
                    }
                } else if ch == '\n' {
                    masked.push('\n');
                } else {
                    push_masked_char(&mut masked, ch);
                }
                continue;
            }

            if let Some(hash_count) = raw_string_hashes {
                if raw_string_closes(source, index, hash_count) {
                    raw_string_hashes = None;
                    push_masked_char(&mut masked, ch);
                    for _ in 0..hash_count {
                        if let Some((_, next_ch)) = chars.next() {
                            push_masked_char(&mut masked, next_ch);
                        }
                    }
                } else if ch == '\n' {
                    masked.push('\n');
                } else {
                    push_masked_char(&mut masked, ch);
                }
                continue;
            }

            if in_string {
                if ch == '\n' {
                    masked.push('\n');
                } else {
                    push_masked_char(&mut masked, ch);
                }
                if string_escape {
                    string_escape = false;
                } else if ch == '\\' {
                    string_escape = true;
                } else if ch == '"' {
                    in_string = false;
                }
                continue;
            }

            if ch == '/' && next == Some('/') {
                in_line_comment = true;
                push_masked_char(&mut masked, ch);
                if let Some((_, next_ch)) = chars.next() {
                    push_masked_char(&mut masked, next_ch);
                }
            } else if ch == '/' && next == Some('*') {
                block_comment_depth = 1;
                push_masked_char(&mut masked, ch);
                if let Some((_, next_ch)) = chars.next() {
                    push_masked_char(&mut masked, next_ch);
                }
            } else if let Some(hash_count) = raw_string_start(source, index) {
                raw_string_hashes = Some(hash_count);
                push_masked_char(&mut masked, ch);
                for _ in 0..hash_count {
                    if let Some((_, next_ch)) = chars.next() {
                        push_masked_char(&mut masked, next_ch);
                    }
                }
                if let Some((_, next_ch)) = chars.next() {
                    push_masked_char(&mut masked, next_ch);
                }
            } else if ch == '"' {
                in_string = true;
                string_escape = false;
                push_masked_char(&mut masked, ch);
            } else {
                masked.push(ch);
            }
        }

        masked
    }

    fn push_masked_char(masked: &mut String, ch: char) {
        for _ in 0..ch.len_utf8() {
            masked.push(' ');
        }
    }

    fn raw_string_start(source: &str, index: usize) -> Option<usize> {
        let rest = source.get(index..)?;
        let mut chars = rest.chars();
        if chars.next()? != 'r' {
            return None;
        }

        let mut hash_count = 0usize;
        for ch in chars {
            match ch {
                '#' => hash_count += 1,
                '"' => return Some(hash_count),
                _ => return None,
            }
        }

        None
    }

    fn raw_string_closes(source: &str, index: usize, hash_count: usize) -> bool {
        let Some(rest) = source.get(index..) else {
            return false;
        };
        if !rest.starts_with('"') {
            return false;
        }

        rest[1..].starts_with(&"#".repeat(hash_count))
    }

    pub(super) fn is_identifier_continue(ch: char) -> bool {
        ch == '_' || ch.is_ascii_alphanumeric()
    }
}

#[cfg(test)]
mod extraction_path_tests {
    use super::rust_source_scan::{is_identifier_continue, mask_comments_and_string_literals};
    use std::fs;
    use std::path::{Path, PathBuf};

    const MOVED_PATH_SENTINELS: &[&str] = &[
        "components",
        "conv.rs",
        "ctx.rs",
        "data_registry.rs",
        "data_descriptors",
        "engine_state_catalog.rs",
        "error.rs",
        "foundation_pods.rs",
        "game_state_refs.rs",
        "ir",
        "ir/e2e_tests.rs",
        "ir/parity_tests.rs",
        "ir/scopes.rs",
        "ir/test_scope.rs",
        "ir_scopes.rs",
        "luau.rs",
        "luau_prelude.rs",
        "luau_require.rs",
        "luau_virtual_modules.rs",
        "primitive_adapters.rs",
        "primitives/entity.rs",
        "primitives/light.rs",
        "primitives/mod.rs",
        "primitives/store.rs",
        "primitives/world.rs",
        "primitives_registry.rs",
        "provenance.rs",
        "quickjs.rs",
        "reaction_dispatch.rs",
        "reaction_registry.rs",
        "reactions/mod.rs",
        "reactions/registry.rs",
        "reactions/system_commands.rs",
        "refresh_plan.rs",
        "registry.rs",
        "runtime",
        "sequence.rs",
        "slot_table.rs",
        "staged_manifest.rs",
        "state_crossings.rs",
        "store_bridge.rs",
        "typedef/common.rs",
        "typedef/luau.rs",
        "typedef/mod.rs",
        "typedef/tests",
        "typedef/ts.rs",
        "ui",
        "value_types.rs",
        "watcher.rs",
    ];

    const INTENTIONAL_COMPATIBILITY_OR_FIXTURE_PATHS: &[&str] = &[
        "primitives/entity.rs",
        "primitives/light.rs",
        "primitives/mod.rs",
        "primitives/store.rs",
        "primitives/world.rs",
        "reactions/mod.rs",
        "reactions/registry.rs",
        "reactions/system_commands.rs",
        "typedef/mod.rs",
        "typedef/tests",
    ];

    const MOVED_MODULE_SENTINELS: &[&str] = &[
        "components",
        "conv",
        "ctx",
        "data_registry",
        "data_descriptors",
        "engine_state_catalog",
        "error",
        "foundation_pods",
        "game_state_refs",
        "ir",
        "ir_scopes",
        "luau",
        "luau_prelude",
        "luau_require",
        "luau_virtual_modules",
        "primitives_registry",
        "provenance",
        "quickjs",
        "reaction_dispatch",
        "refresh_plan",
        "registry",
        "runtime",
        "sequence",
        "slot_table",
        "staged_manifest",
        "state_crossings",
        "value_types",
        "watcher",
    ];

    #[test]
    fn scripting_core_extraction_deleted_paths_do_not_reappear() {
        let scripting_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/scripting");
        let unexpected = MOVED_PATH_SENTINELS
            .iter()
            .copied()
            .filter(|path| {
                !INTENTIONAL_COMPATIBILITY_OR_FIXTURE_PATHS.contains(path)
                    && implementation_path_exists(&scripting_dir.join(path))
            })
            .map(PathBuf::from)
            .collect::<Vec<_>>();

        assert!(
            unexpected.is_empty(),
            "scripting-core implementation paths reappeared under crates/postretro/src/scripting: {unexpected:?}. Keep VM substrate in crates/scripting-core; only the allowlisted compatibility barrels and typedef fixtures may remain here."
        );
    }

    #[test]
    fn scripting_core_extraction_barrel_declarations_do_not_reappear() {
        let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let declaration_roots = [
            Path::new("scripting/mod.rs"),
            Path::new("bin/gen_script_types.rs"),
        ];
        let mut unexpected = Vec::new();

        for relative_path in declaration_roots {
            let path = src_dir.join(relative_path);
            let Ok(source) = fs::read_to_string(path) else {
                continue;
            };

            for module in MOVED_MODULE_SENTINELS {
                if contains_module_declaration(&source, module) {
                    unexpected.push(format!("{} declares mod {module}", relative_path.display()));
                }
            }
        }

        assert!(
            unexpected.is_empty(),
            "removed/collapsed scripting barrels were declared again: {unexpected:?}. Keep floor/core APIs in postretro_foundation, postretro_entities, or postretro_scripting_core; only real postretro modules and retained compatibility fixtures may be declared here."
        );
    }

    #[test]
    fn barrel_declaration_scan_matches_module_names_not_prefixes() {
        assert!(contains_module_declaration(
            "pub(crate) mod registry;",
            "registry"
        ));
        assert!(!contains_module_declaration(
            "pub(crate) mod registry_extra;",
            "registry"
        ));
    }

    #[test]
    fn barrel_declaration_scan_matches_visibility_attributes_and_inline_modules() {
        assert!(contains_module_declaration(
            "pub(super) mod registry;",
            "registry"
        ));
        assert!(contains_module_declaration(
            "#[cfg(test)] pub(crate) mod registry;",
            "registry"
        ));
        assert!(contains_module_declaration(
            "pub(in crate::scripting) mod registry;",
            "registry"
        ));
        assert!(contains_module_declaration(
            "pub(crate) mod conv { }",
            "conv"
        ));
    }

    #[test]
    fn barrel_declaration_scan_ignores_comments_and_strings() {
        let source = r###"
            // mod registry;
            /* pub(crate) mod ctx; */
            let _literal = "mod conv;";
            let _raw = r#"pub(super) mod watcher;"#;
        "###;

        assert!(!contains_module_declaration(source, "registry"));
        assert!(!contains_module_declaration(source, "ctx"));
        assert!(!contains_module_declaration(source, "conv"));
        assert!(!contains_module_declaration(source, "watcher"));
    }

    fn implementation_path_exists(path: &Path) -> bool {
        if path.is_file() {
            return true;
        }
        path.is_dir() && contains_file(path)
    }

    fn contains_file(dir: &Path) -> bool {
        let Ok(entries) = fs::read_dir(dir) else {
            return false;
        };

        entries.filter_map(Result::ok).any(|entry| {
            let path = entry.path();
            path.is_file() || (path.is_dir() && contains_file(&path))
        })
    }

    fn contains_module_declaration(source: &str, module: &str) -> bool {
        mask_comments_and_string_literals(source)
            .lines()
            .any(|line| line_contains_module_declaration(line, module))
    }

    fn line_contains_module_declaration(line: &str, module: &str) -> bool {
        let Some(mut rest) = strip_outer_attributes(line.trim_start()) else {
            return false;
        };

        rest = rest.trim_start();
        if let Some(after_visibility) = strip_visibility(rest) {
            rest = after_visibility.trim_start();
        }

        let Some(rest) = rest.strip_prefix("mod ") else {
            return false;
        };

        let Some(rest) = rest.strip_prefix(module) else {
            return false;
        };

        let rest = rest.trim_start();

        !rest.chars().next().is_some_and(is_identifier_continue)
            && (rest.starts_with(';') || rest.starts_with('{'))
    }

    fn strip_outer_attributes(mut rest: &str) -> Option<&str> {
        loop {
            rest = rest.trim_start();
            if !rest.starts_with("#[") {
                return Some(rest);
            }

            let attribute_end = bracketed_attribute_end(rest)?;
            rest = &rest[attribute_end..];
        }
    }

    fn bracketed_attribute_end(attribute: &str) -> Option<usize> {
        let mut depth = 0usize;

        for (index, ch) in attribute.char_indices() {
            match ch {
                '[' => depth += 1,
                ']' => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        return Some(index + ch.len_utf8());
                    }
                }
                _ => {}
            }
        }

        None
    }

    fn strip_visibility(rest: &str) -> Option<&str> {
        if let Some(rest) = rest.strip_prefix("pub ") {
            return Some(rest);
        }

        let rest = rest.strip_prefix("pub(")?;
        let closing_paren = rest.find(')')?;
        Some(&rest[closing_paren + 1..])
    }
}

#[cfg(test)]
mod scripting_boundary_tests {
    use super::rust_source_scan::{is_identifier_continue, mask_comments_and_string_literals};
    use std::fs;
    use std::path::{Path, PathBuf};

    const REMOVED_OR_COLLAPSED_BARRELS: &[&str] = &[
        "data_registry",
        "foundation_pods",
        "game_state_refs",
        "ir",
        "ir_scopes",
        "luau_require",
        "luau_virtual_modules",
        "refresh_plan",
        "luau",
        "quickjs",
        "watcher",
        "value_types",
        "registry",
        "components",
        "ctx",
        "slot_table",
        "provenance",
        "error",
        "engine_state_catalog",
        "conv",
        "reaction_dispatch",
        "runtime",
        "sequence",
        "staged_manifest",
        "state_crossings",
        "luau_prelude",
        "primitives_registry",
        "data_descriptors",
        "typedef",
    ];

    #[test]
    fn scripting_boundary_rejects_removed_barrel_imports() {
        let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut violations = Vec::new();

        collect_boundary_violations(&src_dir, &src_dir, &mut violations);

        assert!(
            violations.is_empty(),
            "removed/collapsed scripting barrels were imported through crate::scripting::<name> or crate::scripting::{{<name>...}} use trees: {violations:#?}. Import floor/core APIs from postretro_foundation, postretro_entities, or postretro_scripting_core directly."
        );
    }

    #[test]
    fn scripting_boundary_scan_ignores_comments_strings_and_prefixes() {
        let source = r###"
            // use crate::scripting::registry::EntityId;
            let _literal = "crate::scripting::ctx::ScriptCtx";
            let _raw = r#"crate::scripting::components::Transform"#;
            use crate::scripting::registry_extra::EntityId;
        "###;
        let scan_source = mask_comments_and_string_literals(source);

        let direct_hits = scan_source
            .match_indices("crate::scripting::")
            .filter_map(|(index, _)| direct_path_barrel(&scan_source[index..]))
            .collect::<Vec<_>>();

        assert!(direct_hits.is_empty());
    }

    #[test]
    fn scripting_boundary_scan_matches_direct_and_grouped_barrels() {
        assert_eq!(
            direct_path_barrel("crate::scripting::registry::EntityId"),
            Some("registry")
        );

        let use_item = "use crate::scripting::{registry::EntityId, ctx::{ScriptCtx, Other}};";
        let group = grouped_use_tree(use_item).expect("grouped scripting use");

        assert!(grouped_use_tree_contains_barrel(group, "registry"));
        assert!(grouped_use_tree_contains_barrel(group, "ctx"));
        assert!(!grouped_use_tree_contains_barrel(group, "registry_extra"));
    }

    #[test]
    fn scripting_boundary_scan_matches_nested_crate_grouped_barrels() {
        let nested_group = crate_grouped_use_tree("use crate::{scripting::{registry::EntityId}};")
            .expect("nested crate grouped use");
        assert!(crate_grouped_use_tree_contains_scripting_barrel(
            nested_group,
            "registry"
        ));

        let path_group = crate_grouped_use_tree("use crate::{scripting::registry::EntityId};")
            .expect("crate grouped scripting path use");
        assert!(crate_grouped_use_tree_contains_scripting_barrel(
            path_group, "registry"
        ));
        assert!(!crate_grouped_use_tree_contains_scripting_barrel(
            path_group,
            "registry_extra"
        ));
    }

    fn collect_boundary_violations(src_dir: &Path, path: &Path, violations: &mut Vec<String>) {
        let Ok(entries) = fs::read_dir(path) else {
            return;
        };

        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                collect_boundary_violations(src_dir, &path, violations);
            } else if is_rust_source(&path) && !is_allowlisted(src_dir, &path) {
                scan_rust_source(src_dir, &path, violations);
            }
        }
    }

    fn scan_rust_source(src_dir: &Path, path: &Path, violations: &mut Vec<String>) {
        let Ok(source) = fs::read_to_string(path) else {
            return;
        };
        let scan_source = mask_comments_and_string_literals(&source);

        for (path_index, _) in scan_source.match_indices("crate::scripting::") {
            if let Some(barrel) = direct_path_barrel(&scan_source[path_index..]) {
                violations.push(format!(
                    "{}:{} references crate::scripting::{barrel}",
                    display_relative_path(src_dir, path).display(),
                    line_number_at(&source, path_index)
                ));
            }
        }

        for use_index in use_keyword_indices(&scan_source) {
            let Some(use_end) = use_item_end(&scan_source[use_index..]) else {
                continue;
            };
            let use_item = &scan_source[use_index..use_index + use_end];

            if let Some(group) = grouped_use_tree(use_item) {
                for barrel in REMOVED_OR_COLLAPSED_BARRELS {
                    if grouped_use_tree_contains_barrel(group, barrel) {
                        violations.push(format!(
                            "{}:{} imports crate::scripting::{{...{barrel}...}}",
                            display_relative_path(src_dir, path).display(),
                            line_number_at(&source, use_index)
                        ));
                    }
                }
            }

            if let Some(group) = crate_grouped_use_tree(use_item) {
                for barrel in REMOVED_OR_COLLAPSED_BARRELS {
                    if crate_grouped_use_tree_contains_scripting_barrel(group, barrel) {
                        violations.push(format!(
                            "{}:{} imports crate::{{...scripting::{barrel}...}}",
                            display_relative_path(src_dir, path).display(),
                            line_number_at(&source, use_index)
                        ));
                    }
                }
            }
        }
    }

    fn use_keyword_indices(source: &str) -> impl Iterator<Item = usize> + '_ {
        source.match_indices("use").filter_map(|(index, _)| {
            let before = source[..index].chars().next_back();
            let after = source[index + "use".len()..].chars().next();
            (!before.is_some_and(is_identifier_continue)
                && !after.is_some_and(is_identifier_continue))
            .then_some(index)
        })
    }

    fn use_item_end(source_from_use: &str) -> Option<usize> {
        let mut depth = 0usize;

        for (index, byte) in source_from_use.bytes().enumerate() {
            match byte {
                b'{' => depth += 1,
                b'}' => depth = depth.saturating_sub(1),
                b';' if depth == 0 => return Some(index + 1),
                _ => {}
            }
        }

        None
    }

    fn direct_path_barrel(path: &str) -> Option<&'static str> {
        let rest = path.strip_prefix("crate::scripting::")?;
        let (segment, _) = split_first_path_segment(rest);
        REMOVED_OR_COLLAPSED_BARRELS
            .iter()
            .copied()
            .find(|barrel| *barrel == segment)
    }

    fn grouped_use_tree(use_item: &str) -> Option<&str> {
        let path = use_path_after_keyword(use_item)?;
        let rest = path.strip_prefix("crate::scripting::")?;
        let rest = rest.trim_start();
        let rest = rest.strip_prefix('{')?;
        let group_end = grouped_use_tree_end(rest)?;
        Some(&rest[..group_end])
    }

    fn crate_grouped_use_tree(use_item: &str) -> Option<&str> {
        let path = use_path_after_keyword(use_item)?;
        let rest = path.strip_prefix("crate::")?;
        let rest = rest.trim_start();
        let rest = rest.strip_prefix('{')?;
        let group_end = grouped_use_tree_end(rest)?;
        Some(&rest[..group_end])
    }

    fn use_path_after_keyword(use_item: &str) -> Option<&str> {
        Some(use_item.strip_prefix("use")?.trim_start())
    }

    fn split_first_path_segment(path: &str) -> (&str, &str) {
        let end = path
            .char_indices()
            .find_map(|(index, ch)| (!is_identifier_continue(ch)).then_some(index))
            .unwrap_or(path.len());

        (&path[..end], &path[end..])
    }

    fn grouped_use_tree_end(source_after_open_brace: &str) -> Option<usize> {
        let mut depth = 0usize;

        for (index, byte) in source_after_open_brace.bytes().enumerate() {
            match byte {
                b'{' => depth += 1,
                b'}' if depth == 0 => return Some(index),
                b'}' => depth -= 1,
                _ => {}
            }
        }

        None
    }

    fn grouped_use_tree_contains_barrel(group: &str, barrel: &str) -> bool {
        let mut depth = 0usize;
        let mut item_start = 0usize;

        for (index, byte) in group.bytes().enumerate() {
            match byte {
                b'{' => depth += 1,
                b'}' => depth = depth.saturating_sub(1),
                b',' if depth == 0 => {
                    if grouped_use_item_starts_with_barrel(&group[item_start..index], barrel) {
                        return true;
                    }
                    item_start = index + 1;
                }
                _ => {}
            }
        }

        grouped_use_item_starts_with_barrel(&group[item_start..], barrel)
    }

    fn crate_grouped_use_tree_contains_scripting_barrel(group: &str, barrel: &str) -> bool {
        let mut depth = 0usize;
        let mut item_start = 0usize;

        for (index, byte) in group.bytes().enumerate() {
            match byte {
                b'{' => depth += 1,
                b'}' => depth = depth.saturating_sub(1),
                b',' if depth == 0 => {
                    if crate_grouped_use_item_contains_scripting_barrel(
                        &group[item_start..index],
                        barrel,
                    ) {
                        return true;
                    }
                    item_start = index + 1;
                }
                _ => {}
            }
        }

        crate_grouped_use_item_contains_scripting_barrel(&group[item_start..], barrel)
    }

    fn crate_grouped_use_item_contains_scripting_barrel(item: &str, barrel: &str) -> bool {
        let trimmed = item.trim_start();
        let Some(rest) = trimmed.strip_prefix("scripting") else {
            return false;
        };
        if rest.chars().next().is_some_and(is_identifier_continue) {
            return false;
        }

        let Some(rest) = rest.trim_start().strip_prefix("::") else {
            return false;
        };
        let rest = rest.trim_start();
        if let Some(rest) = rest.strip_prefix('{') {
            let Some(group_end) = grouped_use_tree_end(rest) else {
                return false;
            };
            return grouped_use_tree_contains_barrel(&rest[..group_end], barrel);
        }

        grouped_use_item_starts_with_barrel(rest, barrel)
    }

    fn grouped_use_item_starts_with_barrel(item: &str, barrel: &str) -> bool {
        let trimmed = item.trim_start();
        let Some(rest) = trimmed.strip_prefix(barrel) else {
            return false;
        };

        !rest.chars().next().is_some_and(is_identifier_continue)
            && (rest.is_empty()
                || rest.starts_with("::")
                || rest.chars().next().is_some_and(char::is_whitespace)
                || rest.starts_with(','))
    }

    fn line_number_at(source: &str, byte_index: usize) -> usize {
        source[..byte_index]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count()
            + 1
    }

    fn is_rust_source(path: &Path) -> bool {
        path.extension().is_some_and(|extension| extension == "rs")
    }

    fn is_allowlisted(src_dir: &Path, path: &Path) -> bool {
        let Ok(relative) = path.strip_prefix(src_dir) else {
            return false;
        };

        // This source scan catches direct `crate::scripting::<name>` paths and
        // ordinary grouped crate use trees. It still does not parse aliases to
        // `scripting` or `super::` relative forms. After barrel deletion those
        // forms are compile errors, so this is a reintroduction lock rather
        // than a full parser.
        relative == Path::new("scripting/mod.rs")
            || relative.starts_with(Path::new("scripting/typedef"))
    }

    fn display_relative_path(src_dir: &Path, path: &Path) -> PathBuf {
        path.strip_prefix(src_dir)
            .map(PathBuf::from)
            .unwrap_or_else(|_| path.to_path_buf())
    }
}
