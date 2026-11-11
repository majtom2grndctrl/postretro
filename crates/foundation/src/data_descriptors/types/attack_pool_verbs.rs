// The attack pool member verbs a `getMapEntities("attackPool")` handle emits.
// See: context/lib/scripting.md §12 (Entity addressing)

/// `p.start()`: start the pool, or re-assign its tag holders while it runs.
pub const START_ATTACK_POOL: &str = "startAttackPool";
/// `p.stop()`: return its members to the default pool.
pub const STOP_ATTACK_POOL: &str = "stopAttackPool";

/// Every attack-pool member primitive. Each addresses one pool by member id:
/// no subject token or group carries them, and neither takes `args`.
pub const ATTACK_POOL_PRIMITIVES: [&str; 2] = [START_ATTACK_POOL, STOP_ATTACK_POOL];

pub fn is_attack_pool_primitive(primitive: &str) -> bool {
    ATTACK_POOL_PRIMITIVES.contains(&primitive)
}
