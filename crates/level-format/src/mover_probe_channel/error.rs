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
        "MoverProbeChannelProbeMaskNotTight: direct record {record} in cell {cell} masks local probe {probe}, where every mover stores 255 at every pose; a record's mask is exactly its movers' q < 255 probes"
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
        "MoverProbeChannelMoverGatesNothing: mover entry {entry} (mover {mover_id}) of direct record {record} in cell {cell} stores 255 at every masked probe and pose"
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
        "MoverProbeChannelDepthRecordStoresNothing: depth record {record} stores no pose; a probe whose depth the mover changes at no pose carries no record"
    )]
    DepthRecordStoresNothing { record: usize },
    #[error(
        "MoverProbeChannelMoverRecordOrder: mover record {record} does not follow its predecessor; mover records ascend by id with no repeats"
    )]
    MoverRecordOrder { record: usize },
    #[error(
        "MoverProbeChannelMoverRecordMissing: the {block} block names mover {mover_id}, which has no mover record, so none of its poses exists"
    )]
    MoverRecordMissing { block: &'static str, mover_id: u32 },
    #[error(
        "MoverProbeChannelMoverRecordUnused: mover {mover_id} has a mover record but no entry in any block; mover records cover exactly the movers with entries"
    )]
    MoverRecordUnused { mover_id: u32 },
    #[error(
        "MoverProbeChannelRampRange: mover {mover_id} ramp ({start}, {end}) is not finite with 0 <= start < end <= 1"
    )]
    RampRange { mover_id: u32, start: f32, end: f32 },
    #[error(
        "MoverProbeChannelPoseCount: mover {mover_id} stores {count} pose(s); every mover stores at least its two termini"
    )]
    PoseCount { mover_id: u32, count: u32 },
    #[error("MoverProbeChannelKnotNotFinite: mover {mover_id} knot {knot} is {value}, not finite")]
    KnotNotFinite {
        mover_id: u32,
        knot: usize,
        value: f32,
    },
    #[error(
        "MoverProbeChannelKnotOutOfRange: mover {mover_id} knot {knot} is {value}, outside 0 to 1"
    )]
    KnotOutOfRange {
        mover_id: u32,
        knot: usize,
        value: f32,
    },
    #[error(
        "MoverProbeChannelKnotOrder: mover {mover_id} knot {knot} is {value} after {previous}; knots ascend strictly"
    )]
    KnotOrder {
        mover_id: u32,
        knot: usize,
        value: f32,
        previous: f32,
    },
    #[error(
        "MoverProbeChannelKnotEnds: mover {mover_id} knots run from {first} to {last}; they run exactly from 0 to 1, the termini"
    )]
    KnotEnds {
        mover_id: u32,
        first: f32,
        last: f32,
    },
    #[error(
        "MoverProbeChannelPoseOutOfRange: {block} {what} {index} (mover {mover_id}) names a pose at or past the mover's {pose_count} pose(s)"
    )]
    PoseOutOfRange {
        block: &'static str,
        what: &'static str,
        index: usize,
        mover_id: u32,
        pose_count: usize,
    },
    #[error(
        "MoverProbeChannelBounceEntryLength: bounce entry {entry} (mover {mover_id}) spans {bytes} byte(s); its mover's {pose_count} pose(s) need {expected}"
    )]
    BounceEntryLength {
        entry: usize,
        mover_id: u32,
        pose_count: usize,
        bytes: usize,
        expected: usize,
    },
    #[error(
        "MoverProbeChannelStoredAndBase: {block} {what} {index} marks local probe {probe} both stored and base at pose {pose}"
    )]
    StoredAndBase {
        block: &'static str,
        what: &'static str,
        index: usize,
        pose: usize,
        probe: u32,
    },
    #[error(
        "MoverProbeChannelBaseAtTerminus: {block} {what} {index} marks base at terminus pose {pose}; an unmarked probe already reads the base there"
    )]
    BaseAtTerminus {
        block: &'static str,
        what: &'static str,
        index: usize,
        pose: usize,
    },
    #[error(
        "MoverProbeChannelBaseOutsideAffected: bounce entry {entry} in cell {cell} marks base at pose {pose} for probes {mask:#018x} that store no state at any pose"
    )]
    BaseOutsideAffected {
        cell: usize,
        entry: usize,
        pose: usize,
        mask: u64,
    },
    #[error(
        "MoverProbeChannelAcrossMidPoseCount: across entry {entry} (mover {mover_id}) holds {found} mid-pose mask(s); its mover's poses have {expected} mid pose(s), and a terminus is never marked across"
    )]
    AcrossMidPoseCount {
        entry: usize,
        mover_id: u32,
        found: u32,
        expected: usize,
    },
    #[error(
        "MoverProbeChannelAcrossCellOutOfRange: across entry {entry} names listed cell {listed}, past the {cells} listed cell(s)"
    )]
    AcrossCellOutOfRange {
        entry: usize,
        listed: u32,
        cells: usize,
    },
    #[error(
        "MoverProbeChannelAcrossOrder: across entry {entry} does not follow its predecessor; entries ascend by (cell, mover) with no repeats"
    )]
    AcrossOrder { entry: usize },
    #[error(
        "MoverProbeChannelAcrossEntryEmpty: across entry {entry} (mover {mover_id}) marks no probe at any mid pose; only (cell, mover) pairs with a mark are written"
    )]
    AcrossEntryEmpty { entry: usize, mover_id: u32 },
    #[error(
        "MoverProbeChannelAcrossMarkNotValid: across entry {entry} in cell {cell} marks probes {mask:#018x} at pose {pose}, which is not a subset of the valid probes {valid:#018x}"
    )]
    AcrossMarkNotValid {
        cell: usize,
        entry: usize,
        pose: usize,
        mask: u64,
        valid: u64,
    },
    #[error(
        "MoverProbeChannelAcrossMarkNotNamed: across entry {entry} in cell {cell} marks local probe {probe} at pose {pose}, but no direct record or bounce entry of mover {mover_id} names that probe there"
    )]
    AcrossMarkNotNamed {
        cell: usize,
        entry: usize,
        pose: usize,
        probe: u32,
        mover_id: u32,
    },
    #[error(
        "MoverProbeChannelAcrossBounceState: across entry {entry} in cell {cell} marks local probe {probe} at pose {pose}, where mover {mover_id}'s bounce entry stores a state or reads the base; an enclosed mid pose stores no value state and blends across"
    )]
    AcrossBounceState {
        cell: usize,
        entry: usize,
        pose: usize,
        probe: u32,
        mover_id: u32,
    },
    #[error(
        "MoverProbeChannelAcrossVisibility: direct record {record} in cell {cell} stores q {q} for mover {mover_id} at local probe {probe}, pose {pose}, which the across marks name; an across pose stores 255 and is never read"
    )]
    AcrossVisibility {
        cell: usize,
        record: usize,
        mover_id: u32,
        probe: u32,
        pose: usize,
        q: u8,
    },
    #[error("MoverProbeChannelTrailingBytes: {count} byte(s) follow the depth block")]
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
