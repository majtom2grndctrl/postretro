// Role entry: the one construction of a session's endpoint, seat ledger and
// role-bound state, shared by boot flags and in-app host and join so the two
// cannot drift.
// See: context/lib/networking.md §Role model

use std::net::{IpAddr, SocketAddr};

use postretro_foundation::Seat;
use postretro_net::wire::{ConnectClaim, PlayerClaimId, encode_connect_claim};

use super::ScriptingCore;
use super::role_state::{RoleBoundState, SessionRole, apply_role};
use crate::netcode;

/// Why a role's endpoint could not be built. The machine stays single-player.
#[derive(Debug)]
pub(crate) enum EntryFailure {
    /// Socket bind or transport construction failed.
    Endpoint(String),
    /// Session-id entropy was unavailable for a hosted run.
    Identity(String),
}

/// What entering a role built.
pub(crate) struct RoleEntry {
    /// The endpoint the role holds: `None` for single-player and for any
    /// failure.
    pub(crate) endpoint: Option<netcode::NetEndpoint>,
    pub(crate) failure: Option<EntryFailure>,
    /// Remote seats the role change released, whose per-owner slot values the
    /// caller clears.
    pub(crate) released: Vec<Seat>,
}

/// Build the endpoint for `net_role`, then the seat ledger and every
/// role-bound value. A failed endpoint or identity leaves the machine
/// single-player. A machine that already holds a seat ledger and enters a
/// hosted run mints a fresh session identity and seat counter in place from
/// `mint`, keeping seat 0's carried state; if that mint fails, the ledger is
/// untouched.
pub(crate) fn enter_role(
    net_role: &netcode::NetRole,
    player_id: Option<[u8; 16]>,
    scripting: &mut ScriptingCore,
    seat_table: &mut Option<netcode::SeatTable>,
    mint: netcode::SessionIdSource,
    probe_lan: impl FnOnce() -> Option<IpAddr>,
) -> RoleEntry {
    let client_user_data = player_id
        .map(|player_id| ConnectClaim {
            player_id: PlayerClaimId(player_id),
            display_name: String::new(),
        })
        .as_ref()
        .map(encode_connect_claim);
    let mut failure = None;
    let mut endpoint = match netcode::NetEndpoint::from_role(net_role, client_user_data) {
        Ok(endpoint) => {
            match net_role {
                netcode::NetRole::SinglePlayer => {}
                netcode::NetRole::Host { port } => {
                    log::info!("[Net] hosting (listen server) on port {port}");
                }
                netcode::NetRole::Connect { addr } => {
                    log::info!("[Net] connecting to {addr}");
                }
            }
            endpoint
        }
        Err(err) => {
            log::error!("[Net] endpoint setup failed ({err}); starting single-player");
            failure = Some(EntryFailure::Endpoint(err));
            None
        }
    };
    // Dev builds relay every session through the lag proxy; seed it before
    // the first poll so the handshake already runs conditioned.
    #[cfg(feature = "dev-tools")]
    if let Some(endpoint) = endpoint.as_mut() {
        netcode::seed_lag_profile_from_environment(endpoint);
    }
    if !matches!(&endpoint, Some(netcode::NetEndpoint::Client { .. })) {
        let identity_error = match seat_table.as_mut() {
            None => {
                let (seats, identity_error) =
                    authority_seat_table_or_local_only(&mut endpoint, netcode::SeatTable::new());
                *seat_table = Some(seats);
                identity_error
            }
            Some(seats) if endpoint.is_some() => match seats.remint_for_hosting(mint) {
                Ok(()) => None,
                Err(err) => {
                    endpoint = None;
                    Some(err)
                }
            },
            Some(_) => None,
        };
        if let Some(err) = identity_error {
            log::error!(
                "[Net] session identity setup failed ({err}); disabling networking for this session"
            );
            failure.get_or_insert(EntryFailure::Identity(err.to_string()));
        }
    }
    // Identity setup above can still have demoted the endpoint to
    // single-player, so the role reads it after. A listen host publishes the
    // address players should dial, for the frontend to show.
    let role = SessionRole::of(endpoint.as_ref());
    let published = published_host_address(net_role, role == SessionRole::Host, probe_lan);
    if let Some(address) = published {
        log::info!("[Net] players join with --connect {address}");
    }
    let registry = scripting.script_ctx.registry.clone();
    let released = apply_role(
        scripting,
        seat_table,
        &mut registry.borrow_mut(),
        &RoleBoundState::derive(role, published),
    );
    RoleEntry {
        endpoint,
        failure,
        released,
    }
}

