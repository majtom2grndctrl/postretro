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

/// Why a role's endpoint could not be built. The machine stays single-player;
/// [`enter_role`] has logged the error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntryFailure {
    /// Socket bind or transport construction failed.
    Endpoint,
    /// Session-id entropy was unavailable for a hosted run.
    Identity,
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
/// role-bound value. The endpoint takes `password` before its first poll: a
/// host challenges every joiner for a non-empty one, and a client answers
/// with it. A failed endpoint or identity leaves the machine single-player. A machine that already holds a seat ledger and enters a
/// hosted run mints a fresh session identity and seat counter in place from
/// `mint`, keeping seat 0's carried state; if that mint fails, the ledger is
/// untouched.
pub(crate) fn enter_role(
    net_role: &netcode::NetRole,
    claim: Option<([u8; 16], &str)>,
    scripting: &mut ScriptingCore,
    seat_table: &mut Option<netcode::SeatTable>,
    mint: netcode::SessionIdSource,
    probe_lan: impl FnOnce() -> Option<IpAddr>,
    password: &str,
) -> RoleEntry {
    let client_user_data = claim
        .map(|(player_id, display_name)| ConnectClaim {
            player_id: PlayerClaimId(player_id),
            display_name: display_name.to_owned(),
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
            failure = Some(EntryFailure::Endpoint);
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
            failure.get_or_insert(EntryFailure::Identity);
        }
    }
    // The endpoint keeps only the key derived from the password.
    if let Some(endpoint) = endpoint.as_mut() {
        endpoint.set_session_password(password);
    }
    // Identity setup above can still have demoted the endpoint to
    // single-player, so the role reads it after. A listen host publishes the
    // address players should dial, for the frontend to show.
    let role = SessionRole::of(endpoint.as_ref());
    let published = published_host_address(net_role, role == SessionRole::Host, probe_lan);
    if let Some(address) = published {
        log::info!("[Net] players join with --connect {address}");
    }
    let mut state = RoleBoundState::derive(role, published);
    state.password_protected = state.hosting && !password.is_empty();
    let registry = scripting.script_ctx.registry.clone();
    let released = apply_role(scripting, seat_table, &mut registry.borrow_mut(), &state);
    RoleEntry {
        endpoint,
        failure,
        released,
    }
}

/// The session facts a boot publishes, as in-app entry does: the remembered
/// address seeds `session.joinAddress`; `--password` seeds the password
/// slots no boot endpoint consumed (see [`flag_seeded_slots`]); a `--connect` endpoint reads
/// `connecting`; a failed `--host` sets `hostFailed`, a malformed `--connect`
/// sets `badAddress`, and a `--connect` whose endpoint failed sets
/// `connectFailed`. `parse_error` names the flag that failed, if any.
pub(crate) fn publish_boot_facts(
    script_ctx: &postretro_entities::ScriptCtx,
    parse_error: Option<&netcode::NetArgError>,
    net_role: &netcode::NetRole,
    entry: &RoleEntry,
    remembered_address: Option<&str>,
    cli_password: Option<&str>,
) {
    use crate::startup::lifecycle::session_entry::{
        CONNECTION_SLOT, ConnectionPhase, END_CAUSE_SLOT, EndCause, JOIN_ADDRESS_SLOT,
    };
    use postretro_entities::slot_table::SlotValue;
    let write = |slot: &str, value: SlotValue| {
        if let Err(err) =
            postretro_scripting_core::store_bridge::write_store_slot(script_ctx, slot, value)
        {
            log::warn!("[Net] failed to write `{slot}`: {err}");
        }
    };
    if let Some(address) = remembered_address {
        write(JOIN_ADDRESS_SLOT, SlotValue::String(address.to_string()));
    }
    if let Some(password) = cli_password {
        let live_host = matches!(&entry.endpoint, Some(netcode::NetEndpoint::Host { .. }));
        for slot in flag_seeded_slots(net_role, live_host) {
            // The error is never logged: a validation message could quote
            // the value, and the password never reaches a log.
            if postretro_scripting_core::store_bridge::write_store_slot(
                script_ctx,
                slot,
                SlotValue::String(password.to_string()),
            )
            .is_err()
            {
                log::warn!("[Net] failed to seed `{slot}` from --password");
            }
        }
    }
    let end_cause = match (
        parse_error.and_then(netcode::NetArgError::failed_role_flag),
        net_role,
    ) {
        (Some(crate::startup::cli_flags::CONNECT_FLAG), _) => Some(EndCause::BadAddress),
        (Some(_), _) => Some(EndCause::HostFailed),
        (None, netcode::NetRole::Host { .. }) if entry.failure.is_some() => {
            Some(EndCause::HostFailed)
        }
        (None, netcode::NetRole::Connect { .. }) if entry.failure.is_some() => {
            Some(EndCause::ConnectFailed)
        }
        _ => None,
    };
    if let Some(cause) = end_cause {
        write(END_CAUSE_SLOT, SlotValue::Enum(cause.as_str().to_string()));
    }
    if matches!(&entry.endpoint, Some(netcode::NetEndpoint::Client { .. })) {
        write(
            CONNECTION_SLOT,
            SlotValue::Enum(ConnectionPhase::Connecting.as_str().to_string()),
        );
    }
}

