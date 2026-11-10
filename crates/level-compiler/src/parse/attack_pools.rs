//! `ai_attack_pool` authoring checks: a member tag is required, and no two pools share a tag.
//! See: context/lib/build_pipeline.md §Custom FGD · context/lib/entity_model.md §7c (Attack pools)

use std::collections::HashMap;

use anyhow::{Result, bail};

pub(super) use crate::script_light_membership::ATTACK_POOL_CLASSNAME;

/// The pools seen so far, by each `_tags` tag they carry. Triggers and scripts
/// reach a pool by tag, so a tag two pools share would address both.
#[derive(Default)]
pub(super) struct AttackPoolChecker {
    by_tag: HashMap<String, String>,
}

impl AttackPoolChecker {
    /// Check one pool, `entity` labelling it as `entity N 'ai_attack_pool'`.
    /// An empty `member_tag` would assign no one on start; it is an error.
    pub(super) fn record(
        &mut self,
        entity: usize,
        props: &HashMap<String, String>,
        tags: &[String],
    ) -> Result<()> {
        let label = format!("entity {entity} '{ATTACK_POOL_CLASSNAME}'");
        let member_tag = props.get("member_tag").map_or("", |value| value.trim());
        if member_tag.is_empty() {
            bail!(
                "{label} has an empty `member_tag`; name the tag its members carry (`_tags`, or a spawner's `spawned_tags`)"
            );
        }
        for tag in tags {
            if let Some(earlier) = self.by_tag.get(tag) {
                bail!(
                    "{earlier} and {label} share `_tags` `{tag}`; a trigger or script reaching `{tag}` would address both pools, so give each its own tag"
                );
            }
        }
        for tag in tags {
            self.by_tag.insert(tag.clone(), label.clone());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(label: &str, entities: &str) -> Result<crate::map_data::MapData> {
        let world = "// entity 0\n{\n\"classname\" \"worldspawn\"\n{\n\
             ( -512 0 0 ) ( -512 1 0 ) ( -512 0 1 ) tex 0 0 0 1 1\n\
             ( -448 0 0 ) ( -448 0 1 ) ( -448 1 0 ) tex 0 0 0 1 1\n\
             ( 0 -512 0 ) ( 0 -512 1 ) ( 1 -512 0 ) tex 0 0 0 1 1\n\
             ( 0 -448 0 ) ( 1 -448 0 ) ( 0 -448 1 ) tex 0 0 0 1 1\n\
             ( 0 0 -512 ) ( 1 0 -512 ) ( 0 1 -512 ) tex 0 0 0 1 1\n\
             ( 0 0 -448 ) ( 0 1 -448 ) ( 1 0 -448 ) tex 0 0 0 1 1\n\
             }\n}\n";
        let path = std::env::temp_dir().join(format!(
            "postretro-attack-pools-{label}-{}.map",
            std::process::id()
        ));
        std::fs::write(&path, format!("{world}{entities}")).expect("write fixture map");
        let parsed = super::super::parse_map_file(&path, crate::map_format::MapFormat::IdTech2);
        let _ = std::fs::remove_file(&path);
        parsed
    }

    fn pool(tags: &str, member_tag: &str) -> String {
        format!(
            "{{\n\"classname\" \"ai_attack_pool\"\n\"origin\" \"0 0 0\"\n\"_tags\" \"{tags}\"\n\"member_tag\" \"{member_tag}\"\n\"budget_profile\" \"arena\"\n}}\n"
        )
    }

    // A8: an empty `member_tag` is a compile error naming the entity.
    #[test]
    fn ai_attack_pool_empty_member_tag_is_compile_error() {
        let error = parse("empty-member-tag", &pool("arena", " "))
            .expect_err("an empty member tag fails the build")
            .to_string();
        assert!(
            error.contains("entity 1 'ai_attack_pool' has an empty `member_tag`"),
            "{error}"
        );
    }

    // A8: two pools sharing one `_tags` is a compile error naming both.
    #[test]
    fn ai_attack_pools_sharing_tags_is_compile_error() {
        let error = parse(
            "shared-tags",
            &format!("{}{}", pool("arena", "guard"), pool("east arena", "brute")),
        )
        .expect_err("a shared tag fails the build")
        .to_string();
        assert!(
            error.contains(
                "entity 1 'ai_attack_pool' and entity 2 'ai_attack_pool' share `_tags` `arena`"
            ),
            "{error}"
        );
    }

    // A5 (compile half): two pools sharing one `member_tag` compile cleanly,
    // and the latest start decides which one an enemy takes turns in.
    #[test]
    fn ai_attack_pools_sharing_member_tag_compile_cleanly() {
        let map = parse(
            "shared-member-tag",
            &format!("{}{}", pool("calm", "guard"), pool("frenzy", "guard")),
        )
        .expect("a shared member tag compiles");
        let pools = map
            .map_entities
            .iter()
            .filter(|entity| entity.classname == ATTACK_POOL_CLASSNAME)
            .count();
        assert_eq!(pools, 2, "both pools reach the map-entity record");
    }
}
