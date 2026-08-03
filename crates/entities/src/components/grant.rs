// Additive resource grants for existing entity capabilities.
// See: context/lib/entity_model.md §2 (Health and AmmoReserve components)

use crate::components::ammo_reserve::AmmoReserve;
use crate::components::health::{HealthComponent, set_health_absolute};
use crate::registry::{EntityId, EntityRegistry};

/// The observable result of a resource grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantOutcome {
    Applied,
    SkippedNoComponent,
    SkippedInvalidAmount,
}

/// Add health to a recipient that already carries [`HealthComponent`].
///
/// The absolute-health chokepoint owns clamping and the positive-health
/// re-arm/attribution reset semantics, so this function deliberately performs
/// neither operation itself.
pub fn grant_health(
    registry: &mut EntityRegistry,
    recipient: EntityId,
    amount: f32,
) -> GrantOutcome {
    if !amount.is_finite() || amount < 0.0 {
        log::warn!("[Grant] grantHealth: amount {amount} is negative or non-finite; no-op");
        return GrantOutcome::SkippedInvalidAmount;
    }

    let Ok(health) = registry.get_component::<HealthComponent>(recipient) else {
        log::warn!("[Grant] grantHealth: entity {recipient} has no HealthComponent; skipping");
        return GrantOutcome::SkippedNoComponent;
    };

    // A zero grant is observable as successful but must not reset live damage
    // attribution through the absolute-health write path.
    if amount == 0.0 {
        return GrantOutcome::Applied;
    }

    let current = health.current;
    set_health_absolute(registry, recipient, current + amount);
    GrantOutcome::Applied
}

