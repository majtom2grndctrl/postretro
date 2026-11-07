//! A residency walk's levers: the lead, and each pool's size.
//! See: context/lib/rendering_pipeline.md §4

use postretro_render_cpu::lightmap_pool::{PerPool, PoolKind};

/// The runs to make, in place of the default sweep: see [`parse_pool_layers`].
pub(super) const POOL_LAYERS_ENV: &str = "POSTRETRO_LIGHTMAP_WALK_POOL_LAYERS";

/// `bound` in [`POOL_LAYERS_ENV`]: each pool at its id 58 figure, no margin.
const AT_THE_BOUND: &str = "bound";

#[derive(Debug, Clone, Copy)]
pub(super) struct Levers {
    pub(super) lead_metres: f32,
    /// A pool's first generation in layers, taken as given. `None` sizes that
    /// pool as the runtime does, from the level's id 58.
    pub(super) pool_layers: PerPool<Option<u32>>,
}

/// The shipped levers: the run every lever sweep varies one lever from.
pub(super) const DEFAULT_LEVERS: Levers = Levers {
    lead_metres: 16.0,
    pool_layers: PerPool {
        lightmap: None,
        mask: None,
    },
};

impl Levers {
    /// The pool sizes this run asked for, lightmap then mask. It names the
    /// request, not the result, so a paired run's label is the same on any bake.
    pub(super) fn pools_label(&self) -> String {
        let figure = |kind: PoolKind| {
            self.pool_layers[kind].map_or_else(|| "id58".to_string(), |layers| layers.to_string())
        };
        format!(
            "pools {}/{}",
            figure(PoolKind::Lightmap),
            figure(PoolKind::Mask)
        )
    }
}

/// Each run's pool sizes from a comma-separated list. A run is `LIGHTMAP/MASK`
/// in layers: `34/15`. Either figure may be left out to take that pool's size
/// from id 58, so `30` and `30/` set the lightmap pool alone and `/15` the mask
/// pool alone. `bound` is both pools at their stored `bound`, with no margin.
pub(super) fn parse_pool_layers(
    list: &str,
    bound: PerPool<u32>,
) -> Result<Vec<PerPool<Option<u32>>>, String> {
    list.split(',')
        .map(|run| {
            let run = run.trim();
            if run == AT_THE_BOUND {
                return Ok(bound.map(Some));
            }
            let (lightmap, mask) = run.split_once('/').unwrap_or((run, ""));
            Ok(PerPool {
                lightmap: pool_figure(lightmap)
                    .map_err(|why| format!("{POOL_LAYERS_ENV} run {run:?}: {why}"))?,
                mask: pool_figure(mask)
                    .map_err(|why| format!("{POOL_LAYERS_ENV} run {run:?}: {why}"))?,
            })
        })
        .collect()
}

fn pool_figure(figure: &str) -> Result<Option<u32>, String> {
    let figure = figure.trim();
    if figure.is_empty() {
        return Ok(None);
    }
    match figure.parse() {
        Ok(0) => Err("a pool holds at least one layer".to_string()),
        Ok(layers) => Ok(Some(layers)),
        Err(_) => Err(format!("{figure:?} is not a layer count")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUND: PerPool<u32> = PerPool {
        lightmap: 27,
        mask: 12,
    };

    fn pools(lightmap: Option<u32>, mask: Option<u32>) -> PerPool<Option<u32>> {
        PerPool { lightmap, mask }
    }

    // Task 10's sweeps name each pool apart, and one pool alone.
    #[test]
    fn pool_layers_list_sets_each_pool_apart_and_leaves_the_rest_to_id_58() {
        assert_eq!(
            parse_pool_layers("34/15, 30, 28/, /15, bound", BOUND).unwrap(),
            [
                pools(Some(34), Some(15)),
                pools(Some(30), None),
                pools(Some(28), None),
                pools(None, Some(15)),
                pools(Some(27), Some(12)),
            ]
        );
    }

    #[test]
    fn pool_layers_list_refuses_a_zero_or_unreadable_figure_by_name() {
        for list in ["0/15", "34/0", "many", "34/15/2"] {
            let why = parse_pool_layers(list, BOUND).unwrap_err();
            assert!(why.contains(POOL_LAYERS_ENV), "{why}");
        }
    }

    #[test]
    fn run_label_names_what_each_pool_asked_for() {
        assert_eq!(DEFAULT_LEVERS.pools_label(), "pools id58/id58");
        let levers = Levers {
            pool_layers: pools(Some(30), None),
            ..DEFAULT_LEVERS
        };
        assert_eq!(levers.pools_label(), "pools 30/id58");
    }
}
