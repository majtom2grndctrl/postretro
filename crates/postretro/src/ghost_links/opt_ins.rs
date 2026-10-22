// Descriptor ghost opt-ins as this peer reads them: descriptor `ghost` and `projectile.ghost`.
// See: context/lib/entity_model.md §7d

use std::collections::HashSet;

use postretro_entities::{DataRegistry, EntityTypeDescriptor};

/// The archetypes this peer's own descriptors opt in, by canonical name. Each
/// peer reads its own descriptors, never the wire, so peers whose descriptors
/// differ see different ghosts.
///
/// Re-derived whenever the committed descriptor snapshot's generation moves, so
/// a committed reload that flips an opt-in re-resolves membership on the next
/// frame, through the membership ramp. Lookups are then hash probes, allocation
/// free.
#[derive(Debug, Default)]
pub(crate) struct GhostOptIns {
    /// The descriptor generation these sets were derived from; `None` before
    /// the first refresh.
    generation: Option<u64>,
    /// Descriptors whose top-level `ghost` is set.
    descriptors: HashSet<String>,
    /// Weapon descriptors whose `projectile.ghost` is set.
    projectile_weapons: HashSet<String>,
}

impl GhostOptIns {
    /// Follow the committed descriptor snapshot. Rebuilds only when its
    /// generation changed.
    pub(crate) fn refresh(&mut self, data: &DataRegistry) {
        let generation = data.entity_types_generation();
        if self.generation == Some(generation) {
            return;
        }
        self.generation = Some(generation);
        self.rebuild(&data.entities);
    }

    fn rebuild(&mut self, descriptors: &[EntityTypeDescriptor]) {
        self.descriptors.clear();
        self.projectile_weapons.clear();
        for descriptor in descriptors {
            let Some(name) = descriptor.canonical_name.as_deref() else {
                continue;
            };
            if descriptor.ghost {
                self.descriptors.insert(name.to_string());
            }
            let projectile_ghost = descriptor
                .weapon
                .as_ref()
                .and_then(|weapon| weapon.projectile.as_ref())
                .is_some_and(|projectile| projectile.ghost);
            if projectile_ghost {
                self.projectile_weapons.insert(name.to_string());
            }
        }
    }

    /// Whether descriptor `name` sets `ghost`.
    pub(crate) fn descriptor(&self, name: &str) -> bool {
        self.descriptors.contains(name)
    }

    /// Whether weapon descriptor `name` sets `projectile.ghost`.
    pub(crate) fn projectile(&self, weapon: &str) -> bool {
        self.projectile_weapons.contains(weapon)
    }
}

/// The install warning's stable wording (acceptance contract).
pub(crate) const NO_OPT_IN_WARNING: &str = "no loaded descriptor sets `ghost`";

/// Warn once, at level install on every role, when the level has links but no
/// loaded descriptor sets top-level `ghost`: no pawn or descriptor entity can
/// ghost, so a forgotten opt-in is never silent. A weapon's `projectile.ghost`
/// or a matched `ghost_tags` does not quiet it; the warning names what can
/// still ghost, or says nothing can. `movers_ghost`: some link's `ghost_tags`
/// matched a mover at install.
pub(crate) fn warn_if_links_without_opt_ins(
    link_count: usize,
    movers_ghost: bool,
    data: &DataRegistry,
) {
    if link_count == 0 {
        return;
    }
    let mut opt_ins = GhostOptIns::default();
    opt_ins.refresh(data);
    if !opt_ins.descriptors.is_empty() {
        return;
    }
    let still_ghost = match (movers_ghost, !opt_ins.projectile_weapons.is_empty()) {
        (true, false) => "only tagged movers can",
        (true, true) => {
            "only tagged movers and projectiles whose weapon sets `projectile.ghost` can"
        }
        (false, true) => "only projectiles whose weapon sets `projectile.ghost` can",
        (false, false) => "nothing can ghost",
    };
    let links = if link_count == 1 { "link" } else { "links" };
    log::warn!(
        "[Ghost] level has {link_count} ghost {links} but {NO_OPT_IN_WARNING}, so no pawn \
         or descriptor entity ghosts; {still_ghost}"
    );
}