/// The password slots `--password` seeds at boot: the ones a later in-app
/// press uses that no boot endpoint consumed. A live `--host` consumed it as
/// hosting does; a failed one keeps it for an in-app retry. `--connect`
/// seeds only the join slot, which admission consumes as an in-app join's
/// does, so the password a player joined with never becomes the one they
/// host with. With no role flag, both slots take it.
fn flag_seeded_slots(net_role: &netcode::NetRole, live_host: bool) -> Vec<&'static str> {
    use crate::startup::lifecycle::session_entry::{HOST_PASSWORD_SLOT, JOIN_PASSWORD_SLOT};
    match net_role {
        netcode::NetRole::SinglePlayer => vec![HOST_PASSWORD_SLOT, JOIN_PASSWORD_SLOT],
        netcode::NetRole::Host { .. } if live_host => Vec::new(),
        netcode::NetRole::Host { .. } => vec![HOST_PASSWORD_SLOT],
        netcode::NetRole::Connect { .. } => vec![JOIN_PASSWORD_SLOT],
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

    #[test]
    fn password_flag_seeds_only_the_slots_no_boot_endpoint_consumed() {
        use crate::startup::lifecycle::session_entry::{HOST_PASSWORD_SLOT, JOIN_PASSWORD_SLOT};
        let host = netcode::NetRole::Host { port: 27015 };
        let connect = netcode::NetRole::Connect {
            addr: "127.0.0.1:27015".parse().unwrap(),
        };
        assert_eq!(
            flag_seeded_slots(&netcode::NetRole::SinglePlayer, false),
            vec![HOST_PASSWORD_SLOT, JOIN_PASSWORD_SLOT]
        );
        assert!(
            flag_seeded_slots(&host, true).is_empty(),
            "hosting consumed it"
        );
        assert_eq!(
            flag_seeded_slots(&host, false),
            vec![HOST_PASSWORD_SLOT],
            "a failed host keeps it for a retry"
        );
        assert_eq!(flag_seeded_slots(&connect, false), vec![JOIN_PASSWORD_SLOT]);
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

    /// AC U7: the networking contract describes gate 1 as the id comparison
    /// it is under `Unsecure`, admission as an access decision once a
    /// password can gate it, and runtime entry as built.
    #[test]
    fn networking_contract_drops_the_stale_entry_and_handshake_claims() {
        let doc = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../context/lib/networking.md"),
        )
        .unwrap();
        for stale in [
            "encrypted netcode handshake",
            "at the encryption layer",
            "not a trust decision",
            "needs a relaunch",
            "Runtime entry (decided, not yet built)",
            "Session password (decided, not yet built)",
        ] {
            assert!(!doc.contains(stale), "networking.md still says `{stale}`");
        }
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
                let relative = path
                    .strip_prefix(&root)
                    .unwrap()
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
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