/// Add ammunition to a recipient that already carries an [`AmmoReserve`].
///
/// Amounts truncate toward zero, then [`AmmoReserve::credit`] handles
/// saturation. This is the sole post-spawn write path into a reserve.
pub fn grant_ammo(
    registry: &mut EntityRegistry,
    recipient: EntityId,
    ammo_type: &str,
    amount: f32,
) -> GrantOutcome {
    if !amount.is_finite() || amount < 0.0 {
        log::warn!("[Grant] grantAmmo: amount {amount} is negative or non-finite; no-op");
        return GrantOutcome::SkippedInvalidAmount;
    }

    let Ok(reserve) = registry.get_component::<AmmoReserve>(recipient) else {
        log::warn!("[Grant] grantAmmo: entity {recipient} has no AmmoReserve; skipping");
        return GrantOutcome::SkippedNoComponent;
    };

    let credited = amount.trunc() as u32;
    if credited == 0 {
        return GrantOutcome::Applied;
    }

    let mut updated = reserve.clone();
    updated.credit(ammo_type, credited);
    // `get_component` above established that this is a live recipient with an
    // existing reserve; a later stale-id error cannot arise within this call.
    let _ = registry.set_component(recipient, updated);
    GrantOutcome::Applied
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use postretro_foundation::data_descriptors::HealthDescriptor;

    use super::*;
    use crate::components::health::{ContributorLedgerRecord, PendingKillCredit};
    use crate::registry::Transform;

    fn health_component(max: f32) -> HealthComponent {
        HealthComponent::from_descriptor(&HealthDescriptor {
            max,
            hitbox: None,
            zone_multipliers: Default::default(),
        })
    }

    #[test]
    fn grant_health_clamps_at_component_maximum() {
        let mut registry = EntityRegistry::new();
        let recipient = registry.spawn(Transform::default());
        let mut health = health_component(80.0);
        health.current = 70.0;
        registry.set_component(recipient, health).unwrap();

        assert_eq!(
            grant_health(&mut registry, recipient, 20.0),
            GrantOutcome::Applied
        );
        assert_eq!(
            registry
                .get_component::<HealthComponent>(recipient)
                .unwrap()
                .current,
            80.0
        );
    }

    #[test]
    fn grant_health_uses_absolute_write_rearm_and_credit_reset() {
        let mut registry = EntityRegistry::new();
        let recipient = registry.spawn(Transform::default());
        let mut health = health_component(80.0);
        health.current = 20.0;
        health.death_handled = true;
        health.record_contributor_damage(ContributorLedgerRecord::new("weapon.old", 12.0));
        health.pending_kill_credit = Some(PendingKillCredit {
            tags: vec!["downed".to_string()],
            contributor_ledger: health.contributor_ledger.clone(),
        });
        registry.set_component(recipient, health).unwrap();

        assert_eq!(
            grant_health(&mut registry, recipient, 5.0),
            GrantOutcome::Applied
        );

        let health = registry
            .get_component::<HealthComponent>(recipient)
            .unwrap();
        assert_eq!(health.current, 25.0);
        assert!(!health.death_handled);
        assert!(health.pending_kill_credit.is_none());
        assert!(health.contributor_ledger.entries().is_empty());
        assert!(health.contributor_ledger.overflow().is_none());
    }

    #[test]
    fn grant_ammo_truncates_and_saturates() {
        let mut registry = EntityRegistry::new();
        let recipient = registry.spawn(Transform::default());
        registry
            .set_component(recipient, AmmoReserve::new())
            .unwrap();

        assert_eq!(
            grant_ammo(&mut registry, recipient, "cells", 3.9),
            GrantOutcome::Applied
        );
        assert_eq!(
            registry
                .get_component::<AmmoReserve>(recipient)
                .unwrap()
                .available("cells"),
            3
        );

        assert_eq!(
            grant_ammo(&mut registry, recipient, "cells", u32::MAX as f32),
            GrantOutcome::Applied
        );
        assert_eq!(
            registry
                .get_component::<AmmoReserve>(recipient)
                .unwrap()
                .available("cells"),
            u32::MAX
        );
    }

    #[test]
    fn invalid_grant_amounts_do_not_mutate_existing_components() {
        let mut registry = EntityRegistry::new();
        let recipient = registry.spawn(Transform::default());
        let mut health = health_component(80.0);
        health.current = 20.0;
        registry.set_component(recipient, health).unwrap();
        registry
            .set_component(recipient, AmmoReserve::new())
            .unwrap();

        for amount in [-1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(
                grant_health(&mut registry, recipient, amount),
                GrantOutcome::SkippedInvalidAmount
            );
            assert_eq!(
                grant_ammo(&mut registry, recipient, "cells", amount),
                GrantOutcome::SkippedInvalidAmount
            );
        }

        assert_eq!(
            registry
                .get_component::<HealthComponent>(recipient)
                .unwrap()
                .current,
            20.0
        );
        assert_eq!(
            registry
                .get_component::<AmmoReserve>(recipient)
                .unwrap()
                .available("cells"),
            0
        );
    }

    #[test]
    fn zero_grants_are_successful_no_ops() {
        let mut registry = EntityRegistry::new();
        let recipient = registry.spawn(Transform::default());
        let mut health = health_component(80.0);
        health.current = 20.0;
        health.record_contributor_damage(ContributorLedgerRecord::new("weapon.old", 12.0));
        let expected_health = health.clone();
        let expected_reserve = AmmoReserve::new();
        registry.set_component(recipient, health).unwrap();
        registry
            .set_component(recipient, expected_reserve.clone())
            .unwrap();

        assert_eq!(
            grant_health(&mut registry, recipient, 0.0),
            GrantOutcome::Applied
        );
        assert_eq!(
            grant_ammo(&mut registry, recipient, "cells", 0.0),
            GrantOutcome::Applied
        );
        assert_eq!(
            registry
                .get_component::<HealthComponent>(recipient)
                .unwrap(),
            &expected_health
        );
        assert_eq!(
            registry.get_component::<AmmoReserve>(recipient).unwrap(),
            &expected_reserve
        );
    }

    #[test]
    fn grants_skip_recipients_without_the_required_component() {
        let mut registry = EntityRegistry::new();
        let recipient = registry.spawn(Transform::default());

        assert_eq!(
            grant_health(&mut registry, recipient, 1.0),
            GrantOutcome::SkippedNoComponent
        );
        assert_eq!(
            grant_ammo(&mut registry, recipient, "cells", 1.0),
            GrantOutcome::SkippedNoComponent
        );
    }

    #[test]
    fn ammo_reserve_writes_have_only_grant_seed_and_carry_restore_non_test_call_sites() {
        let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let crates_dir = workspace_root.join("crates");
        let mut credit_call_sites = Vec::new();
        collect_method_call_sites(&crates_dir, &crates_dir, "credit", &mut credit_call_sites);
        credit_call_sites.sort();

        assert_eq!(
            credit_call_sites,
            vec![
                PathBuf::from("entities/src/components/grant.rs"),
                PathBuf::from("postretro/src/scripting/builtins/wieldable_inventory.rs"),
            ],
            "post-spawn reserve writes must route through grant_ammo; descriptor spawn seeding is the sole exception"
        );

        let mut exact_write_call_sites = Vec::new();
        collect_method_call_sites(
            &crates_dir,
            &crates_dir,
            "set_exact",
            &mut exact_write_call_sites,
        );
        exact_write_call_sites.sort();

        assert_eq!(
            exact_write_call_sites,
            vec![PathBuf::from(
                "postretro/src/scripting/builtins/wieldable_inventory.rs"
            )],
            "exact reserve writes are reserved for carried-state restoration"
        );
    }

    fn collect_method_call_sites(
        root: &Path,
        path: &Path,
        method: &str,
        call_sites: &mut Vec<PathBuf>,
    ) {
        let Ok(entries) = fs::read_dir(path) else {
            return;
        };

        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                collect_method_call_sites(root, &path, method, call_sites);
            } else if path.extension().is_some_and(|extension| extension == "rs")
                && !is_test_source_file(root, &path)
            {
                let Ok(source) = fs::read_to_string(&path) else {
                    continue;
                };
                let production_source = mask_test_only_blocks(&source);
                let relative_path = path.strip_prefix(root).map(PathBuf::from).unwrap_or(path);
                if method_call_count(&production_source, method) > 0 {
                    call_sites.push(relative_path.clone());
                }
            }
        }
    }

    fn is_test_source_file(root: &Path, path: &Path) -> bool {
        let Ok(relative_path) = path.strip_prefix(root) else {
            return false;
        };
        let is_in_integration_tests = relative_path
            .components()
            .any(|component| component.as_os_str() == "tests");
        let is_test_harness = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|stem| stem.ends_with("_test") || stem.ends_with("_test_fixtures"));

        is_in_integration_tests || is_test_harness
    }

    fn method_call_count(source: &str, method: &str) -> usize {
        let needle = format!(".{method}");
        source
            .match_indices(&needle)
            .filter(|(offset, _)| {
                source[*offset + needle.len()..]
                    .trim_start()
                    .starts_with('(')
            })
            .count()
    }

    fn mask_test_only_blocks(source: &str) -> String {
        let mut masked = mask_comments_and_string_literals(source);
        let mut search_start = 0;

        while let Some(attribute_offset) = masked[search_start..].find("#[cfg(test)]") {
            let attribute_start = search_start + attribute_offset;
            let Some(body_start) = masked[attribute_start..].find('{') else {
                break;
            };
            let body_start = attribute_start + body_start;
            let Some(body_end) = matching_brace_end(&masked, body_start) else {
                break;
            };
            masked.replace_range(
                attribute_start..body_end,
                &" ".repeat(body_end - attribute_start),
            );
            search_start = body_end;
        }

        masked
    }

    fn matching_brace_end(source: &str, opening_brace: usize) -> Option<usize> {
        let mut depth = 0usize;
        for (offset, byte) in source[opening_brace..].bytes().enumerate() {
            match byte {
                b'{' => depth += 1,
                b'}' => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        return Some(opening_brace + offset + 1);
                    }
                }
                _ => {}
            }
        }
        None
    }

    // Mirrors the source-scan masking precedent in
    // crates/postretro/src/scripting/mod.rs. The caller-count gate must ignore
    // examples and unit fixtures as well as comments.
    fn mask_comments_and_string_literals(source: &str) -> String {
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
        rest.starts_with('"') && rest[1..].starts_with(&"#".repeat(hash_count))
    }
}
