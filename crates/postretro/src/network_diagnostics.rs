//! App-side bridge between the live net endpoint's lag proxy and the
//! dev-tools Network tab. The renderer receives a plain-data lever; netcode
//! types stay on this side.
//! See: context/lib/networking.md §Testing the conditioned link

use crate::netcode::{LagProfile, NetEndpoint};
use crate::render::debug_ui::{NetworkLagLever, NetworkSessionRole};

/// The tab's lever for `endpoint`, or `None` when no lag proxy relays for it.
pub(crate) fn lag_lever(endpoint: &NetEndpoint) -> Option<NetworkLagLever> {
    let role = match endpoint {
        NetEndpoint::Host { .. } => NetworkSessionRole::Host,
        NetEndpoint::Client { .. } => NetworkSessionRole::Client,
    };
    let rtt_ms = endpoint
        .measured_rtt()
        .map(|rtt| rtt.as_secs_f64() * 1000.0);
    endpoint
        .lag_profile()
        .map(|profile| lever_from_profile(profile, role, rtt_ms))
}

/// Write the lever back when the tab changed an impairment field.
pub(crate) fn apply_lag_lever(
    endpoint: &mut NetEndpoint,
    before: &NetworkLagLever,
    after: &NetworkLagLever,
) {
    if after.impairment_changed(before) {
        endpoint.set_lag_profile(profile_from_lever(after));
    }
}

fn lever_from_profile(
    profile: LagProfile,
    role: NetworkSessionRole,
    measured_rtt_ms: Option<f64>,
) -> NetworkLagLever {
    NetworkLagLever {
        delay_ms: profile.delay_ms,
        jitter_ms: profile.jitter_ms,
        loss_percent: profile.loss_probability * 100.0,
        role,
        measured_rtt_ms,
    }
}

fn profile_from_lever(lever: &NetworkLagLever) -> LagProfile {
    LagProfile {
        delay_ms: lever.delay_ms,
        jitter_ms: lever.jitter_ms,
        loss_probability: lever.loss_percent / 100.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::netcode::NetRole;

    #[test]
    fn lever_and_profile_convert_both_ways() {
        let profile = LagProfile {
            delay_ms: 75,
            jitter_ms: 30,
            loss_probability: 0.05,
        };
        let lever = lever_from_profile(profile, NetworkSessionRole::Client, Some(150.0));
        assert_eq!((lever.delay_ms, lever.jitter_ms), (75, 30));
        assert!((lever.loss_percent - 5.0).abs() < 1.0e-9);
        assert_eq!(lever.measured_rtt_ms, Some(150.0));
        let back = profile_from_lever(&lever);
        assert_eq!((back.delay_ms, back.jitter_ms), (75, 30));
        assert!((back.loss_probability - 0.05).abs() < 1.0e-12);
    }

    #[test]
    fn standard_preset_matches_the_harness_link() {
        let mut lever = lever_from_profile(LagProfile::OFF, NetworkSessionRole::Host, None);
        lever.set_standard_profile();
        let profile = profile_from_lever(&lever);
        assert_eq!((profile.delay_ms, profile.jitter_ms), (45, 60));
        assert!((profile.loss_probability - 0.05).abs() < 1.0e-12);
    }

    #[test]
    fn host_lever_applies_only_a_changed_impairment() {
        let mut endpoint = NetEndpoint::from_role(&NetRole::Host { port: 0 }, None)
            .expect("host endpoint constructs")
            .expect("host role yields an endpoint");
        let before = lag_lever(&endpoint).expect("dev-tools host is proxied");
        assert_eq!(before.role, NetworkSessionRole::Host);
        assert_eq!(before.measured_rtt_ms, None);
        assert!(!before.impairment_changed(&lever_from_profile(
            LagProfile::OFF,
            NetworkSessionRole::Host,
            None
        )));

        let mut after = before;
        after.delay_ms = 120;
        after.loss_percent = 10.0;
        apply_lag_lever(&mut endpoint, &before, &after);
        let applied = endpoint.lag_profile().expect("proxied");
        assert_eq!((applied.delay_ms, applied.jitter_ms), (120, 0));
        assert!((applied.loss_probability - 0.1).abs() < 1.0e-12);

        // An unchanged lever leaves the live profile alone, even one set
        // elsewhere since the lever was built.
        endpoint.set_lag_profile(LagProfile::OFF);
        apply_lag_lever(&mut endpoint, &after, &after);
        assert_eq!(endpoint.lag_profile(), Some(LagProfile::OFF));
    }
}
