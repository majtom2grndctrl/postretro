// Engine-side netcode glue (M15 Phase 1). The ONLY engine code that touches the
// `EntityRegistry` on behalf of replication: `postretro-net` emits typed
// snapshots and never mutates the registry (entity_model.md §6). This module
// owns role selection, the optional endpoint held by `App`, the
// `NetworkId <-> EntityId` maps, and the two game-logic-owned steps (host
// `serialize`, client `apply`) plus the `WireTransform <-> Transform` and
// `ComponentKind -> u16` conversions.
//
// Phase 1 is "ugly-but-connected": the client is a pure viewer of
// host-authoritative state. No prediction, no client->server gameplay input, no
// despawn reconciliation (a `NetworkId` missing from a later snapshot is left
// untouched — Phase 2 owns remove-missing).

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use glam::{Quat, Vec3};

use postretro_net::transport::{NetClient, NetServer};
use postretro_net::wire::{ComponentPayload, NetworkId, Snapshot, WireTransform};

use crate::scripting::registry::{
    ComponentKind, ComponentValue, EntityId, EntityRegistry, Transform,
};

/// Default listen port for `--host` when no port is supplied.
pub(crate) const DEFAULT_HOST_PORT: u16 = 27015;

/// Max clients a listen server accepts. Phase 1 co-op bar is "ugly-but-connected"
/// loopback; a small ceiling keeps the netcode transport allocation modest.
const MAX_CLIENTS: usize = 8;

/// Network role selected at startup from CLI args.
///
/// Default is single-player (net inert — no endpoint is constructed). `--host
/// [port]` opens a listen server; `--connect <ip:port>` opens a client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NetRole {
    /// No networking. The endpoint is `None`; serialize/apply never run.
    SinglePlayer,
    /// Listen server bound to `port`.
    Host { port: u16 },
    /// Client connecting to `addr`.
    Connect { addr: SocketAddr },
}

/// Parsed net configuration. Today this is just the role; kept as a struct so
/// future net CLI knobs (tick rate override, snapshot rate) extend it without
/// rippling the `main.rs` call site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NetConfig {
    pub(crate) role: NetRole,
}

/// Error parsing the net CLI flags. Carries an operator-facing message; `main.rs`
/// logs it and falls back to single-player rather than aborting boot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NetArgError(pub(crate) String);

impl std::fmt::Display for NetArgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Parse the net role from the full `argv` slice (including `argv[0]`).
///
/// Recognized flags, scanned independently of the positional PRL-map path (which
/// the existing `resolve_map_path` handling owns — this parser never consumes it):
/// - `--host [port]` — listen server; bare `--host` uses [`DEFAULT_HOST_PORT`].
/// - `--connect <ip:port>` — client; `<ip:port>` is required.
///
/// Absent both flags, the role is [`NetRole::SinglePlayer`]. `--host` and
/// `--connect` are mutually exclusive — supplying both is an error.
pub(crate) fn parse_net_config(args: &[String]) -> Result<NetConfig, NetArgError> {
    let mut role: Option<NetRole> = None;

    let mut iter = args.iter().skip(1).peekable();
    while let Some(arg) = iter.next() {
        // `--host` with an optional inline (`--host=PORT`) or following port.
        let host_inline = arg.strip_prefix("--host=");
        if arg == "--host" || host_inline.is_some() {
            if role.is_some() {
                return Err(NetArgError(
                    "--host and --connect are mutually exclusive".into(),
                ));
            }
            let port = if let Some(value) = host_inline {
                parse_port(value)?
            } else if let Some(value) = iter.next_if(|v| !v.is_empty() && !v.starts_with("--")) {
                parse_port(value)?
            } else {
                DEFAULT_HOST_PORT
            };
            role = Some(NetRole::Host { port });
            continue;
        }

        // `--connect <ip:port>` with optional inline (`--connect=ip:port`).
        let connect_inline = arg.strip_prefix("--connect=");
        if arg == "--connect" || connect_inline.is_some() {
            if role.is_some() {
                return Err(NetArgError(
                    "--host and --connect are mutually exclusive".into(),
                ));
            }
            let value = if let Some(value) = connect_inline {
                value.to_string()
            } else {
                iter.next_if(|v| !v.is_empty() && !v.starts_with("--"))
                    .cloned()
                    .ok_or_else(|| NetArgError("--connect requires an <ip:port> address".into()))?
            };
            let addr: SocketAddr = value
                .parse()
                .map_err(|_| NetArgError(format!("invalid --connect address: {value}")))?;
            role = Some(NetRole::Connect { addr });
            continue;
        }
    }

    Ok(NetConfig {
        role: role.unwrap_or(NetRole::SinglePlayer),
    })
}

