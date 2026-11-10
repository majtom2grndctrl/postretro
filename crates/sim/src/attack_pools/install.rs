//! Install-time check of each attack pool's `budget_profile` against the mod's profiles.
//! See: context/lib/entity_model.md §7c (Attack budget, Attack pools)

use postretro_entities::{ComponentKind, ComponentValue, EntityRegistry};
use postretro_foundation::attack_budget::DEFAULT_PROFILE_NAME;

/// Warn once per attack pool whose `budget_profile` the mod's `attackBudgets`
/// does not declare. Such a pool runs under `default`. Runs at level install,
/// after every map placement materializes.
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
    // profile says nothing.
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
}
