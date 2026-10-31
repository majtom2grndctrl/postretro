//! Scope raw-fill residency to one bake across the production consumer workers.
use super::*;
use crate::shadowmask_bake::RawFillProbe;

struct RawBake {
    id22: Vec<u8>,
    id42: Vec<u8>,
    peak: usize,
    cache_write_live: Option<usize>,
    layer_bytes: usize,
    carry_bytes: usize,
    layers: u32,
}

fn bake_residency(cache: &StageCache, cut: bool) -> RawBake {
    let lights = [over([12.0, 20.0, 12.0], 1000.0, 5.0)];
    let mut geo = if cut {
        horizontal_quads(&[FLOOR])
    } else {
        horizontal_quads(&[
            (0.0, 0.0, 12.0, 0.0),
            (24.0, 0.0, 12.0, 0.0),
            (48.0, 0.0, 12.0, 0.0),
        ])
    };
    let static_lights = StaticBakedLights::from_lights(&lights);
    let prepared = lightmap_bake::prepare_atlas_within(
        &mut geo,
        &static_lights,
        0.25,
        &[],
        lightmap_bake::BlockOrdering::by_cell_id(2),
        CUT_POOL_EDGE,
        &BakeControl::unrestricted(),
    )
    .unwrap();
    assert_eq!(prepared.cut_parents.is_empty(), !cut);
    assert!(prepared.layer_count >= 3);
    let layers = prepared.layer_count;
    let layer_bytes = prepared.atlas_width as usize * prepared.atlas_height as usize * 4;
    let carry_bytes = shadowmask_bake::maximum_cut_carry_bytes(&prepared) as usize;
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let probe = RawFillProbe::default();
    let window = PartitionWindow {
        size: 4,
        raw_fill_probe: Some(probe.clone()),
        ..PartitionWindow::default()
    };
    let control = BakeControl::new(
        Arc::new(Governor::new(4, false)),
        &StageProgress::indeterminate(),
    );
    // The stage opens and closes fills on whichever worker consumes the ordered
    // items. The probe travels with their buffers instead of observing TLS.
    let (out, cache_write_live) = ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .unwrap()
        .install(|| {
            RAW_FILL_AT_LIGHTMAP_CACHE_WRITE.with(|sample| sample.set(None));
            let out = bake_fused_windowed(
                &test_args(),
                Some(cache),
                &control,
                &BakeControl::unrestricted(),
                &mut geo,
                &static_lights,
                &AlphaLightsNs::from_lights(&lights),
                None,
                &[],
                &crate::texel_footprint::NO_EMBEDDED_EMITTERS,
                &bvh,
                &prims,
                &config(false),
                prepared,
                &window,
            )
            .unwrap();
            (
                out,
                RAW_FILL_AT_LIGHTMAP_CACHE_WRITE.with(std::cell::Cell::take),
            )
        });
    assert_eq!(
        probe.live(),
        0,
        "all buffers release before the stage returns"
    );
    assert_eq!(
        probe.closed_layers(),
        layers as usize,
        "including the first/last layer exactly once"
    );
    RawBake {
        id22: out.lightmap.section.to_bytes(),
        id42: out.shadowmask.unwrap().to_bytes(),
        peak: probe.peak(),
        cache_write_live,
        layer_bytes,
        carry_bytes,
        layers,
    }
}

fn remove_shadowmask_memo(dir: &std::path::Path, id42: &[u8]) -> Vec<u8> {
    let entry = std::fs::read_dir(dir)
        .unwrap()
        .find_map(|entry| {
            let path = entry.unwrap().path();
            let bytes = std::fs::read(&path).unwrap();
            bytes.ends_with(id42).then_some((path, bytes))
        })
        .expect("shadowmask memo carries the section and exact report");
    std::fs::remove_file(entry.0).unwrap();
    entry.1
}

#[test]
fn raw_fill_is_one_layer_on_compose_and_lightmap_hit_shadowmask_miss() {
    for cut in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let cache = StageCache::new(dir.path()).unwrap();
        let cold = bake_residency(&cache, cut);
        assert!(cold.layers >= 3);
        let bound = cold.layer_bytes + cold.carry_bytes;
        assert!(
            cold.peak <= bound,
            "cut={cut}: peak {} exceeds {bound}",
            cold.peak
        );
        if !cut {
            assert_eq!(
                cold.peak, cold.layer_bytes,
                "future resident partitions never allocate another raw layer"
            );
            assert_eq!(cold.carry_bytes, 0);
        }
        let sample = cold
            .cache_write_live
            .expect("cold id-22 streamed write samples the fill");
        assert!(
            sample > 0 && sample <= bound,
            "last layer and only its unfinished cut blocks remain at id-22 write"
        );

        let cold_memo = remove_shadowmask_memo(dir.path(), &cold.id42);
        cache.clear_test_accesses();
        let reuse = bake_residency(&cache, cut);
        assert_eq!(cache.test_access("lightmap_section").read_hits, 1);
        assert_eq!(reuse.id22, cold.id22);
        assert_eq!(reuse.id42, cold.id42);
        assert!(
            reuse.peak <= bound,
            "reuse retains the same one-layer/carry bound"
        );
        if !cut {
            assert_eq!(reuse.peak, cold.layer_bytes);
        }
        assert_eq!(
            reuse.cache_write_live, None,
            "a lightmap hit writes no id-22 entry"
        );
        let reuse_memo = remove_shadowmask_memo(dir.path(), &reuse.id42);
        assert_eq!(
            reuse_memo, cold_memo,
            "reuse preserves owners and the drop report, including cache header"
        );
    }
}
