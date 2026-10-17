// Dev-tools launch seed for the lag proxy: `POSTRETRO_NET_LAG`.
// See: context/lib/networking.md §Testing the conditioned link

use crate::endpoint::{LagProfile, NetEndpoint};

/// Environment variable that seeds the lag proxy at launch.
const LAG_SEED_ENV: &str = "POSTRETRO_NET_LAG";

/// Apply the `POSTRETRO_NET_LAG` profile to a freshly built endpoint, before
/// its first poll, so the handshake already runs conditioned. Unset or empty
/// leaves the proxy off; a malformed value warns once and leaves it off.
pub fn seed_lag_profile_from_environment(endpoint: &mut NetEndpoint) {
    apply_lag_seed(endpoint, std::env::var(LAG_SEED_ENV));
}

fn apply_lag_seed(endpoint: &mut NetEndpoint, value: Result<String, std::env::VarError>) {
    let value = match value {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => return,
        Err(std::env::VarError::NotUnicode(_)) => {
            warn_malformed();
            return;
        }
    };
    if value.trim().is_empty() {
        return;
    }
    let Some(profile) = parse_lag_seed(&value) else {
        warn_malformed();
        return;
    };
    endpoint.set_lag_profile(profile);
    if !profile.is_off() {
        log::info!(
            "[Net] lag proxy seeded from {LAG_SEED_ENV}: delay {} ms, jitter {} ms, loss {}% per direction",
            profile.delay_ms,
            profile.jitter_ms,
            profile.loss_probability * 100.0,
        );
    }
}

fn warn_malformed() {
    log::warn!(
        "[Net] ignoring malformed {LAG_SEED_ENV}; expected delay_ms[,jitter_ms[,loss_percent]] \
         (e.g. 75,30,5; loss 0-100); running unimpaired"
    );
}

/// Parse `delay_ms[,jitter_ms[,loss_percent]]`. Delay and jitter are whole
/// milliseconds; loss is a percent in `[0, 100]` and may be fractional. Omitted
/// trailing fields are zero. Whitespace around a field is ignored. `None` for
/// anything else.
#[must_use]
fn parse_lag_seed(value: &str) -> Option<LagProfile> {
    let mut fields = value.split(',').map(str::trim);
    let delay_ms = fields.next()?.parse().ok()?;
    let jitter_ms = match fields.next() {
        Some(field) => field.parse().ok()?,
        None => 0,
    };
    let loss_percent: f64 = match fields.next() {
        Some(field) => field.parse().ok()?,
        None => 0.0,
    };
    if fields.next().is_some() || !(0.0..=100.0).contains(&loss_percent) {
        return None;
    }
    Some(LagProfile {
        delay_ms,
        jitter_ms,
        loss_probability: loss_percent / 100.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::Level;
    use postretro_test_log_capture::LogCapture;

    fn profile(delay_ms: u32, jitter_ms: u32, loss_probability: f64) -> LagProfile {
        LagProfile {
            delay_ms,
            jitter_ms,
            loss_probability,
        }
    }

    #[test]
    fn lag_seed_accepts_one_to_three_fields() {
        for (value, expected) in [
            ("75,30,5", profile(75, 30, 0.05)),
            (" 75 , 30 , 5 ", profile(75, 30, 0.05)),
            ("75", profile(75, 0, 0.0)),
            ("75,30", profile(75, 30, 0.0)),
            ("0,0,2.5", profile(0, 0, 0.025)),
            ("0,0,100", profile(0, 0, 1.0)),
            ("0", LagProfile::OFF),
        ] {
            assert_eq!(parse_lag_seed(value), Some(expected), "{value:?}");
        }
    }

    #[test]
    fn lag_seed_refuses_malformed_values() {
        for value in [
            "",
            ",",
            "75,",
            "75,30,",
            "75,30,5,1",
            "-1",
            "75.5",
            "75,-30",
            "75,30,-1",
            "75,30,101",
            "75,30,NaN",
            "75,30,inf",
            "75ms",
            "fast",
        ] {
            assert_eq!(parse_lag_seed(value), None, "{value:?}");
        }
    }

    fn host_endpoint() -> NetEndpoint {
        NetEndpoint::from_role(&crate::NetRole::Host { port: 0 }, None)
            .expect("host endpoint constructs")
            .expect("host role yields an endpoint")
    }

    #[test]
    fn lag_seed_conditions_the_endpoint_and_logs_the_active_profile() {
        let capture = LogCapture::start();
        let mut endpoint = host_endpoint();
        apply_lag_seed(&mut endpoint, Ok("75,30,5".to_owned()));
        assert_eq!(endpoint.lag_profile(), Some(profile(75, 30, 0.05)));
        capture.assert_logged_once(Level::Info, "lag proxy seeded from POSTRETRO_NET_LAG");
        capture.assert_not_logged(Level::Warn, LAG_SEED_ENV);
    }

    #[test]
    fn malformed_or_absent_lag_seed_leaves_the_endpoint_unimpaired() {
        let capture = LogCapture::start();
        let mut endpoint = host_endpoint();
        apply_lag_seed(&mut endpoint, Ok("75;30;5".to_owned()));
        assert_eq!(endpoint.lag_profile(), Some(LagProfile::OFF));
        capture.assert_logged_once(Level::Warn, "malformed POSTRETRO_NET_LAG");
        capture.clear();
        for value in [
            Err(std::env::VarError::NotPresent),
            Ok(String::new()),
            Ok("0".to_owned()),
        ] {
            apply_lag_seed(&mut endpoint, value);
            assert_eq!(endpoint.lag_profile(), Some(LagProfile::OFF));
        }
        assert!(capture.records().is_empty(), "off seeds log nothing");
    }
}
