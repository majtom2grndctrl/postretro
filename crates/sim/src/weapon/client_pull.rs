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
    /// running. Nothing is heard or seen.
    Silent,
}

/// Choose a pull's presentation from the replicated magazine and reload
/// state, mirroring what the host authorizes:
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
/// A weapon without ammo presents a fire, as does any value used that is
/// absent or describes a slot other than `active_slot`. So does a magazine
/// the host names absent for this slot: the host holds a resourceless weapon
/// there, which never dry fires.
pub fn client_pull_presentation(
    weapon: &WeaponComponent,
    active_slot: usize,
    projection: &ReplicatedWeaponProjection,
) -> ClientPullPresentation {
    let Some(ammo) = weapon.effective().ammo else {
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
    /// Declare the shot with no hits now. No predicted projectile is shown,
    /// so none will declare a contact later, and the host's authorized shot
    /// must still retire. When the host did fire, that shot's damage is lost.
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
/// `impact` at its contacts, or a predicted projectile. A dry fire raises
/// `dry_fire` alone; a silent pull raises nothing. Every presentation still
/// declares: a hitscan shot declares its resolved hits at once whatever it
/// presents, and a projectile shot that shows no projectile declares empty.
pub fn client_pull_effects(
    presentation: ClientPullPresentation,
    has_projectile_launch: bool,
    has_contacts: bool,
) -> ClientPullEffects {
    let shows_fire = presentation == ClientPullPresentation::Fire;
    let addresses = match presentation {
        ClientPullPresentation::Fire if has_contacts => vec!["activate", "impact"],
        ClientPullPresentation::Fire => vec!["activate"],
        ClientPullPresentation::DryFire => vec!["dry_fire"],
        ClientPullPresentation::Silent => Vec::new(),
    };
    let spawn_projectile = shows_fire && has_projectile_launch;
    let declaration = if !has_projectile_launch {
        ClientShotDeclaration::ResolvedNow
    } else if spawn_projectile {
        ClientShotDeclaration::OnProjectileResolution
    } else {
        ClientShotDeclaration::EmptyNow
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
        use ClientShotDeclaration::{EmptyNow, OnProjectileResolution, ResolvedNow};
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
            // Projectile: only a fire shows a projectile, which declares
            // later; a dry or silent pull declares empty at once. A projectile
            // resolution carries no same-frame contacts; the `true` rows pin
            // that a dry or silent pull would raise no impact even so.
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
                effects(&["dry_fire"], false, EmptyNow),
            ),
            (
                (DryFire, true, true),
                effects(&["dry_fire"], false, EmptyNow),
            ),
            ((Silent, true, false), effects(&[], false, EmptyNow)),
            ((Silent, true, true), effects(&[], false, EmptyNow)),
        ];
        for ((presentation, launch, contacts), expected) in table {
            assert_eq!(
                client_pull_effects(presentation, launch, contacts),
                expected,
                "{presentation:?}, projectile launch {launch}, contacts {contacts}",
            );
        }
    }
}
