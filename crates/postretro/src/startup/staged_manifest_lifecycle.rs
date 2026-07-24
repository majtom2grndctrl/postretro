// Main-thread staged mod-manifest commit lifecycle. The scripting runtime owns
// build/validation; this app-side seam applies successful snapshots to the
// session, active level, and UI in their required commit order.
// See: context/lib/boot_sequence.md §1 · context/lib/scripting.md

use postretro_scripting_core::runtime::StagedManifestCommitOutcome;
use postretro_scripting_core::staged_manifest::StagedManifestBuildStatus;

use crate::App;

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
            self.commit_staged_ui_manifest(&result, &outcome);
        }
    }
}
