// Connected-client pull presentation: reads the slot-correlated owner-private
// weapon projection to choose what a predicted pull shows, and turns that
// choice into its emissions, projectile spawn and hit declaration. Presentation
// only: every pull the fire gate passes is predicted and declared as a fire.
// See: context/lib/networking.md §Combat authority: FIRE vs HIT

use postretro_entities::components::weapon::WeaponComponent;
use postretro_foundation::ReloadStyle;

/// One owner-private weapon value and the host wieldable slot it describes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlotSample<T> {
    pub slot: usize,
    pub value: T,
}

/// The owner-private weapon projection as a connected client last received
/// it, each value beside the host wieldable slot it describes. The host
/// projects its own active weapon, which lags a local switch by a round trip,
/// and state records arrive per slot rather than atomically, so values may
/// briefly describe different slots. Presentation trusts a value only for the
/// slot it names.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct ReplicatedWeaponProjection {
    /// `player.weaponCooldownMs`.
    pub cooldown: Option<SlotSample<f32>>,
    /// `player.ammo`. A sample whose value is `None` is the host naming a
    /// slot whose weapon has no magazine: an authoritative absence, which
    /// clears the store value as the host HUD does.
    pub magazine: Option<SlotSample<Option<f32>>>,
    /// `player.ammoReserve`, with the same absence as [`Self::magazine`].
    pub reserve: Option<SlotSample<Option<f32>>>,
    /// `player.reloadProgress`.
    pub reload_progress: Option<SlotSample<f32>>,
    /// `player.reloadActive`.
    pub reload_active: Option<SlotSample<bool>>,
    /// `player.heat`, absent for a slot whose weapon runs no heat.
    pub heat: Option<SlotSample<Option<f32>>>,
    /// `player.overheatAt`, with the same absence as [`Self::heat`].
    pub overheat_at: Option<SlotSample<Option<f32>>>,
    /// `player.overheated`; a weapon without heat reads `false`.
    pub overheated: Option<SlotSample<bool>>,
    /// `player.cell`, absent for a slot whose weapon runs no cell.
    pub cell: Option<SlotSample<Option<f32>>>,
    /// `player.cellCapacity`, with the same absence as [`Self::cell`].
    pub cell_capacity: Option<SlotSample<Option<f32>>>,
}

impl ReplicatedWeaponProjection {
    /// Take every value `fresh` carries and keep the rest. A fresh absence is
    /// carried: it replaces the held count.
    pub fn merge(&mut self, fresh: &Self) {
        fn take<T: Copy>(held: &mut Option<SlotSample<T>>, fresh: Option<SlotSample<T>>) {
            if fresh.is_some() {
                *held = fresh;
            }
        }
        take(&mut self.cooldown, fresh.cooldown);
        take(&mut self.magazine, fresh.magazine);
        take(&mut self.reserve, fresh.reserve);
        take(&mut self.reload_progress, fresh.reload_progress);
        take(&mut self.reload_active, fresh.reload_active);
        take(&mut self.heat, fresh.heat);
        take(&mut self.overheat_at, fresh.overheat_at);
        take(&mut self.overheated, fresh.overheated);
        take(&mut self.cell, fresh.cell);
        take(&mut self.cell_capacity, fresh.cell_capacity);
    }
}

/// A sample's value when it describes `slot`.
fn sample_for_slot<T: Copy>(sample: Option<SlotSample<T>>, slot: usize) -> Option<T> {
    sample
        .filter(|sample| sample.slot == slot)
        .map(|sample| sample.value)
}

/// What a connected client shows for a trigger pull. Presentation only: every
/// pull the fire gate passes is predicted and declared the same way, so the
/// host applies damage whenever it fires, and a wrong guess costs a sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientPullPresentation {
    /// Fire sound, muzzle FX, predicted impact and any predicted projectile.
    Fire,
    /// The host's `Empty`: the dry-fire sound only.
    DryFire,
    /// The host's silent `Rejected`: a reload the pull cannot cancel is
    /// running, or the weapon is overheated. Nothing is heard or seen.
    Silent,
}

