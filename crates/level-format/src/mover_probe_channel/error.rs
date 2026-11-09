// Named MoverProbeChannel (id 57) refusals: the decoder's structural checks
// and the loader's checks against ids 34, 35, 43, 45 and 59.
// See: context/lib/build_pipeline.md §PRL section IDs

use thiserror::Error;

use super::{MoverChannelBlock, MoverLightNamespace};

/// Every way id 57 refuses a level. Each message names the section.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum MoverProbeChannelError {
    #[error("MoverProbeChannelTruncated: {what} needs {needed} byte(s), {remaining} remain")]
    Truncated {
        what: &'static str,
        needed: usize,
        remaining: usize,
    },
    #[error("MoverProbeChannelSizeOverflow: {what} overflows usize")]
    SizeOverflow { what: &'static str },
    #[error(
        "MoverProbeChannelEpochMismatch: epoch {found}, expected {expected}; re-bake the level"
    )]
    EpochMismatch { found: u8, expected: u8 },
    #[error(
        "MoverProbeChannelEmpty: no direct, bounce or depth entry; the section is never written empty"
    )]
    Empty,
    #[error("MoverProbeChannelAffinityFactor: affinity factor {found}, expected {expected}")]
    AffinityFactor { found: u8, expected: u8 },
    #[error(
        "MoverProbeChannelGridMismatch: affinity grid {found:?}, but the DirectShVolume (id 35) and OctahedralShVolume (id 34) probe grid {grid:?} needs {expected:?}"
    )]
    GridMismatch {
        found: [u32; 3],
        grid: [u32; 3],
        expected: [u32; 3],
    },
    #[error(
        "MoverProbeChannelTileGeometry: OctahedralShVolume (id 34) tiles are {dimension} + border {border}; this section's tiles need {expected_dimension} + border {expected_border}"
    )]
    TileGeometry {
        dimension: u32,
        border: u32,
        expected_dimension: u32,
        expected_border: u32,
    },
    #[error(
        "MoverProbeChannelValidityMismatch: cell {cell} valid-probe mask {found:#018x}, OctahedralShVolume (id 34) validity {expected:#018x}"
    )]
    ValidityMismatch {
        cell: usize,
        found: u64,
        expected: u64,
    },
    #[error("MoverProbeChannelPlacement: {block} placement byte {value} is unknown")]
    UnknownPlacement { block: MoverChannelBlock, value: u8 },
    #[error(
        "MoverProbeChannelStreamedBlockWithoutEntries: the {block} payload is placed with id 50 but the {block} block has no entry"
    )]
    StreamedBlockWithoutEntries { block: MoverChannelBlock },
    #[error(
        "MoverProbeChannelStreamedPayloadUnresolved: the {block} payload streams with ClusterShPayloads (id 50), but this load holds no id-50 source for it"
    )]
    StreamedPayloadUnresolved { block: MoverChannelBlock },
    #[error(
        "MoverProbeChannelBorrowedFamilyAbsent: direct record {record} borrows its term from {family}, which this PRL does not hold"
    )]
    BorrowedFamilyAbsent { record: usize, family: &'static str },
    #[error(
        "MoverProbeChannelStreamedPayloadMismatch: the {block} payload of affinity cell {cell} as placed with ClusterShPayloads (id 50) {detail}"
    )]
    StreamedPayloadMismatch {
        block: MoverChannelBlock,
        cell: u32,
        detail: String,
    },
    #[error("MoverProbeChannelStreamedRows: an id-50 row block {detail}")]
    StreamedRows { detail: String },
    #[error(
        "MoverProbeChannelCsrOffsets: {block} offsets[{index}] is {offset} after {previous}; offsets start at 0 and never decrease"
    )]
    CsrOffsets {
        block: &'static str,
        index: usize,
        offset: u32,
        previous: u32,
    },
    #[error(
        "MoverProbeChannelReservedBits: {what} of entry or record {index} is {value:#x}, expected 0"
    )]
    ReservedBits {
        what: &'static str,
        index: usize,
        value: u32,
    },
    #[error(
        "MoverProbeChannelLightNamespace: direct record {record} names namespace {value}, which is unknown"
    )]
    UnknownLightNamespace { record: usize, value: u8 },
    #[error(
        "MoverProbeChannelOwnTileRequired: direct record {record} names a {namespace:?} light without its own tile; no other section holds that light's term"
    )]
    OwnTileRequired {
        record: usize,
        namespace: MoverLightNamespace,
    },
    #[error(
        "MoverProbeChannelEntryOrder: {block} entry {entry} in cell {cell} does not follow its predecessor; entries ascend with no repeats"
    )]
    EntryOrder {
        block: &'static str,
        cell: usize,
        entry: usize,
    },
    #[error(
        "MoverProbeChannelProbeMaskEmpty: {block} entry {entry} in cell {cell} has an empty probe mask"
    )]
    ProbeMaskEmpty {
        block: &'static str,
        cell: usize,
        entry: usize,
    },
    #[error(
        "MoverProbeChannelProbeMaskNotValid: {block} entry {entry} in cell {cell} has probe mask {mask:#018x}, which is not a subset of the valid probes {valid:#018x}"
    )]
    ProbeMaskNotValid {
        block: &'static str,
        cell: usize,
        entry: usize,
        mask: u64,
        valid: u64,
    },
    #[error(
        "MoverProbeChannelProbeMaskNotTight: direct record {record} in cell {cell} masks local probe {probe}, where every mover stores 255 at both termini; a record's mask is exactly its movers' q < 255 probes"
    )]
    ProbeMaskNotTight {
        cell: usize,
        record: usize,
        probe: u32,
    },
    #[error(
        "MoverProbeChannelRecordWithoutMovers: direct record {record} in cell {cell} has no mover entry"
    )]
    RecordWithoutMovers { cell: usize, record: usize },
    #[error(
        "MoverProbeChannelMoverOrder: mover entry {entry} of direct record {record} in cell {cell} does not follow its predecessor; a record's movers ascend by id with no repeats"
    )]
    MoverOrder {
        cell: usize,
        record: usize,
        entry: usize,
    },
    #[error(
        "MoverProbeChannelMoverGatesNothing: mover entry {entry} (mover {mover_id}) of direct record {record} in cell {cell} stores 255 at every masked probe"
    )]
    MoverGatesNothing {
        cell: usize,
        record: usize,
        entry: usize,
        mover_id: u32,
    },
    #[error(
        "MoverProbeChannelCellOrder: listed cell {listed} is affinity cell {cell} after {previous}; listed cells ascend with no repeats"
    )]
    CellOrder {
        listed: usize,
        cell: u32,
        previous: u32,
    },
    #[error(
        "MoverProbeChannelCellOutOfRange: listed cell {listed} is affinity cell {cell}, outside the {cells}-cell affinity grid"
    )]
    CellOutOfRange {
        listed: usize,
        cell: u32,
        cells: usize,
    },
    #[error(
        "MoverProbeChannelCellWithoutEntries: affinity cell {cell} is listed but holds no direct record or bounce entry"
    )]
    CellWithoutEntries { cell: usize },
    #[error(
        "MoverProbeChannelDepthRecordOrder: depth record {record} does not follow its predecessor; records ascend by (mover, probe) with no repeats"
    )]
    DepthRecordOrder { record: usize },
    #[error(
        "MoverProbeChannelDepthRecordStoresNothing: depth record {record} stores neither terminus; a probe whose depth the mover changes at neither terminus carries no record"
    )]
    DepthRecordStoresNothing { record: usize },
    #[error(
        "MoverProbeChannelRampMover: ramp record {record} names mover {found}, expected {expected}; ramp records cover exactly the movers with entries, ascending"
    )]
    RampMover {
        record: usize,
        found: u32,
        expected: u32,
    },
    #[error(
        "MoverProbeChannelRampRange: mover {mover_id} ramp ({start}, {end}) is not finite with 0 <= start < end <= 1"
    )]
    RampRange { mover_id: u32, start: f32, end: f32 },
    #[error("MoverProbeChannelTrailingBytes: {count} byte(s) follow the ramp records")]
    TrailingBytes { count: usize },
    #[error(
        "MoverProbeChannelUnresolvedMover: mover {mover_id} is not in KinematicGeometry (id 43)"
    )]
    UnresolvedMover { mover_id: u32 },
    #[error(
        "MoverProbeChannelLightOutOfNamespace: direct record {record} names {namespace:?} light {light_index}, {detail}"
    )]
    LightOutOfNamespace {
        record: usize,
        namespace: MoverLightNamespace,
        light_index: u32,
        detail: String,
    },
    #[error(
        "MoverProbeChannelDepthProbeOutOfRange: depth record {record} names probe {probe}, outside the {lattice}-probe lattice"
    )]
    DepthProbeOutOfRange {
        record: usize,
        probe: u32,
        lattice: u64,
    },
    #[error(
        "MoverProbeChannelDepthProbeNotValid: depth record {record} names invalid probe {probe}"
    )]
    DepthProbeNotValid { record: usize, probe: u32 },
    #[error(
        "MoverProbeChannelTileRange: {id59} ProbeDepthTiles (id 59) tile(s) plus {live} live tile(s) pass the {max}-tile reference range"
    )]
    TileRange { id59: usize, live: usize, max: u32 },
    #[error("MoverProbeChannelStorageCeiling: {message}")]
    StorageCeiling { message: String },
}
