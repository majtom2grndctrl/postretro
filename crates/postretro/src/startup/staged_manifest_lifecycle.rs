// Main-thread staged mod-manifest commit lifecycle. The scripting runtime owns
// build/validation; this app-side seam applies successful snapshots to the
// session, active level, and UI in their required commit order.
// See: context/lib/boot_sequence.md §1 · context/lib/scripting.md

use postretro_foundation::SwitchingDescriptor;
use postretro_scripting_core::runtime::{ModRenderProfile, StagedManifestCommitOutcome};
use postretro_scripting_core::staged_manifest::{
    StagedManifestBuildResult, StagedManifestBuildStatus,
};

use crate::App;

fn clear_replaced_presentation_overlay_state(
    pool: &mut crate::presentation_pool::PresentationPool,
    client_facts: &mut crate::netcode::ClientOverlayFactState,
    host_facts: &mut crate::netcode::HostOverlayFactTracker,
) {
    pool.clear_overlays();
    client_facts.clear();
    host_facts.clear();
}

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

/// Switching policy a staged manifest result commits, if any. Like the render
/// profile, it is a whole-snapshot App policy: a committed no-start result
/// restores defaults, while every rejected or failed result retains the active
/// setting.
pub(crate) fn staged_switching(
    result: &StagedManifestBuildResult,
    outcome: &StagedManifestCommitOutcome,
) -> Option<SwitchingDescriptor> {
    if !matches!(outcome, StagedManifestCommitOutcome::Committed { .. }) {
        return None;
    }

    match &result.status {
        StagedManifestBuildStatus::Built(manifest) => Some(manifest.switching),
        StagedManifestBuildStatus::NoStartScript => Some(SwitchingDescriptor::default()),
        StagedManifestBuildStatus::Failed => None,
    }
}