/// Choose a pull's presentation from the replicated resource and reload
/// state, mirroring what the host authorizes:
///
/// - An overheated heat weapon is refused silently. The crossing shot itself
///   fires: the latch it sets arrives a round trip later.
/// - A cell weapon whose charge cannot pay for a shot dry fires.
///
/// - An idle weapon whose magazine cannot pay for a shot dry fires; with ammo
///   it fires.
/// - During a magazine reload the host refuses every pull. During a per-shell
///   reload a pull the magazine covers cancels the reload and fires; one it
///   does not cover is refused.
/// - A reload flag held at full progress is the Completed endpoint the owner
///   projection replays; live progress stays below 1 while a reload runs, so
///   the weapon is idle.
///
/// A resourceless weapon presents a fire, as does any value used that is
/// absent or describes a slot other than `active_slot`. So does a magazine or
/// charge the host names absent for this slot: the host holds a weapon of
/// another kind there, which this rule cannot judge.
pub fn client_pull_presentation(
    weapon: &WeaponComponent,
    active_slot: usize,
    projection: &ReplicatedWeaponProjection,
) -> ClientPullPresentation {
    let effective = weapon.effective();
    if effective.heat.is_some() {
        return match sample_for_slot(projection.overheated, active_slot) {
            Some(true) => ClientPullPresentation::Silent,
            _ => ClientPullPresentation::Fire,
        };
    }
    if let Some(cell) = effective.cell {
        return match sample_for_slot(projection.cell, active_slot) {
            Some(Some(charge)) if charge < cell.cost_per_shot => ClientPullPresentation::DryFire,
            _ => ClientPullPresentation::Fire,
        };
    }
    let Some(ammo) = effective.ammo else {
        return ClientPullPresentation::Fire;
    };
    let (Some(Some(magazine)), Some(reload_active)) = (
        sample_for_slot(projection.magazine, active_slot),
        sample_for_slot(projection.reload_active, active_slot),
    ) else {
        return ClientPullPresentation::Fire;
    };
    let reloading = if reload_active {
        let Some(progress) = sample_for_slot(projection.reload_progress, active_slot) else {
            return ClientPullPresentation::Fire;
        };
        progress < 1.0
    } else {
        false
    };
    let covers_shot = magazine >= ammo.cost_per_shot as f32;
    match (reloading, ammo.reload_style) {
        (false, _) if covers_shot => ClientPullPresentation::Fire,
        (false, _) => ClientPullPresentation::DryFire,
        (true, ReloadStyle::PerShell) if covers_shot => ClientPullPresentation::Fire,
        (true, _) => ClientPullPresentation::Silent,
    }
}

/// When a predicted pull declares its hits to the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientShotDeclaration {
    /// Declare the resolved hitscan hits and world contacts now.
    ResolvedNow,
    /// Declare the shot with no hits now. An actual materialization failure or expiry has no later contact to declare.
    EmptyNow,
    /// The predicted projectile declares its contact or expiry later. If it
    /// fails to materialize, the caller declares [`Self::EmptyNow`] instead.
    OnProjectileResolution,
}

/// What a predicted pull does once it has a resolution, by presentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientPullEffects {
    /// Weapon event addresses to raise, in order. `impact` carries the
    /// resolution's contacts; the others carry the shooter.
    pub addresses: Vec<&'static str>,
    /// Spawn the predicted projectile from the resolution's launch.
    pub spawn_projectile: bool,
    pub declaration: ClientShotDeclaration,
}

