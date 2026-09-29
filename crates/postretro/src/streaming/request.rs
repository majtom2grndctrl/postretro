//! Resource-neutral read request: resource, key, tier, identity, and ranges.
//! See: context/lib/rendering_pipeline.md §"Cluster SH residency"

use std::ops::Range;

/// A streamed resource that shares the one read issuer and drain budget.
/// Declaration order is the drain tie-break at equal class: SH first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum StreamResource {
    /// One id-50 cluster chunk. Key: cluster id.
    Sh,
    /// One lightmap cell block with its shadowmask block, installed as a
    /// pair. Key: block id.
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "lightmap residency submits block reads next")
    )]
    LightmapBlock,
}

impl StreamResource {
    pub(crate) const COUNT: usize = 2;

    pub(crate) const fn index(self) -> usize {
        match self {
            Self::Sh => 0,
            Self::LightmapBlock => 1,
        }
    }

    /// Error-section label, matching the resource's log tag.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Sh => "SH streaming",
            Self::LightmapBlock => "lightmap streaming",
        }
    }
}

/// Mandatory work of every resource is read and drained before optional
/// work of any. Declaration order is that precedence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum ReadTier {
    Mandatory,
    Optional,
}

/// Ranges one request may carry. A lightmap pair reads id 22's irradiance
/// plus direction blob and id 42's two shadowmask groups: two ranges.
pub(crate) const MAX_READ_RANGES: usize = 2;

/// A request's absolute PRL file ranges, in the order its bytes are handed
/// back. Inline, so a request is `Copy` and submitting one never allocates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReadRanges {
    spans: [(u64, u64); MAX_READ_RANGES],
    len: u8,
}

impl ReadRanges {
    /// One range. An empty range needs no read; it is handed back as an
    /// empty buffer.
    pub(crate) fn one(range: Range<u64>) -> Self {
        debug_assert!(range.start <= range.end, "reversed read range");
        Self {
            spans: [(range.start, range.end), (0, 0)],
            len: 1,
        }
    }

    /// Two ranges read as one unit: the request completes once, after both.
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "lightmap residency submits block pairs next")
    )]
    pub(crate) fn pair(first: Range<u64>, second: Range<u64>) -> Self {
        debug_assert!(first.start <= first.end && second.start <= second.end);
        Self {
            spans: [(first.start, first.end), (second.start, second.end)],
            len: 2,
        }
    }

    pub(crate) fn len(&self) -> usize {
        usize::from(self.len)
    }

    pub(crate) fn get(&self, index: usize) -> Range<u64> {
        let (start, end) = self.spans[..self.len()][index];
        start..end
    }
}

/// Which level generation and content a request was issued against. A
/// completion whose identity no longer matches its resource is stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReadIdentity {
    pub(crate) generation: u64,
    /// The level's content tag.
    pub(crate) content_tag: [u8; 32],
    /// The item's own content hash: an SH chunk hash, or zero for a resource
    /// whose items carry none.
    pub(crate) item_hash: [u8; 32],
}

/// One resource item for the shared issuer to read.
///
/// Invariants the issuer keeps for every request, whatever its resource:
/// - Mandatory requests of every resource are read before optional requests
///   of any; within a tier, ranges are read in ascending file offset.
/// - Nearby ranges of one resource coalesce into one physical read under
///   `COALESCE_MAX_GAP_BYTES` and `COALESCE_MAX_SPAN_BYTES`. A read never
///   spans two resources, so each resource's reader validates its own spans.
/// - New submissions are taken after every physical read, so fresh mandatory
///   work preempts queued optional work.
/// - A request whose key its resource no longer targets is cancelled before
///   its next read, whole: bytes already read for it are dropped.
/// - A request completes exactly once, after all its ranges are read, or
///   is cancelled or failed whole.
/// - A submission identical to a request still pending at the issuer (same
///   resource, key, and identity) is absorbed: no second read, no second
///   completion. It can only raise the pending request's tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReadRequest {
    pub(crate) resource: StreamResource,
    /// Resource-local key: SH cluster id, or lightmap block id.
    pub(crate) key: u32,
    pub(crate) tier: ReadTier,
    pub(crate) identity: ReadIdentity,
    pub(crate) ranges: ReadRanges,
}

impl ReadRequest {
    /// True when `other` names the same item: resource, key, and identity.
    pub(crate) fn same_item(&self, other: &Self) -> bool {
        self.resource == other.resource && self.key == other.key && self.identity == other.identity
    }
}
