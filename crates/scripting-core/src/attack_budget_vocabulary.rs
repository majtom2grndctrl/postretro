// Committing a mod's attack tokens and budget profiles: the registry copy the
// AI's attack budget reads and the data-registry copy every other consumer reads.
// See: context/lib/scripting.md §2 (Attack token declarations)

use std::sync::Arc;

use postretro_entities::{ComponentKind, ComponentValue, EntityRegistry};
use postretro_foundation::attack_budget::{AttackBudgetVocabulary, DEFAULT_PROFILE_NAME};

use crate::ctx::ScriptCtx;

/// Install `vocabulary` as the committed one, replacing both copies whole.
/// The AI re-resolves draws and budgets when it sees the new pointer, so a
/// hot reload applies from the next tick. A placed attack pool whose profile
/// the new vocabulary drops warns here; at first boot no pool is placed yet,
/// and level install warns once its pools exist.
pub fn commit_attack_budgets(ctx: &ScriptCtx, vocabulary: AttackBudgetVocabulary) {
    let vocabulary = Arc::new(vocabulary);
    {
        let mut registry = ctx.registry.borrow_mut();
        registry.set_attack_budgets(Arc::clone(&vocabulary));
        warn_undeclared_budget_profiles(&registry);
    }
    ctx.data_registry.borrow_mut().attack_budgets = vocabulary;
}

/// Warn once per attack pool whose `budget_profile` the committed
/// `attackBudgets` does not declare. Such a pool runs under `default`. Runs at
/// level install, after every map placement materializes, and at every
/// commit, so a hot reload that removes a profile a placed pool names says so.
pub fn warn_undeclared_budget_profiles(registry: &EntityRegistry) {
    let vocabulary = registry.attack_budgets();
    for (id, value) in registry.iter_with_kind(ComponentKind::AttackPool) {
        let ComponentValue::AttackPool(pool) = value else {
            continue;
        };
        let profile = pool.budget_profile.as_str();
        if profile.is_empty()
            || profile == DEFAULT_PROFILE_NAME
            || vocabulary.profiles().contains_key(profile)
        {
            continue;
        }
        let tags = registry
            .get_tags(id)
            .map(|tags| tags.join(" "))
            .unwrap_or_default();
        log::warn!(
            "[AttackPool] ai_attack_pool {id} (`_tags` `{tags}`): budget_profile `{profile}` is not declared in the mod's `attackBudgets`; using `default`"
        );
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use postretro_entities::Transform;
    use postretro_entities::components::attack_pool::AttackPoolComponent;
    use postretro_foundation::attack_budget::AttackBudgetVocabulary;
    use serde_json::json;

    use super::*;

    // A1: a `budget_profile` the mod does not declare warns once at level
    // load and the pool runs as `default`; a declared, empty or `default`
    // profile says nothing. A reload commit runs the same check.
    #[test]
    fn undeclared_budget_profile_warns_at_load() {
        let mut registry = EntityRegistry::new();
        registry.set_attack_budgets(Arc::new(
            AttackBudgetVocabulary::from_manifest(
                Some(
                    &json!([{ "kind": "attackToken", "name": "melee", "budget": { "limit": 2 } }]),
                ),
                Some(&json!({ "arena": [
                    { "token": { "kind": "attackToken", "name": "melee" }, "limit": 1 }
                ] })),
            )
            .expect("a valid registration"),
        ));
        for (tag, profile) in [
            ("east", "arena"),
            ("west", ""),
            ("north", "default"),
            ("south", "frenzy"),
        ] {
            let pool = registry
                .try_spawn(Transform::default(), &[tag.to_string()])
                .unwrap();
            registry
                .set_component(
                    pool,
                    AttackPoolComponent::new("guard".to_string(), profile.to_string()),
                )
                .unwrap();
        }
        let capture = postretro_test_log_capture::LogCapture::start();
        warn_undeclared_budget_profiles(&registry);
        capture.assert_logged_once(
            log::Level::Warn,
            "`_tags` `south`): budget_profile `frenzy` is not declared in the mod's `attackBudgets`; using `default`",
        );
        capture.assert_logged_once(log::Level::Warn, "budget_profile");
    }

    fn vocabulary_with_profiles(profiles: serde_json::Value) -> AttackBudgetVocabulary {
        AttackBudgetVocabulary::from_manifest(
            Some(&json!([{ "kind": "attackToken", "name": "melee", "budget": { "limit": 2 } }])),
            Some(&profiles),
        )
        .expect("a valid registration")
    }

    // A hot reload that drops a profile a placed pool names warns at the
    // commit, as level install would; the commit that still declares it says
    // nothing.
    #[test]
    fn reload_commit_dropping_a_placed_pool_profile_warns() {
        let ctx = ScriptCtx::new();
        let arena = json!({ "arena": [
            { "token": { "kind": "attackToken", "name": "melee" }, "limit": 1 }
        ] });
        commit_attack_budgets(&ctx, vocabulary_with_profiles(arena.clone()));
        {
            let mut registry = ctx.registry.borrow_mut();
            let pool = registry
                .try_spawn(Transform::default(), &["east".to_string()])
                .unwrap();
            registry
                .set_component(
                    pool,
                    AttackPoolComponent::new("guard".to_string(), "arena".to_string()),
                )
                .unwrap();
        }
        let capture = postretro_test_log_capture::LogCapture::start();
        commit_attack_budgets(&ctx, vocabulary_with_profiles(arena));
        capture.assert_not_logged(log::Level::Warn, "budget_profile");

        commit_attack_budgets(&ctx, vocabulary_with_profiles(json!({})));
        capture.assert_logged_once(
            log::Level::Warn,
            "`_tags` `east`): budget_profile `arena` is not declared in the mod's `attackBudgets`; using `default`",
        );
    }
}
