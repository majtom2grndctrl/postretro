// Command relevance: which commands a game uses, derived from its data.
// See: context/lib/input.md §2 (Relevance)

use super::commands::Command;

/// Facts about the mounted game that decide relevance. The App derives them
/// from the entity registry and, while participating in co-op, from the
/// installed host tuning, so the input module stays free of registry types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RelevanceFacts {
    /// Any movement descriptor carries dash.
    pub dash: bool,
    /// Any movement descriptor carries crouch.
    pub crouch: bool,
    /// Any weapon's resource is the magazine kind.
    pub magazine: bool,
    /// Any weapon declares a secondary activation.
    pub secondary: bool,
}

impl RelevanceFacts {
    /// Every data-driven command relevant: what validation checks against, and
    /// how a rebind sees the saved rows of commands the game does not use now.
    pub const EVERY: Self = Self {
        dash: true,
        crouch: true,
        magazine: true,
        secondary: true,
    };

    /// The union of two fact sets: co-op relevance is local derivation plus
    /// the host tuning, because tuning sites keep no local fallback.
    pub fn union(self, other: Self) -> Self {
        Self {
            dash: self.dash || other.dash,
            crouch: self.crouch || other.crouch,
            magazine: self.magazine || other.magazine,
            secondary: self.secondary || other.secondary,
        }
    }
}

/// How a command takes part in a game's bindings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relevance {
    /// Bound, listed in the controls panel, part of conflicts.
    Relevant,
    /// Unbound, unlisted, never in a conflict, draws no glyph.
    Irrelevant,
    /// Bound to engine defaults only, unlisted, never in a conflict (fly-cam).
    DevOnly,
}

/// Derived relevance before the author's `show` override. An exhaustive match,
/// so a new command fails to compile until it is classified: a default
/// "relevant" rule would fail open (`networking.md` §What gates).
pub const fn derived_relevance(command: Command, facts: RelevanceFacts) -> Relevance {
    let relevant = match command {
        Command::Dash => facts.dash,
        Command::Crouch => facts.crouch,
        Command::Reload => facts.magazine,
        Command::AltFire => facts.secondary,
        Command::MoveUp | Command::MoveDown => return Relevance::DevOnly,
        Command::MoveForward
        | Command::MoveBack
        | Command::MoveLeft
        | Command::MoveRight
        | Command::LookX
        | Command::LookY
        | Command::Sprint
        | Command::Jump
        | Command::Use
        | Command::Drop
        | Command::Shoot
        | Command::SelectWieldable1
        | Command::SelectWieldable2
        | Command::SelectWieldable3
        | Command::SelectWieldable4
        | Command::SelectWieldable5
        | Command::SelectWieldable6
        | Command::SelectWieldable7
        | Command::SelectWieldable8
        | Command::SelectWieldable9
        | Command::SelectWieldable10
        | Command::CycleWieldableNext
        | Command::CycleWieldablePrevious
        | Command::ToggleLastWieldable => true,
        // UI commands are always relevant and cannot be hidden.
        Command::NavUp
        | Command::NavDown
        | Command::NavLeft
        | Command::NavRight
        | Command::NavNext
        | Command::NavPrev
        | Command::NavTabNext
        | Command::NavTabPrev
        | Command::NavConfirm
        | Command::NavCancel
        | Command::NavMenu
        | Command::NavOptions
        | Command::TextBackspace
        | Command::TextSpace
        | Command::TextCommit => true,
    };
    if relevant {
        Relevance::Relevant
    } else {
        Relevance::Irrelevant
    }
}

/// Whether the author's `show` may override this command's derivation. UI
/// commands are always shown and dev-only commands are never listed.
pub fn show_override_allowed(command: Command) -> bool {
    matches!(
        derived_relevance(command, RelevanceFacts::default()),
        Relevance::Relevant | Relevance::Irrelevant
    ) && command.context() == super::commands::CommandContext::Gameplay
}

/// Relevance after the author's `show` override, where one is allowed.
pub fn relevance(command: Command, facts: RelevanceFacts, show: Option<bool>) -> Relevance {
    match (derived_relevance(command, facts), show) {
        (Relevance::DevOnly, _) => Relevance::DevOnly,
        (_, Some(true)) if show_override_allowed(command) => Relevance::Relevant,
        (_, Some(false)) if show_override_allowed(command) => Relevance::Irrelevant,
        (derived, _) => derived,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> RelevanceFacts {
        RelevanceFacts::default()
    }

    #[test]
    fn data_driven_commands_follow_their_facts() {
        let none = facts();
        let all = RelevanceFacts {
            dash: true,
            crouch: true,
            magazine: true,
            secondary: true,
        };
        for command in [
            Command::Dash,
            Command::Crouch,
            Command::Reload,
            Command::AltFire,
        ] {
            assert_eq!(derived_relevance(command, none), Relevance::Irrelevant);
            assert_eq!(derived_relevance(command, all), Relevance::Relevant);
        }
        let only_magazine = RelevanceFacts {
            magazine: true,
            ..none
        };
        assert_eq!(
            derived_relevance(Command::Reload, only_magazine),
            Relevance::Relevant
        );
        assert_eq!(
            derived_relevance(Command::Dash, only_magazine),
            Relevance::Irrelevant
        );
    }

    #[test]
    fn fly_cam_commands_are_dev_only_whatever_the_author_shows() {
        for command in [Command::MoveUp, Command::MoveDown] {
            assert_eq!(relevance(command, facts(), None), Relevance::DevOnly);
            assert_eq!(relevance(command, facts(), Some(true)), Relevance::DevOnly);
        }
    }

    #[test]
    fn force_show_and_force_hide_flip_the_derived_answer() {
        assert_eq!(
            relevance(Command::Dash, facts(), Some(true)),
            Relevance::Relevant
        );
        let with_secondary = RelevanceFacts {
            secondary: true,
            ..facts()
        };
        assert_eq!(
            relevance(Command::AltFire, with_secondary, Some(false)),
            Relevance::Irrelevant
        );
    }

    #[test]
    fn ui_commands_cannot_be_hidden() {
        assert_eq!(
            relevance(Command::NavConfirm, facts(), Some(false)),
            Relevance::Relevant
        );
        assert!(!show_override_allowed(Command::NavConfirm));
    }

    #[test]
    fn co_op_relevance_is_the_union_with_host_tuning() {
        let local = facts();
        let host = RelevanceFacts {
            dash: true,
            ..facts()
        };
        assert!(local.union(host).dash);
        assert!(!local.union(facts()).dash);
    }
}