fn parse_port(value: &str) -> Result<u16, NetArgError> {
    value
        .parse::<u16>()
        .map_err(|_| NetArgError(format!("invalid --host port: {value}")))
}

/// The active network endpoint held by `App`. `None` for single-player; a
/// `Host`/`Client` variant once the role's transport is constructed.
///
/// Construction can fail (socket bind, transport init); failures are logged at
/// the call site and degrade to single-player (the field stays `None`) so a
/// netcode setup error never blocks boot.
pub(crate) enum NetEndpoint {
    /// Listen server plus the host-side `EntityId -> NetworkId` allocator. The
    /// `NetServer` is boxed: it is by far the largest endpoint payload (the renet
    /// connection layer + netcode transport), so an unboxed variant would inflate
    /// every `NetEndpoint` to its size (clippy::large_enum_variant). Boxing keeps
    /// the enum compact; the endpoint is a per-process singleton, so the extra
    /// indirection is paid once.
    Host {
        server: Box<NetServer>,
        allocator: NetworkIdAllocator,
        /// Monotonic server tick stamp written into each snapshot.
        tick: u32,
    },
    /// Client plus the `NetworkId -> EntityId` map for applied snapshots. The
    /// `NetClient` is boxed for the same reason the `Host` server is.
    Client {
        client: Box<NetClient>,
        map: HashMap<NetworkId, EntityId>,
    },
}

impl NetEndpoint {
    /// Construct the endpoint for `role`, or `Ok(None)` for single-player.
    ///
    /// The netcode clock origin is `SystemTime::now()` since the unix epoch
    /// (`NetServer::new`/`NetClient::new` contract). Returns the transport error
    /// for the caller to log and fall back to single-player.
    pub(crate) fn from_role(role: &NetRole) -> Result<Option<NetEndpoint>, String> {
        match role {
            NetRole::SinglePlayer => Ok(None),
            NetRole::Host { port } => {
                let bind_addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, *port));
                let socket = UdpSocket::bind(bind_addr)
                    .map_err(|e| format!("host bind {bind_addr} failed: {e}"))?;
                let public_addr = socket
                    .local_addr()
                    .map_err(|e| format!("host local_addr failed: {e}"))?;
                let server = NetServer::new(socket, public_addr, MAX_CLIENTS, now())
                    .map_err(|e| format!("host transport init failed: {e}"))?;
                Ok(Some(NetEndpoint::Host {
                    server: Box::new(server),
                    allocator: NetworkIdAllocator::new(),
                    tick: 0,
                }))
            }
            NetRole::Connect { addr } => {
                // Bind an ephemeral local socket on the same address family.
                let bind_addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0));
                let socket =
                    UdpSocket::bind(bind_addr).map_err(|e| format!("client bind failed: {e}"))?;
                // Client id is arbitrary under unsecure auth; use the wall clock
                // so two clients on one host do not collide.
                let client_id = now().as_nanos() as u64;
                let client = NetClient::new(socket, *addr, client_id, now())
                    .map_err(|e| format!("client transport init failed: {e}"))?;
                Ok(Some(NetEndpoint::Client {
                    client: Box::new(client),
                    map: HashMap::new(),
                }))
            }
        }
    }
}

fn now() -> Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
}

/// Host-side monotonic `EntityId -> NetworkId` allocator. Ids are never recycled:
/// each newly-seen `EntityId` gets the next counter value, and the mapping is
/// stable for the entity's lifetime so the client's `NetworkId -> EntityId` map
/// stays coherent across snapshots.
pub(crate) struct NetworkIdAllocator {
    next: u32,
    map: HashMap<EntityId, NetworkId>,
}

impl NetworkIdAllocator {
    pub(crate) fn new() -> Self {
        Self {
            next: 0,
            map: HashMap::new(),
        }
    }

