// Public compiler helpers used by workspace tools and cross-crate bake tests.
// The compiler pipeline itself remains owned by the `prl-build` binary target.

pub mod bc5;
/// BC6H block decode, so cross-bake tests read the colour tiles a build emits.
pub mod bc6h;
pub mod cache;
/// Only the Quake angle and point-origin converters, so placement tests in
/// other crates build entity poses through the compiler's own conversion points.
pub mod format {
    pub mod quake_map {
        pub mod angles;
        pub mod entity_origin;
    }
}
pub mod geometry;
pub mod geometry_utils;
pub mod map_data;
pub mod map_format;
pub mod navmesh_bake;
pub mod partition;
pub mod texture_mips;