/// Kinematic-mover default committed with a staged manifest snapshot. Static
/// mover components read this only when their next level is installed.
pub(crate) fn staged_mover_auto_close_ms(
    result: &StagedManifestBuildResult,
    outcome: &StagedManifestCommitOutcome,
) -> Option<f32> {
    if !matches!(outcome, StagedManifestCommitOutcome::Committed { .. }) {
        return None;
    }
    match &result.status {
        StagedManifestBuildStatus::Built(manifest) => Some(manifest.movers.auto_close_ms),
        StagedManifestBuildStatus::NoStartScript => {
            Some(crate::runtime_movers::ENGINE_AUTO_CLOSE_MS)
        }
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
            let committed = match &outcome {
                StagedManifestCommitOutcome::Committed { .. } => true,
                StagedManifestCommitOutcome::DiscardedStale { .. }
                | StagedManifestCommitOutcome::FailedBuild { .. }
                | StagedManifestCommitOutcome::Rejected { .. }
                | StagedManifestCommitOutcome::ReleaseNoop => false,
            };
            if committed {
                let events = match &result.status {
                    StagedManifestBuildStatus::Built(manifest) => manifest.events.clone(),
                    StagedManifestBuildStatus::NoStartScript => Vec::new(),
                    StagedManifestBuildStatus::Failed => Vec::new(),
                };
                let presentation_templates = match &result.status {
                    StagedManifestBuildStatus::Built(manifest) => {
                        manifest.presentation_templates.clone()
                    }
                    StagedManifestBuildStatus::NoStartScript => Vec::new(),
                    StagedManifestBuildStatus::Failed => Vec::new(),
                };
                let presentation_overlays = match &result.status {
                    StagedManifestBuildStatus::Built(manifest) => {
                        manifest.presentation_overlays.clone()
                    }
                    StagedManifestBuildStatus::NoStartScript => Vec::new(),
                    StagedManifestBuildStatus::Failed => Vec::new(),
                };
                if let Some(session) = self.session.as_mut() {
                    let mod_id = session
                        .scripting
                        .script_runtime
                        .committed_mod_identity()
                        .map(|(id, _)| id.to_string());
                    session.scripting.impact_policy_runtime.set_mod_id(mod_id);
                    session
                        .scripting
                        .impact_policy_runtime
                        .replace_global_events(events);
                    session
                        .scripting
                        .impact_policy_runtime
                        .replace_presentation_templates(presentation_templates.clone());
                    session
                        .scripting
                        .impact_policy_runtime
                        .replace_presentation_overlays(presentation_overlays);
                    clear_replaced_presentation_overlay_state(
                        &mut session.presentation_pool,
                        &mut session.client_overlay_facts,
                        &mut session.host_overlay_fact_tracker,
                    );
                }
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.set_presentation_templates(presentation_templates);
                }
                if let Some(endpoint) = self
                    .session
                    .as_mut()
                    .and_then(|session| session.net_endpoint.as_mut())
                {
                    // Store removal is invisible to reconcile plans, so every
                    // committed manifest rebuilds the cache rather than trying to
                    // detect only additions. Host participants are re-registered
                    // against the fresh tracker so their next records are baselines.
                    endpoint.reset_state_slot_schema();
                }
            }
            if committed && self.has_installed_level() {
                // A parked tail is a snapshot of a body the author may have just
                // edited away, so a hot reload drops every pending instance (O40) —
                // with a `warn!` naming the count, matching the level-teardown
                // clear. Its own session borrow, scoped tight like the recompose
                // borrow below.
                if let Some(session) = self.session.as_ref() {
                    session.scripting.scheduler.clear();
                }
                if let Some(session) = self.session.as_ref() {
                    session
                        .scripting
                        .script_ctx
                        .data_registry
                        .borrow_mut()
                        .recompose_active_sets(&self.active_level_tags);
                }
                // E18 Pass A re-runs after the recompose (which rebuilds
                // `DataRegistry.reactions` from retained originals, erasing a
                // prior in-place drop) and BEFORE the trigger binder rebuild, so
                // a body Pass A rejects is not bound below (O40).
                if let Some(session) = self.session.as_ref() {
                    let script_ctx = session.scripting.script_ctx.clone();
                    crate::startup::reaction_validation::validate_reaction_bodies_pass_a(
                        &script_ctx,
                    );
                }
                self.rebuild_active_reaction_subscribers();
                self.rebuild_active_system_reaction_bindings();
                self.rebuild_active_trigger_bindings();
                // E18 Pass B re-runs after BOTH binder rebuilds: it reads the
                // freshly-rebuilt system-reaction bindings (V4b) and derives Exit
                // edges (V5) into the freshly-rebuilt `self.trigger_bindings`.
                // `mem::take` isolates the mutable `trigger_bindings` borrow from
                // the shared session borrow the pass also needs.
                let mut trigger_bindings = std::mem::take(&mut self.trigger_bindings);
                if let Some(session) = self.session.as_ref() {
                    let script_ctx = session.scripting.script_ctx.clone();
                    crate::startup::reaction_validation::validate_trigger_coupled_pass_b(
                        &script_ctx,
                        &mut trigger_bindings,
                        &session.scripting.system_reaction_ir_bindings,
                    );
                }
                self.trigger_bindings = trigger_bindings;
            }
            // Ahead of the UI commit so the first frame that presents the
            // reloaded UI already renders through the reloaded bloom profile.
            if let Some(render_profile) = staged_render_profile(&result, &outcome) {
                self.apply_mod_bloom_render_profile(render_profile);
            }
            if let Some(switching) = staged_switching(&result, &outcome) {
                self.switching = switching;
            }
            if let Some(mover_auto_close_ms) = staged_mover_auto_close_ms(&result, &outcome)
                && let Some(session) = self.session.as_mut()
            {
                session.scripting.mover_auto_close_ms = mover_auto_close_ms;
            }
            self.commit_staged_ui_manifest(&result, &outcome);
            if committed {
                self.install_network_mod_content();
            }
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
        built_result_with_switching(render, SwitchingDescriptor::default())
    }

    fn built_result_with_switching(
        render: ModRenderProfile,
        switching: SwitchingDescriptor,
    ) -> StagedManifestBuildResult {
        StagedManifestBuildResult {
            generation: GENERATION,
            mod_root: PathBuf::from("content/dev"),
            status: StagedManifestBuildStatus::Built(Box::new(StagedManifest {
                name: "RenderProfile".to_string(),
                id: "render-profile".to_string(),
                version: "1".to_string(),
                render,
                movers: Default::default(),
                switching,
                entities: Vec::new(),
                maps: Vec::new(),
                reactions: Vec::new(),
                crossings: Vec::new(),
                events: Vec::new(),
                trigger_events: Vec::new(),
                trigger_pools: Vec::new(),
                ui_trees: Vec::new(),
                presentation_templates: Vec::new(),
                presentation_overlays: Vec::new(),
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

    #[test]
    fn staged_switching_commits_only_successful_whole_snapshots() {
        let expected = SwitchingDescriptor {
            commit_on_direct_select: false,
            cycle_commit_dwell_ms: 125.0,
            block_during_reload: true,
        };
        let result = built_result_with_switching(quarter_pixelated(), expected);
        assert_eq!(staged_switching(&result, &committed()), Some(expected));
        assert_eq!(
            staged_switching(
                &status_result(StagedManifestBuildStatus::NoStartScript),
                &committed(),
            ),
            Some(SwitchingDescriptor::default()),
        );
        assert_eq!(
            staged_switching(
                &status_result(StagedManifestBuildStatus::Failed),
                &committed(),
            ),
            None,
        );
        for outcome in non_committed_outcomes() {
            assert_eq!(staged_switching(&result, &outcome), None);
        }
    }

    // Regression: replacing overlay templates cleared the pool but retained a
    // client terminal/pending fact stream that could target the new template.
    #[test]
    fn overlay_authoring_replacement_clears_client_fact_lifecycle() {
        let entity = postretro_entities::EntityId::from_raw(4);
        let mut pool = crate::presentation_pool::PresentationPool::new(1);
        pool.refresh_overlay(
            entity,
            postretro_entities::PresentationTemplateHandle::from("old-overlay"),
            1.0,
            1,
            u64::from(entity.to_raw()),
        );
        let mut client_facts = crate::netcode::ClientOverlayFactState::default();
        crate::netcode::ingest_client_overlay_fact(
            &mut client_facts,
            &mut pool,
            crate::netcode::ClientOverlayFact::new(
                postretro_net::wire::NetworkId(7),
                0.0,
                0.0,
                false,
                false,
            ),
            None,
            None,
            None,
        );
        let mut host_facts = crate::netcode::HostOverlayFactTracker::default();

        clear_replaced_presentation_overlay_state(&mut pool, &mut client_facts, &mut host_facts);

        assert_eq!(pool.live_counts(), (0, 0));
        assert_eq!(client_facts.terminal_len(), 0);
        assert_eq!(client_facts.pending_len(), 0);
    }
}