    /// Stamp `id` with its stable `NetworkId`, allocating a fresh one on first
    /// sight. Monotonic counter; never recycled.
    pub(crate) fn stamp(&mut self, id: EntityId) -> NetworkId {
        if let Some(net_id) = self.map.get(&id) {
            return *net_id;
        }
        let net_id = NetworkId(self.next);
        self.next += 1;
        self.map.insert(id, net_id);
        net_id
    }
}

/// Engine-aligned `u16` wire discriminant for a `ComponentKind`, via an
/// exhaustive match (no enum-layout reliance, no `_` arm). A renamed/removed
/// variant is a compile error here, which is the drift guard's whole point:
/// keep this numerically equal to `ComponentPayload::kind()` in `postretro-net`.
pub(crate) fn component_kind_discriminant(kind: ComponentKind) -> u16 {
    match kind {
        ComponentKind::Transform => 0,
        ComponentKind::Light => 1,
        ComponentKind::BillboardEmitter => 2,
        ComponentKind::ParticleState => 3,
        ComponentKind::SpriteVisual => 4,
        ComponentKind::FogVolume => 5,
        ComponentKind::PlayerMovement => 6,
        ComponentKind::Weapon => 7,
        ComponentKind::DescriptorProvenance => 8,
        ComponentKind::Mesh => 9,
        ComponentKind::Health => 10,
        ComponentKind::Agent => 11,
        ComponentKind::Brain => 12,
    }
}

/// Convert an engine `Transform` to its wire mirror. Scale is not replicated
/// (Phase 1 sends only position + rotation). glam `Quat` is `xyzw`, mirrored to
/// the wire's fixed `[x, y, z, w]` order.
pub(crate) fn transform_to_wire(transform: &Transform) -> WireTransform {
    let p = transform.position;
    let q = transform.rotation;
    WireTransform {
        position: [p.x, p.y, p.z],
        rotation: [q.x, q.y, q.z, q.w],
    }
}

/// Inverse of [`transform_to_wire`]. Scale was not sent, so it defaults to ONE
/// (a replicated remote entity has no authored scale on the client). Rotation is
/// rebuilt from the `[x, y, z, w]` wire order via `Quat::from_xyzw`.
pub(crate) fn wire_to_transform(wire: &WireTransform) -> Transform {
    Transform {
        position: Vec3::new(wire.position[0], wire.position[1], wire.position[2]),
        rotation: Quat::from_xyzw(
            wire.rotation[0],
            wire.rotation[1],
            wire.rotation[2],
            wire.rotation[3],
        ),
        scale: Vec3::ONE,
    }
}

/// Convert a wire `ComponentPayload` to the engine `ComponentValue` via an
/// exhaustive match over the payload — Phase 1 binds only `Transform`; adding a
/// payload variant (Phase 2) is a compile error here until the arm is added.
fn payload_to_component_value(payload: &ComponentPayload) -> ComponentValue {
    match payload {
        ComponentPayload::Transform(wire) => ComponentValue::Transform(wire_to_transform(wire)),
    }
}

/// Host serialize step (game-logic-owned). Walk the replicable set — every live
/// entity carrying a `Transform` (the host pawn in Phase 1) — stamp each
/// `EntityId` to its `NetworkId`, convert the `Transform` to its wire mirror, and
/// build the `Snapshot` envelope stamped with `tick`.
///
/// Borrows the registry immutably; never mutates it (the apply step is the only
/// registry-touching path, and only on the client).
pub(crate) fn serialize(
    registry: &EntityRegistry,
    allocator: &mut NetworkIdAllocator,
    tick: u32,
) -> Snapshot {
    let mut entries = Vec::new();
    for (id, value) in registry.iter_with_kind(ComponentKind::Transform) {
        if let ComponentValue::Transform(transform) = value {
            let net_id = allocator.stamp(id);
            let payload = ComponentPayload::Transform(transform_to_wire(transform));
            // Engine-aligned discriminant must equal the wire-side `kind()`. This
            // is the live cross-check of the `ComponentKind -> u16` mapping the
            // drift-guard test pins; a divergence here would mis-tag replication.
            debug_assert_eq!(
                component_kind_discriminant(value.kind()),
                payload.kind(),
                "engine/wire component discriminant diverged"
            );
            entries.push((net_id, payload));
        }
    }
    Snapshot { tick, entries }
}

