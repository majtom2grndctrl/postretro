// Old weapon-owned path for the damage POD. The type itself lives in the
// VM-free foundation POD module so components can depend on it without pulling
// the weapon subsystem across the future crate boundary.
//
// See: context/lib/entity_model.md §2 · context/research/weapon-model.md §7

pub(crate) use postretro_foundation::DamagePayload;
