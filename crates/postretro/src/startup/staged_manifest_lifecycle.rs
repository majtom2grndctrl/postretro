// Main-thread staged mod-manifest commit lifecycle. The scripting runtime owns
// build/validation; this app-side seam applies successful snapshots to the
// session, active level, and UI in their required commit order.
// See: context/lib/boot_sequence.md §1 · context/lib/scripting.md

use postretro_scripting_core::runtime::{ModRenderProfile, StagedManifestCommitOutcome};
use postretro_scripting_core::staged_manifest::{
    StagedManifestBuildResult, StagedManifestBuildStatus,
};

use crate::App;

/// Renderer bloom profile a staged manifest result commits, if any.
///
/// `None` means "leave the active profile alone" — every non-`Committed`
/// outcome (stale, failed build, rejected, release no-op) plus a committed
/// build that failed. A committed `NoStartScript` restores the default profile,
/// mirroring how the rest of the staged snapshot is replaced wholesale.
pub(crate) fn staged_render_profile(
    result: &StagedManifestBuildResult,
    outcome: &StagedManifestCommitOutcome,
) -> Option<ModRenderProfile> {
    if !matches!(outcome, StagedManifestCommitOutcome::Committed { .. }) {
        return None;
    }

    match &result.status {
        StagedManifestBuildStatus::Built(manifest) => Some(manifest.render),
        StagedManifestBuildStatus::NoStartScript => Some(ModRenderProfile::default()),
        StagedManifestBuildStatus::Failed => None,
    }
}

impl App {
    pub(crate) fn poll_staged_manifest_results(&mut self) {
        let staged = match self.session.as_mut() {
            Some(session) => session
                .scripting
                .script_runtime
                .poll_staged_manifest_builds(),
            None => return,
        };
        for result in staged {
            // `commit_staged_manifest_result` and the active-set recompose touch
            // the session-owned runtime/ctx/registry; the rebuild + UI commit are
            // App methods. Scope the session borrow to the commit call so the App
            // methods below can re-borrow `self`.
            let outcome = {
                let session = self.session.as_mut().expect("frontend session installed");
                session
                    .scripting
                    .script_runtime
                    .commit_staged_manifest_result(
                        &result,
                        &session.scripting.script_ctx,
                        &session.scripting.sequence_registry,
                    )
            };
            let committed = matches!(outcome, StagedManifestCommitOutcome::Committed { .. });
            if committed {
                let events = match &result.status {
                    StagedManifestBuildStatus::Built(manifest) => manifest.events.clone(),
                    StagedManifestBuildStatus::NoStartScript => Vec::new(),
                    StagedManifestBuildStatus::Failed => Vec::new(),
                };
                if let Some(session) = self.session.as_mut() {
                    session
                        .scripting
                        .impact_policy_runtime
                        .replace_global_events(events);
                }
            }
            if committed && self.has_installed_level() {
                if let Some(session) = self.session.as_ref() {
                    session
                        .scripting
                        .script_ctx
                        .data_registry
                        .borrow_mut()
                        .recompose_active_sets(&self.active_level_tags);
                }
                self.rebuild_active_reaction_subscribers();
                self.rebuild_active_system_reaction_bindings();
                self.rebuild_active_trigger_bindings();
            }
            // Ahead of the UI commit so the first frame that presents the
            // reloaded UI already renders through the reloaded bloom profile.
            if let Some(render_profile) = staged_render_profile(&result, &outcome) {
                self.apply_mod_bloom_render_profile(render_profile);
            }
            self.commit_staged_ui_manifest(&result, &outcome);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_scripting_core::runtime::{ModBloomProfile, ModBloomResolution};
    use postretro_scripting_core::staged_manifest::StagedManifest;
    use std::path::PathBuf;

    const GENERATION: u64 = 12;

    fn quarter_pixelated() -> ModRenderProfile {
        ModRenderProfile {
            bloom: ModBloomProfile {
                resolution: ModBloomResolution::Quarter,
                pixelated: true,
            },
        }
    }

    fn built_result(render: ModRenderProfile) -> StagedManifestBuildResult {
        StagedManifestBuildResult {
            generation: GENERATION,
            mod_root: PathBuf::from("content/dev"),
            status: StagedManifestBuildStatus::Built(Box::new(StagedManifest {
                name: "RenderProfile".to_string(),
                id: "render-profile".to_string(),
                version: "1".to_string(),
                render,
                entities: Vec::new(),
                maps: Vec::new(),
                reactions: Vec::new(),
                crossings: Vec::new(),
                events: Vec::new(),
                trigger_events: Vec::new(),
                trigger_pools: Vec::new(),
                ui_trees: Vec::new(),
                theme: Default::default(),
                frontend: None,
                store_declarations: Default::default(),
                dependency_paths: Vec::new(),
            })),
            diagnostics: Vec::new(),
        }
    }

    fn status_result(status: StagedManifestBuildStatus) -> StagedManifestBuildResult {
        StagedManifestBuildResult {
            generation: GENERATION,
            mod_root: PathBuf::from("content/dev"),
            status,
            diagnostics: Vec::new(),
        }
    }

    fn committed() -> StagedManifestCommitOutcome {
        StagedManifestCommitOutcome::Committed {
            generation: GENERATION,
            descriptor_count: 0,
            applied_actions: 0,
            dropped_missing_targets: 0,
        }
    }

    fn non_committed_outcomes() -> Vec<StagedManifestCommitOutcome> {
        vec![
            StagedManifestCommitOutcome::DiscardedStale {
                generation: GENERATION - 1,
                latest_requested: Some(GENERATION),
            },
            StagedManifestCommitOutcome::FailedBuild {
                generation: GENERATION,
            },
            StagedManifestCommitOutcome::Rejected {
                generation: GENERATION,
                reason: "incompatible store schema".to_string(),
            },
            StagedManifestCommitOutcome::ReleaseNoop,
        ]
    }

    #[test]
    fn committed_built_reload_commits_the_manifest_render_profile() {
        assert_eq!(
            staged_render_profile(&built_result(quarter_pixelated()), &committed()),
            Some(quarter_pixelated()),
        );
    }

    #[test]
    fn committed_no_start_script_reload_restores_the_default_profile() {
        assert_eq!(
            staged_render_profile(
                &status_result(StagedManifestBuildStatus::NoStartScript),
                &committed(),
            ),
            Some(ModRenderProfile::default()),
        );
    }

    #[test]
    fn committed_failed_build_leaves_the_active_profile_untouched() {
        assert_eq!(
            staged_render_profile(
                &status_result(StagedManifestBuildStatus::Failed),
                &committed()
            ),
            None,
        );
    }

    #[test]
    fn non_committed_outcomes_leave_the_active_profile_untouched() {
        // Spec invariant: a profile changes only after a successful commit, even
        // when the discarded result carries a fully built manifest.
        let result = built_result(quarter_pixelated());
        for outcome in non_committed_outcomes() {
            assert_eq!(
                staged_render_profile(&result, &outcome),
                None,
                "{outcome:?} must not move the active bloom profile",
            );
        }
    }
}