/// The address a live listen host publishes, else `None` (both slots keep
/// their empty/false defaults). A host role whose endpoint failed setup has
/// degraded to single-player and publishes nothing. `probe_lan` runs only when
/// hosting.
fn published_host_address(
    role: &netcode::NetRole,
    endpoint_is_host: bool,
    probe_lan: impl FnOnce() -> Option<IpAddr>,
) -> Option<SocketAddr> {
    match role {
        netcode::NetRole::Host { port } if endpoint_is_host => {
            Some(netcode::dialable_host_address(*port, probe_lan()))
        }
        _ => None,
    }
}

/// Preserve the local seat/carry ledger when session identity entropy fails.
/// The returned error tells the caller to disable networking before this
/// table's sentinel identity could reach a peer.
fn authority_seat_table_or_local_only<T, E>(
    net_endpoint: &mut Option<T>,
    result: std::result::Result<netcode::SeatTable, E>,
) -> (netcode::SeatTable, Option<E>) {
    match result {
        Ok(seats) => (seats, None),
        Err(err) => {
            *net_endpoint = None;
            (netcode::SeatTable::local_only(), Some(err))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_address_is_published_only_by_a_live_host() {
        let lan = || Some(std::net::IpAddr::from([10, 0, 0, 7]));
        assert_eq!(
            published_host_address(&netcode::NetRole::Host { port: 27015 }, true, lan),
            Some("10.0.0.7:27015".parse().unwrap())
        );
        // A failed bind (or identity setup) left no host endpoint.
        assert_eq!(
            published_host_address(&netcode::NetRole::Host { port: 27015 }, false, lan),
            None
        );
        let no_probe = || -> Option<std::net::IpAddr> { panic!("probed outside host role") };
        assert_eq!(
            published_host_address(&netcode::NetRole::SinglePlayer, false, no_probe),
            None
        );
        let connect = netcode::NetRole::Connect {
            addr: "127.0.0.1:27015".parse().unwrap(),
        };
        assert_eq!(published_host_address(&connect, false, no_probe), None);
    }

    // Regression: session-id entropy failure used to abort engine boot.
    #[test]
    fn session_identity_failure_keeps_local_carry_ledger_but_is_not_network_safe() {
        let mut net_endpoint = Some(());
        let (seats, identity_error) = authority_seat_table_or_local_only(
            &mut net_endpoint,
            std::result::Result::<netcode::SeatTable, _>::Err("entropy unavailable"),
        );

        assert_eq!(identity_error, Some("entropy unavailable"));
        assert_eq!(net_endpoint, None, "failed identity disables the endpoint");
        assert_eq!(
            seats.session_id_for_test(),
            postretro_net::wire::SessionId([0; 16])
        );
        assert_eq!(
            seats.roster_entries_for_test(),
            vec![postretro_net::wire::RosterEntry {
                seat: 0,
                connected: true,
                name: String::new(),
                status: 0,
            }],
            "single-player carry still owns local seat zero"
        );
    }

    /// Endpoint construction has one production caller: boot flags and
    /// in-app entry both reach it through [`enter_role`].
    #[test]
    fn enter_role_is_the_only_production_endpoint_builder() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut callers = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let relative = path.strip_prefix(&root).unwrap().to_string_lossy().to_string();
                // Test-only modules: their files, fixtures and test dirs.
                if relative.contains("test") || relative.contains("fixture") {
                    continue;
                }
                if path.extension().is_none_or(|ext| ext != "rs") {
                    continue;
                }
                let source = std::fs::read_to_string(&path).unwrap();
                let production = source.split("#[cfg(test)]").next().unwrap();
                if production.contains("from_role(") {
                    callers.push(relative);
                }
            }
        }
        assert_eq!(callers, vec!["session/net_entry.rs".to_string()]);
    }
}