/// Turn a predicted pull's presentation into its effects. `has_projectile_launch`
/// is whether the resolution carries a projectile launch; `has_contacts` is
/// whether it resolved any hitscan contact.
///
/// Only a [`ClientPullPresentation::Fire`] raises `activate` (muzzle FX), an
/// `impact` at its contacts, or projectile cosmetics. A dry fire raises
/// `dry_fire` alone; a silent pull raises nothing. Every presentation still
/// declares: a hitscan shot declares its resolved hits at once whatever it
/// presents, and every projectile simulates and declares contact regardless of cosmetics.
pub fn client_pull_effects(
    presentation: ClientPullPresentation,
    has_projectile_launch: bool,
    has_contacts: bool,
) -> ClientPullEffects {
    let addresses = match presentation {
        ClientPullPresentation::Fire if has_contacts => vec!["activate", "impact"],
        ClientPullPresentation::Fire => vec!["activate"],
        ClientPullPresentation::DryFire => vec!["dry_fire"],
        ClientPullPresentation::Silent => Vec::new(),
    };
    let spawn_projectile = has_projectile_launch;
    let declaration = if !has_projectile_launch {
        ClientShotDeclaration::ResolvedNow
    } else {
        ClientShotDeclaration::OnProjectileResolution
    };
    ClientPullEffects {
        addresses,
        spawn_projectile,
        declaration,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn effects(
        addresses: &[&'static str],
        spawn_projectile: bool,
        declaration: ClientShotDeclaration,
    ) -> ClientPullEffects {
        ClientPullEffects {
            addresses: addresses.to_vec(),
            spawn_projectile,
            declaration,
        }
    }

    #[test]
    fn client_pull_effects_cover_every_presentation_and_resolution() {
        use ClientPullPresentation::{DryFire, Fire, Silent};
        use ClientShotDeclaration::{OnProjectileResolution, ResolvedNow};
        // ((presentation, projectile launch, contacts), effects)
        let table = [
            // Hitscan: every presentation declares its resolved hits now.
            (
                (Fire, false, true),
                effects(&["activate", "impact"], false, ResolvedNow),
            ),
            (
                (Fire, false, false),
                effects(&["activate"], false, ResolvedNow),
            ),
            (
                (DryFire, false, true),
                effects(&["dry_fire"], false, ResolvedNow),
            ),
            (
                (DryFire, false, false),
                effects(&["dry_fire"], false, ResolvedNow),
            ),
            ((Silent, false, true), effects(&[], false, ResolvedNow)),
            ((Silent, false, false), effects(&[], false, ResolvedNow)),
            // Every semantic projectile declares its later resolution. Cosmetic
            // dry/silent feedback never skips the flight or creates a placeholder.
            (
                (Fire, true, false),
                effects(&["activate"], true, OnProjectileResolution),
            ),
            (
                (Fire, true, true),
                effects(&["activate", "impact"], true, OnProjectileResolution),
            ),
            (
                (DryFire, true, false),
                effects(&["dry_fire"], true, OnProjectileResolution),
            ),
            (
                (DryFire, true, true),
                effects(&["dry_fire"], true, OnProjectileResolution),
            ),
            (
                (Silent, true, false),
                effects(&[], true, OnProjectileResolution),
            ),
            (
                (Silent, true, true),
                effects(&[], true, OnProjectileResolution),
            ),
        ];
        for ((presentation, launch, contacts), expected) in table {
            assert_eq!(
                client_pull_effects(presentation, launch, contacts),
                expected,
                "{presentation:?}, projectile launch {launch}, contacts {contacts}",
            );
        }
    }

    fn resource_weapon(resource: serde_json::Value) -> WeaponComponent {
        let descriptor: postretro_entities::data_descriptors::WeaponDescriptor =
            serde_json::from_value(serde_json::json!({
                "damage": 10.0,
                "range": 64.0,
                "primary": { "trigger": "hold", "recoveryMs": 100.0, "steps": [{ "kind": "shot" }] },

                "resolution": "hitscan",
                "resource": resource,
            }))
            .unwrap();
        WeaponComponent::from_descriptor(&descriptor)
    }

    fn heat_weapon() -> WeaponComponent {
        resource_weapon(serde_json::json!({
            "kind": "heat", "heatPerShot": 10.0, "overheatAt": 80.0, "coolPerSecond": 20.0
        }))
    }

    fn cell_weapon() -> WeaponComponent {
        resource_weapon(serde_json::json!({
            "kind": "cell", "capacity": 40.0, "costPerShot": 4.0, "regenPerSecond": 8.0
        }))
    }

    fn on_slot<T>(slot: usize, value: T) -> Option<SlotSample<T>> {
        Some(SlotSample { slot, value })
    }

    #[test]
    fn client_pull_an_overheated_heat_weapon_presents_silent_only_for_its_own_slot() {
        use ClientPullPresentation::{Fire, Silent};
        let weapon = heat_weapon();
        let overheated = |slot, value| ReplicatedWeaponProjection {
            overheated: on_slot(slot, value),
            heat: on_slot(slot, Some(80.0)),
            ..ReplicatedWeaponProjection::default()
        };
        assert_eq!(
            client_pull_presentation(&weapon, 1, &overheated(1, true)),
            Silent
        );
        assert_eq!(
            client_pull_presentation(&weapon, 1, &overheated(1, false)),
            Fire,
            "a hot but unlatched weapon fires, including the crossing shot"
        );
        assert_eq!(
            client_pull_presentation(&weapon, 1, &overheated(0, true)),
            Fire,
            "a latch that names another slot describes another weapon"
        );
        assert_eq!(
            client_pull_presentation(&weapon, 1, &ReplicatedWeaponProjection::default()),
            Fire,
            "nothing replicated yet"
        );
    }

    #[test]
    fn client_pull_a_cell_below_cost_presents_dry_fire_only_for_its_own_slot() {
        use ClientPullPresentation::{DryFire, Fire};
        let weapon = cell_weapon();
        let charge = |slot, value| ReplicatedWeaponProjection {
            cell: on_slot(slot, value),
            cell_capacity: on_slot(slot, value.map(|_| 40.0)),
            ..ReplicatedWeaponProjection::default()
        };
        assert_eq!(
            client_pull_presentation(&weapon, 2, &charge(2, Some(3.9))),
            DryFire
        );
        assert_eq!(
            client_pull_presentation(&weapon, 2, &charge(2, Some(4.0))),
            Fire,
            "a charge that exactly pays the cost fires"
        );
        assert_eq!(
            client_pull_presentation(&weapon, 2, &charge(1, Some(0.0))),
            Fire,
            "an empty cell that names another slot describes another weapon"
        );
        assert_eq!(
            client_pull_presentation(&weapon, 2, &charge(2, None)),
            Fire,
            "the host names this slot's weapon cell-less"
        );
        assert_eq!(
            client_pull_presentation(&weapon, 2, &ReplicatedWeaponProjection::default()),
            Fire,
            "nothing replicated yet"
        );
    }

    #[test]
    fn client_pull_a_resource_rule_reads_only_its_own_kind() {
        use ClientPullPresentation::Fire;
        // An empty magazine or cell never silences a heat weapon, and a latch
        // never dry-fires a cell weapon: each rule reads its own kind's value.
        let everything_empty = ReplicatedWeaponProjection {
            magazine: on_slot(0, Some(0.0)),
            reload_active: on_slot(0, false),
            overheated: on_slot(0, false),
            cell: on_slot(0, Some(0.0)),
            ..ReplicatedWeaponProjection::default()
        };
        assert_eq!(
            client_pull_presentation(&heat_weapon(), 0, &everything_empty),
            Fire
        );
        let latched = ReplicatedWeaponProjection {
            overheated: on_slot(0, true),
            ..ReplicatedWeaponProjection::default()
        };
        assert_eq!(client_pull_presentation(&cell_weapon(), 0, &latched), Fire);
    }

    #[test]
    fn client_pull_merge_carries_heat_and_cell_samples_and_absences() {
        let mut held = ReplicatedWeaponProjection {
            heat: on_slot(0, Some(30.0)),
            overheat_at: on_slot(0, Some(80.0)),
            overheated: on_slot(0, true),
            cell: on_slot(0, None),
            cell_capacity: on_slot(0, None),
            ..ReplicatedWeaponProjection::default()
        };
        held.merge(&ReplicatedWeaponProjection {
            heat: on_slot(1, None),
            overheated: on_slot(1, false),
            cell: on_slot(1, Some(12.0)),
            ..ReplicatedWeaponProjection::default()
        });
        assert_eq!(held.heat, on_slot(1, None), "a fresh absence replaces heat");
        assert_eq!(held.overheat_at, on_slot(0, Some(80.0)), "not carried");
        assert_eq!(held.overheated, on_slot(1, false));
        assert_eq!(held.cell, on_slot(1, Some(12.0)));
        assert_eq!(held.cell_capacity, on_slot(0, None), "not carried");
    }

    fn predicted_due(weapon: &mut WeaponComponent, button: crate::weapon::FireButtonState) -> bool {
        crate::weapon::activation_prediction::advance_predicted_weapon_tick(
            weapon,
            crate::weapon::execution::ActivationCommand {
                tick: 1,
                pawn: 0,
                real_command: true,
                input: postretro_foundation::ActivationInput::default(),
                controller_starts: true,
                primary: button,
                secondary: crate::weapon::FireButtonState {
                    pressed: false,
                    active: false,
                },
            },
            false,
            1000.0 / 60.0,
            true,
        )
        .shot
        .is_some()
    }
    #[test]
    fn client_pull_fire_gate_never_simulates_heat_or_cell() {
        // Connected clients read replicated heat and cell; the local fire gate
        // neither consults nor advances them.
        let pull = crate::weapon::FireButtonState {
            pressed: true,
            active: true,
        };
        let mut hot = heat_weapon();
        let latched = {
            let heat = hot.heat.as_mut().unwrap();
            heat.heat = 80.0;
            heat.overheated = true;
            *heat
        };
        assert!(predicted_due(&mut hot, pull));
        assert_eq!(hot.heat, Some(latched), "no cooling, no latch clear");

        let mut drained = cell_weapon();
        let empty = {
            let cell = drained.cell.as_mut().unwrap();
            cell.charge = 0.0;
            *cell
        };
        assert!(predicted_due(&mut drained, pull));
        assert_eq!(drained.cell, Some(empty), "no regeneration");
    }
}

/// A stale owner projection chooses cosmetics using the frozen shot's actual cost.
pub fn client_shot_presentation(
    weapon: &WeaponComponent,
    active_slot: usize,
    projection: &ReplicatedWeaponProjection,
    shot: &super::ResolvedWeaponShot,
) -> ClientPullPresentation {
    use postretro_foundation::ShotResourceCost;
    match shot.activation.values.resource_cost {
        ShotResourceCost::Heat(_) => {
            if sample_for_slot(projection.overheated, active_slot) == Some(true) {
                ClientPullPresentation::Silent
            } else {
                ClientPullPresentation::Fire
            }
        }
        ShotResourceCost::Cell(cost) => {
            if matches!(sample_for_slot(projection.cell, active_slot), Some(Some(charge)) if charge < cost)
            {
                ClientPullPresentation::DryFire
            } else {
                ClientPullPresentation::Fire
            }
        }
        ShotResourceCost::Ammo(cost) => {
            let (Some(Some(magazine)), Some(reload_active)) = (
                sample_for_slot(projection.magazine, active_slot),
                sample_for_slot(projection.reload_active, active_slot),
            ) else {
                return ClientPullPresentation::Fire;
            };
            let reloading = if reload_active {
                let Some(progress) = sample_for_slot(projection.reload_progress, active_slot)
                else {
                    return ClientPullPresentation::Fire;
                };
                progress < 1.0
            } else {
                false
            };
            let per_shell = weapon
                .effective()
                .ammo
                .is_some_and(|ammo| ammo.reload_style == ReloadStyle::PerShell);
            if reloading && !(per_shell && magazine >= cost as f32) {
                ClientPullPresentation::Silent
            } else if magazine >= cost as f32 {
                ClientPullPresentation::Fire
            } else {
                ClientPullPresentation::DryFire
            }
        }
        ShotResourceCost::None => ClientPullPresentation::Fire,
    }
}