/// Client apply step (game-logic-owned). For each `(NetworkId, ComponentPayload)`:
/// on first sight of the `NetworkId`, `spawn` with the converted `Transform` and
/// record the mapping; otherwise `set_component_value` on the mapped `EntityId`.
///
/// Phase 1 never despawns: a `NetworkId` absent from a later snapshot is left
/// untouched (remove-missing reconciliation is Phase 2). A stale mapped
/// `EntityId` (registry rejected the write) is re-spawned and the map updated.
pub(crate) fn apply(
    registry: &mut EntityRegistry,
    snapshot: &Snapshot,
    map: &mut HashMap<NetworkId, EntityId>,
) {
    for (net_id, payload) in &snapshot.entries {
        let value = payload_to_component_value(payload);
        match map.get(net_id).copied() {
            Some(existing) => {
                if registry
                    .set_component_value(existing, value.clone())
                    .is_err()
                {
                    // The mapped entity is gone (should not happen in Phase 1,
                    // which never despawns) — re-spawn from this payload so the
                    // remote entity stays visible rather than silently vanishing.
                    let id = spawn_from_value(registry, value);
                    map.insert(*net_id, id);
                }
            }
            None => {
                let id = spawn_from_value(registry, value);
                map.insert(*net_id, id);
            }
        }
    }
}

