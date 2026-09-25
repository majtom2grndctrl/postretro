//! Dev-tools-only read view over per-probe residency, so the SH diagnostics
//! overlay can color markers from the same mirrors the sampler consumes
//! without exposing `ShResidencyState`'s private fields.

use postretro_level_loader::ShStreamBaseMetadata;

use super::ShResidencyState;
use crate::render::sh_diagnostics_residency::classify_probe;

/// Per-probe residency classification for `MarkerMode::Residency`. Mirrors
/// what the renderer's compose/sample pipeline has actually done with the
/// probe, not the bake-time validity alone. See `classify_probe` for the
/// precedence rule between classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::render) enum ProbeResidencyClass {
    /// `sampled_words` entry is nonzero: the shader can sample this probe now.
    Sampleable,
    /// `compose_words` entry is nonzero but `sampled_words` is still zero:
    /// installed on GPU, not yet promoted to the sampled indirection.
    InstalledAwaitingCompose,
    /// Valid probe whose owning cluster (`dense_owner`) is a current target,
    /// with nothing installed for it yet.
    Requested,
    /// Valid probe with no residency activity at all.
    Miss,
    /// Bake-time invalid probe (inside solid / off-grid).
    Invalid,
}

impl ShResidencyState {
    /// Streamed base metadata behind this session: grid geometry and each
    /// probe's bake-time validity/density-level/node-scale — the streamed
    /// counterpart to `ShVolumeResources`'s id-34 mirrors used on a
    /// whole-loaded map.
    pub(in crate::render) fn base_metadata(&self) -> &ShStreamBaseMetadata {
        &self.base_metadata
    }

    /// Classify every dense probe by the renderer's own residency mirrors, in
    /// the same x-fastest order as `base_metadata().probes`. `dense_owner`,
    /// `compose_words`, and `sampled_words` are each sized to
    /// `base.probes.len()` at construction (`from_parts`/`setup::from_manifest`)
    /// and indexed by the same dense-probe id the loader assigns from id-49's
    /// `DenseProbe` resource ranges — one entry per dense probe, matching
    /// `base_metadata().probes`'s order.
    pub(in crate::render) fn probe_residency_classes(&self) -> Vec<ProbeResidencyClass> {
        self.base_metadata
            .probes
            .iter()
            .enumerate()
            .map(|(i, probe)| {
                let owner_is_target = self
                    .dense_owner
                    .get(i)
                    .copied()
                    .flatten()
                    .is_some_and(|owner| self.targets.contains(&owner));
                classify_probe(
                    probe.validity,
                    self.sampled_words.get(i).copied().unwrap_or(0),
                    self.compose_words.get(i).copied().unwrap_or(0),
                    owner_is_target,
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::cluster_directory::{
        ClusterDirectorySection, ClusterRangeRecord, ClusterRangeRole, ClusterRecord,
        ClusterResourceDomain, ClusterResourceRecord,
    };
    use postretro_level_format::sh_volume::OctahedralShProbe;
    use postretro_level_loader::ShStreamSourceMetadata;

    use crate::render::sh_streaming::INDIRECT_BASE_ID;

    /// One 4×4×4 brick (64 probes) owned by a single cluster — the same
    /// minimal shape `sh_streaming::tests::state_with_clusters` uses, so this
    /// exercises the real `from_parts` construction path, not a hand-rolled
    /// stand-in.
    fn minimal_state(validity_overrides: &[(usize, u8)]) -> ShResidencyState {
        let mut probes = vec![
            OctahedralShProbe {
                validity: 1,
                density_level: 0,
                node_scale: 0,
                ..Default::default()
            };
            64
        ];
        for &(index, validity) in validity_overrides {
            probes[index].validity = validity;
        }
        let base = ShStreamBaseMetadata {
            grid_origin: [1.0, 2.0, 3.0],
            cell_size: [0.5, 0.5, 0.5],
            grid_dimensions: [4, 4, 4],
            probe_stride: 8,
            tile_dimension: 4,
            tile_border: 1,
            atlas_dimensions: [8, 8],
            layer_count: 1,
            tiles_per_layer: 1,
            atlas_tiles_per_row: 1,
            irradiance_format: 0,
            probes,
            animation_descriptors: Vec::new(),
            slot_for_map_light: Vec::new(),
        };
        let directory = ClusterDirectorySection {
            runtime_cell_count: 0,
            primitive_limit: 0,
            cell_limit: 0,
            clusters: vec![ClusterRecord {
                bounds_min: [0.0; 3],
                bounds_max: [1.0; 3],
                member_start: 0,
                member_count: 0,
                range_start: 0,
                range_count: 1,
                primitive_count: 0,
                flags: 0,
            }],
            resources: vec![ClusterResourceRecord {
                section_id: INDIRECT_BASE_ID,
                domain: ClusterResourceDomain::DenseProbe,
                dimensions: [4, 4, 4],
            }],
            members: Vec::new(),
            ranges: vec![ClusterRangeRecord {
                resource_index: 0,
                start: 0,
                count: 64,
                owner_cluster_id: 0,
                role: ClusterRangeRole::Owned,
            }],
            seam_portal_ids: Vec::new(),
            cluster_hints: Vec::new(),
        };
        ShResidencyState::from_parts(
            [1; 32],
            &directory,
            &base,
            &ShStreamSourceMetadata::default(),
        )
        .expect("minimal single-cluster 4x4x4 dense grid is valid")
    }

    #[test]
    fn base_metadata_exposes_streamed_grid_geometry() {
        let state = minimal_state(&[]);
        let base = state.base_metadata();
        assert_eq!(base.grid_origin, [1.0, 2.0, 3.0]);
        assert_eq!(base.cell_size, [0.5, 0.5, 0.5]);
        assert_eq!(base.grid_dimensions, [4, 4, 4]);
    }

    #[test]
    fn probe_residency_classes_cover_every_class_in_dense_probe_order() {
        let mut state = minimal_state(&[(3, 0)]);
        // Probe 0: sampleable.
        state.sampled_words[0] = 0xABCD;
        // Probe 1: installed, awaiting compose.
        state.compose_words[1] = 0x1234;
        // Probe 2: requested — its owner cluster (0, the sole cluster in this
        // fixture) is a current target, with nothing installed for it.
        state.targets.insert(0);
        // Probe 3 is bake-time invalid regardless of any mirror state.

        let classes = state.probe_residency_classes();
        assert_eq!(classes.len(), 64);
        assert_eq!(classes[0], ProbeResidencyClass::Sampleable);
        assert_eq!(classes[1], ProbeResidencyClass::InstalledAwaitingCompose);
        assert_eq!(classes[2], ProbeResidencyClass::Requested);
        assert_eq!(classes[3], ProbeResidencyClass::Invalid);
    }

    #[test]
    fn probe_residency_classes_default_to_miss() {
        let state = minimal_state(&[]);
        assert!(
            state
                .probe_residency_classes()
                .iter()
                .all(|&class| class == ProbeResidencyClass::Miss)
        );
    }
}