/// Spawn a fresh entity seeded from a replicated `ComponentValue`. A spawn always
/// installs a `Transform`; if the payload is a `Transform`, spawn directly from
/// it, otherwise spawn at the default pose and set the component.
fn spawn_from_value(registry: &mut EntityRegistry, value: ComponentValue) -> EntityId {
    match value {
        ComponentValue::Transform(transform) => registry.spawn(transform),
        other => {
            let id = registry.spawn(Transform::default());
            // The id was just returned by spawn, so the only failure mode is an
            // unsupported component kind, which cannot occur in Phase 1.
            let _ = registry.set_component_value(id, other);
            id
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Float epsilon for transform round-trips (testing_guide §Floating-point:
    // approximate comparison for computed/converted floats).
    const EPSILON: f32 = 1e-6;

    fn sample_transform() -> Transform {
        Transform {
            position: Vec3::new(1.5, -2.0, 3.25),
            // A non-axis-aligned unit quaternion.
            rotation: Quat::from_xyzw(0.182_574_2, 0.365_148_4, 0.547_722_6, 0.730_296_8)
                .normalize(),
            scale: Vec3::splat(2.0),
        }
    }

    // --- Drift guard: Transform's wire discriminant is pinned to 0, equal to
    // `ComponentKind::Transform as u16`, through the exhaustive mapping. ---
    #[test]
    fn transform_discriminant_pinned_to_zero() {
        assert_eq!(
            component_kind_discriminant(ComponentKind::Transform),
            ComponentKind::Transform as u16
        );
        assert_eq!(component_kind_discriminant(ComponentKind::Transform), 0);
    }

    #[test]
    fn discriminant_mapping_matches_enum_layout() {
        // Every variant's mapped discriminant equals its `as u16` value. This
        // keeps the exhaustive match honest without relying on layout in the
        // production conversion. (Listed explicitly — the match in
        // `component_kind_discriminant` is the compile-time exhaustiveness gate.)
        for kind in [
            ComponentKind::Transform,
            ComponentKind::Light,
            ComponentKind::BillboardEmitter,
            ComponentKind::ParticleState,
            ComponentKind::SpriteVisual,
            ComponentKind::FogVolume,
            ComponentKind::PlayerMovement,
            ComponentKind::Weapon,
            ComponentKind::DescriptorProvenance,
            ComponentKind::Mesh,
            ComponentKind::Health,
            ComponentKind::Agent,
            ComponentKind::Brain,
        ] {
            assert_eq!(component_kind_discriminant(kind), kind as u16);
        }
    }

    // --- Round-trip: Transform -> WireTransform -> ComponentValue::Transform
    // preserves position and rotation in [x, y, z, w] order. ---
    #[test]
    fn transform_wire_round_trip_preserves_position_and_rotation() {
        let original = sample_transform();
        let wire = transform_to_wire(&original);

        // Wire stores position in xyz and rotation in [x, y, z, w] order.
        assert!((wire.position[0] - original.position.x).abs() < EPSILON);
        assert!((wire.position[1] - original.position.y).abs() < EPSILON);
        assert!((wire.position[2] - original.position.z).abs() < EPSILON);
        assert!((wire.rotation[0] - original.rotation.x).abs() < EPSILON);
        assert!((wire.rotation[1] - original.rotation.y).abs() < EPSILON);
        assert!((wire.rotation[2] - original.rotation.z).abs() < EPSILON);
        assert!((wire.rotation[3] - original.rotation.w).abs() < EPSILON);

        let payload = ComponentPayload::Transform(wire);
        let value = payload_to_component_value(&payload);
        let ComponentValue::Transform(rebuilt) = value else {
            panic!("Transform payload must rebuild a Transform component");
        };

        assert!((rebuilt.position - original.position).length() < EPSILON);
        // angle_between is 0 when rotations match.
        assert!(rebuilt.rotation.angle_between(original.rotation) < 1e-4);
        // Scale is not replicated; the rebuilt transform defaults to ONE.
        assert_eq!(rebuilt.scale, Vec3::ONE);
    }

    // --- Apply: spawns on first sight, mutates the same EntityId on second. ---
    #[test]
    fn apply_spawns_on_first_sight_and_mutates_on_second() {
        let mut registry = EntityRegistry::new();
        let mut map: HashMap<NetworkId, EntityId> = HashMap::new();

        let first_pos = Vec3::new(1.0, 2.0, 3.0);
        let snapshot1 = Snapshot {
            tick: 1,
            entries: vec![(
                NetworkId(42),
                ComponentPayload::Transform(transform_to_wire(&Transform {
                    position: first_pos,
                    rotation: Quat::IDENTITY,
                    scale: Vec3::ONE,
                })),
            )],
        };
        apply(&mut registry, &snapshot1, &mut map);

        assert_eq!(map.len(), 1, "first sight records exactly one mapping");
        let spawned = *map.get(&NetworkId(42)).expect("NetworkId(42) mapped");
        let after_first = registry
            .get_component::<Transform>(spawned)
            .expect("spawned entity carries a Transform");
        assert!((after_first.position - first_pos).length() < EPSILON);

        // Second snapshot: same NetworkId, moved position. Must mutate, not respawn.
        let moved_pos = Vec3::new(10.0, 20.0, 30.0);
        let snapshot2 = Snapshot {
            tick: 2,
            entries: vec![(
                NetworkId(42),
                ComponentPayload::Transform(transform_to_wire(&Transform {
                    position: moved_pos,
                    rotation: Quat::IDENTITY,
                    scale: Vec3::ONE,
                })),
            )],
        };
        apply(&mut registry, &snapshot2, &mut map);

        assert_eq!(map.len(), 1, "second snapshot does not add a new mapping");
        let same = *map.get(&NetworkId(42)).expect("mapping unchanged");
        assert_eq!(
            same, spawned,
            "same NetworkId must map to the same EntityId"
        );
        let after_second = registry
            .get_component::<Transform>(same)
            .expect("entity still live");
        assert!(
            (after_second.position - moved_pos).length() < EPSILON,
            "second snapshot moves the existing entity"
        );
    }

    #[test]
    fn apply_phase1_never_despawns_missing_network_ids() {
        let mut registry = EntityRegistry::new();
        let mut map: HashMap<NetworkId, EntityId> = HashMap::new();

        let snapshot1 = Snapshot {
            tick: 1,
            entries: vec![
                (
                    NetworkId(1),
                    ComponentPayload::Transform(transform_to_wire(&Transform::default())),
                ),
                (
                    NetworkId(2),
                    ComponentPayload::Transform(transform_to_wire(&Transform::default())),
                ),
            ],
        };
        apply(&mut registry, &snapshot1, &mut map);
        let id1 = *map.get(&NetworkId(1)).unwrap();
        let id2 = *map.get(&NetworkId(2)).unwrap();

        // Second snapshot omits NetworkId(2). Phase 1 leaves it untouched.
        let snapshot2 = Snapshot {
            tick: 2,
            entries: vec![(
                NetworkId(1),
                ComponentPayload::Transform(transform_to_wire(&Transform::default())),
            )],
        };
        apply(&mut registry, &snapshot2, &mut map);

        assert!(registry.exists(id1), "present id stays live");
        assert!(
            registry.exists(id2),
            "omitted NetworkId is NOT despawned in Phase 1"
        );
        assert!(map.contains_key(&NetworkId(2)), "mapping is retained");
    }

    // --- Serialize: stamps each replicable EntityId to a stable NetworkId. ---
    #[test]
    fn serialize_stamps_transform_entities_with_stable_ids() {
        let mut registry = EntityRegistry::new();
        let a = registry.spawn(Transform {
            position: Vec3::new(1.0, 0.0, 0.0),
            ..Transform::default()
        });
        let _b = registry.spawn(Transform {
            position: Vec3::new(2.0, 0.0, 0.0),
            ..Transform::default()
        });
        let mut allocator = NetworkIdAllocator::new();

        let snap1 = serialize(&registry, &mut allocator, 7);
        assert_eq!(snap1.tick, 7);
        assert_eq!(snap1.entries.len(), 2, "both transform entities replicate");

        // The id stamped for `a` is stable across snapshots.
        let a_net_id = allocator.stamp(a);
        let snap2 = serialize(&registry, &mut allocator, 8);
        let a_in_snap2 = snap2.entries.iter().find(|(net_id, _)| *net_id == a_net_id);
        assert!(
            a_in_snap2.is_some(),
            "the same EntityId stamps to the same NetworkId across snapshots"
        );
    }

    // --- Argv parsing: default / --host / --connect, coexisting with the map path. ---
    fn argv(parts: &[&str]) -> Vec<String> {
        std::iter::once("postretro")
            .chain(parts.iter().copied())
            .map(String::from)
            .collect()
    }

    #[test]
    fn parse_default_is_single_player() {
        let config = parse_net_config(&argv(&[])).unwrap();
        assert_eq!(config.role, NetRole::SinglePlayer);
    }

    #[test]
    fn parse_host_without_port_uses_default() {
        let config = parse_net_config(&argv(&["--host"])).unwrap();
        assert_eq!(
            config.role,
            NetRole::Host {
                port: DEFAULT_HOST_PORT
            }
        );
    }

    #[test]
    fn parse_host_with_port() {
        let config = parse_net_config(&argv(&["--host", "30000"])).unwrap();
        assert_eq!(config.role, NetRole::Host { port: 30000 });
        let inline = parse_net_config(&argv(&["--host=40000"])).unwrap();
        assert_eq!(inline.role, NetRole::Host { port: 40000 });
    }

    #[test]
    fn parse_connect_with_addr() {
        let config = parse_net_config(&argv(&["--connect", "127.0.0.1:27015"])).unwrap();
        assert_eq!(
            config.role,
            NetRole::Connect {
                addr: "127.0.0.1:27015".parse().unwrap()
            }
        );
    }

    #[test]
    fn parse_connect_missing_addr_is_error() {
        assert!(parse_net_config(&argv(&["--connect"])).is_err());
        assert!(parse_net_config(&argv(&["--connect", "not-an-addr"])).is_err());
    }

    #[test]
    fn parse_host_and_connect_are_mutually_exclusive() {
        assert!(parse_net_config(&argv(&["--host", "--connect", "127.0.0.1:1"])).is_err());
    }

    #[test]
    fn net_flags_do_not_clobber_positional_map_path() {
        // The positional PRL-map path coexists with the net flags. `parse_net_config`
        // ignores the positional path entirely, and `resolve_map_path` (the existing
        // handler) must still recover it alongside `--host`/`--connect`.
        let args = argv(&["content/dev/maps/campaign-test.prl", "--host", "30000"]);
        let config = parse_net_config(&args).unwrap();
        assert_eq!(config.role, NetRole::Host { port: 30000 });
        assert_eq!(
            crate::resolve_map_path(&args).as_deref(),
            Some("content/dev/maps/campaign-test.prl"),
            "the positional map path survives the net flags"
        );

        // And with --connect: the positional map path leads (the conventional
        // `cargo run -p postretro -- <map>` ordering), then the net flag.
        let args = argv(&["maps/e1m1.prl", "--connect", "127.0.0.1:27015"]);
        let config = parse_net_config(&args).unwrap();
        assert_eq!(
            config.role,
            NetRole::Connect {
                addr: "127.0.0.1:27015".parse().unwrap()
            }
        );
        assert_eq!(
            crate::resolve_map_path(&args).as_deref(),
            Some("maps/e1m1.prl")
        );
    }
}
