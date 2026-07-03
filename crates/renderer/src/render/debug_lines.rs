// Immediate-mode debug-line renderer: per-frame CPU buffer of LineList segments uploaded each frame.
// See: context/lib/rendering_pipeline.md §12 Debug-Line Renderer

use bytemuck::{Pod, Zeroable};
use glam::Vec3;

const DEBUG_LINES_SHADER_SOURCE: &str = include_str!("../shaders/debug_lines.wgsl");

const MAX_DEBUG_SEGMENTS: usize = 256 * 1024;
/// Overlay (always-on-top) segments are only used for bounding-shape AABBs,
/// which carry 12 segments each. A fraction of the depth-tested cap is plenty.
const MAX_DEBUG_OVERLAY_SEGMENTS: usize = MAX_DEBUG_SEGMENTS / 8;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct DebugLineVertex {
    position: [f32; 3],
    color: [u8; 4],
}

pub struct DebugLineRenderer {
    pipeline: wgpu::RenderPipeline,
    overlay_pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    overlay_vertex_buffer: wgpu::Buffer,
    cpu_vertices: Vec<DebugLineVertex>,
    overlay_cpu_vertices: Vec<DebugLineVertex>,
    overflowed_this_frame: bool,
    overlay_overflowed_this_frame: bool,
}

/// The 12 edges of an axis-aligned box, as `(start, end)` segment pairs.
/// Ordered: 4 bottom-face edges, 4 top-face edges, 4 vertical edges.
/// Pure function — no GPU/state dependency — so the wire topology can be
/// asserted by tests without constructing a `DebugLineRenderer`.
fn aabb_edges(min: Vec3, max: Vec3) -> [(Vec3, Vec3); 12] {
    let c = [
        Vec3::new(min.x, min.y, min.z),
        Vec3::new(max.x, min.y, min.z),
        Vec3::new(max.x, min.y, max.z),
        Vec3::new(min.x, min.y, max.z),
        Vec3::new(min.x, max.y, min.z),
        Vec3::new(max.x, max.y, min.z),
        Vec3::new(max.x, max.y, max.z),
        Vec3::new(min.x, max.y, max.z),
    ];
    [
        // Bottom face
        (c[0], c[1]),
        (c[1], c[2]),
        (c[2], c[3]),
        (c[3], c[0]),
        // Top face
        (c[4], c[5]),
        (c[5], c[6]),
        (c[6], c[7]),
        (c[7], c[4]),
        // Verticals
        (c[0], c[4]),
        (c[1], c[5]),
        (c[2], c[6]),
        (c[3], c[7]),
    ]
}

/// Number of segments per ring in [`capsule_edges`]. Low on purpose — an honest
/// debug wireframe, not a smooth capsule. 12 reads as a recognizable circle
/// while keeping the segment budget tiny.
const CAPSULE_RING_SEGMENTS: usize = 12;

/// Number of vertical connectors between the two cylinder rings, and the number
/// of meridian half-circles drawn over each hemisphere cap.
const CAPSULE_VERTICAL_SEGMENTS: usize = 4;

/// Number of straight segments approximating each cap meridian half-circle (a
/// quarter-turn from the ring plane up to the pole).
const CAPSULE_CAP_ARC_SEGMENTS: usize = 4;

/// Total segment count emitted by [`capsule_edges`]: two horizontal rings, the
/// vertical connectors between them, and two meridian half-arcs per vertical
/// (one over the top cap, one under the bottom cap).
const CAPSULE_SEGMENT_COUNT: usize = 2 * CAPSULE_RING_SEGMENTS
    + CAPSULE_VERTICAL_SEGMENTS
    + 2 * CAPSULE_VERTICAL_SEGMENTS * CAPSULE_CAP_ARC_SEGMENTS;

/// An upright (Y-axis) capsule wireframe as `(start, end)` segment pairs,
/// centered on `center`. The `center` convention matches the player pawn's
/// `Transform.position`, which the collision capsule is symmetric about (see
/// `movement/substrate.rs`: the capsule spans `-half_height..+half_height` in
/// local space, so the lowest point is `center.y - (half_height + radius)` —
/// the feet — and the highest is `center.y + (half_height + radius)` — the head).
///
/// Topology (an honest low-poly wireframe, not a smooth capsule):
/// - a top ring and a bottom ring of the cylinder section (at
///   `center.y ± half_height`, radius `radius`),
/// - [`CAPSULE_VERTICAL_SEGMENTS`] vertical connectors joining the two rings,
/// - over each cap, a meridian half-circle per vertical connector, swept from
///   the ring up/down to the pole as [`CAPSULE_CAP_ARC_SEGMENTS`] chords.
///
/// Pure function — no GPU/state dependency — so the wire topology can be
/// asserted by tests without constructing a `DebugLineRenderer`.
fn capsule_edges(center: Vec3, radius: f32, half_height: f32) -> Vec<(Vec3, Vec3)> {
    let mut segments = Vec::with_capacity(CAPSULE_SEGMENT_COUNT);

    let top_y = center.y + half_height;
    let bottom_y = center.y + -half_height;

    // A point on a horizontal ring of `radius` at height `y`, at ring angle `a`.
    let ring_point =
        |a: f32, y: f32| Vec3::new(center.x + radius * a.cos(), y, center.z + radius * a.sin());

    // Two horizontal rings (top + bottom of the cylinder section).
    for i in 0..CAPSULE_RING_SEGMENTS {
        let a0 = std::f32::consts::TAU * (i as f32) / (CAPSULE_RING_SEGMENTS as f32);
        let a1 = std::f32::consts::TAU * ((i + 1) as f32) / (CAPSULE_RING_SEGMENTS as f32);
        segments.push((ring_point(a0, top_y), ring_point(a1, top_y)));
        segments.push((ring_point(a0, bottom_y), ring_point(a1, bottom_y)));
    }

    // Vertical connectors + cap meridian half-circles, sharing the same set of
    // ring angles so each vertical's caps meet the cylinder seam cleanly.
    for v in 0..CAPSULE_VERTICAL_SEGMENTS {
        let a = std::f32::consts::TAU * (v as f32) / (CAPSULE_VERTICAL_SEGMENTS as f32);
        let dir = Vec3::new(a.cos(), 0.0, a.sin());

        let top_ring = Vec3::new(center.x, top_y, center.z) + dir * radius;
        let bottom_ring = Vec3::new(center.x, bottom_y, center.z) + dir * radius;
        // Cylinder-wall vertical connector.
        segments.push((bottom_ring, top_ring));

        // Cap meridians: sweep a quarter-circle from the ring plane (`t = 0`,
        // out along `dir` at the ring height) to the pole (`t = π/2`, straight
        // up/down by `radius`), as a chain of chords.
        let top_pole = Vec3::new(center.x, top_y + radius, center.z);
        let bottom_pole = Vec3::new(center.x, bottom_y + -radius, center.z);
        for s in 0..CAPSULE_CAP_ARC_SEGMENTS {
            let t0 = std::f32::consts::FRAC_PI_2 * (s as f32) / (CAPSULE_CAP_ARC_SEGMENTS as f32);
            let t1 =
                std::f32::consts::FRAC_PI_2 * ((s + 1) as f32) / (CAPSULE_CAP_ARC_SEGMENTS as f32);

            // Top cap: out-component shrinks as `cos`, up-component grows as `sin`.
            let top0 = Vec3::new(center.x, top_y, center.z)
                + dir * (radius * t0.cos())
                + Vec3::Y * (radius * t0.sin());
            let top1 = Vec3::new(center.x, top_y, center.z)
                + dir * (radius * t1.cos())
                + Vec3::Y * (radius * t1.sin());
            // The final chord must land exactly on the pole.
            let top1 = if s + 1 == CAPSULE_CAP_ARC_SEGMENTS {
                top_pole
            } else {
                top1
            };
            segments.push((top0, top1));

            // Bottom cap: mirror downward.
            let bot0 = Vec3::new(center.x, bottom_y, center.z)
                + dir * (radius * t0.cos())
                + Vec3::NEG_Y * (radius * t0.sin());
            let bot1 = Vec3::new(center.x, bottom_y, center.z)
                + dir * (radius * t1.cos())
                + Vec3::NEG_Y * (radius * t1.sin());
            let bot1 = if s + 1 == CAPSULE_CAP_ARC_SEGMENTS {
                bottom_pole
            } else {
                bot1
            };
            segments.push((bot0, bot1));
        }
    }

    segments
}

/// Three axis-aligned segments forming an `+` marker of total length `size`
/// centered at `center`. Pure function — no GPU/state dependency.
fn marker_segments(center: Vec3, size: f32) -> [(Vec3, Vec3); 3] {
    let h = size * 0.5;
    [
        (
            center - Vec3::new(h, 0.0, 0.0),
            center + Vec3::new(h, 0.0, 0.0),
        ),
        (
            center - Vec3::new(0.0, h, 0.0),
            center + Vec3::new(0.0, h, 0.0),
        ),
        (
            center - Vec3::new(0.0, 0.0, h),
            center + Vec3::new(0.0, 0.0, h),
        ),
    ]
}

impl DebugLineRenderer {
    pub fn new(
        device: &wgpu::Device,
        color_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        sample_count: u32,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Debug Lines Shader"),
            source: wgpu::ShaderSource::Wgsl(DEBUG_LINES_SHADER_SOURCE.into()),
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Debug Lines Pipeline Layout"),
            bind_group_layouts: &[Some(uniform_bind_group_layout)],
            immediate_size: 0,
        });

        let make_pipeline = |label: &str, depth_compare: wgpu::CompareFunction| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<DebugLineVertex>() as wgpu::BufferAddress,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[
                            wgpu::VertexAttribute {
                                offset: 0,
                                shader_location: 0,
                                format: wgpu::VertexFormat::Float32x3,
                            },
                            wgpu::VertexAttribute {
                                offset: 12,
                                shader_location: 1,
                                format: wgpu::VertexFormat::Unorm8x4,
                            },
                        ],
                    }],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::LineList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: depth_format,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(depth_compare),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: sample_count,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: color_format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                multiview_mask: None,
                cache: None,
            })
        };

        // Depth-tested pipeline for shapes that should respect world occlusion
        // (probe markers, per-cell wires). Always-on-top pipeline for shapes
        // whose value is x-ray visibility from anywhere in the world (bounding
        // AABBs that sit at or beyond the opaque world hull).
        let pipeline = make_pipeline("Debug Lines Pipeline", wgpu::CompareFunction::LessEqual);
        let overlay_pipeline = make_pipeline(
            "Debug Lines Overlay Pipeline",
            wgpu::CompareFunction::Always,
        );

        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Debug Lines Vertex Buffer"),
            size: (MAX_DEBUG_SEGMENTS * 2 * std::mem::size_of::<DebugLineVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let overlay_vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Debug Lines Overlay Vertex Buffer"),
            size: (MAX_DEBUG_OVERLAY_SEGMENTS * 2 * std::mem::size_of::<DebugLineVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            overlay_pipeline,
            vertex_buffer,
            overlay_vertex_buffer,
            cpu_vertices: Vec::with_capacity(MAX_DEBUG_SEGMENTS * 2),
            overlay_cpu_vertices: Vec::with_capacity(MAX_DEBUG_OVERLAY_SEGMENTS * 2),
            overflowed_this_frame: false,
            overlay_overflowed_this_frame: false,
        }
    }

    pub fn clear(&mut self) {
        self.cpu_vertices.clear();
        self.overlay_cpu_vertices.clear();
        self.overflowed_this_frame = false;
        self.overlay_overflowed_this_frame = false;
    }

    pub fn push_line(&mut self, start: Vec3, end: Vec3, color_rgba: [u8; 4]) {
        if self.cpu_vertices.len() + 2 > MAX_DEBUG_SEGMENTS * 2 {
            if !self.overflowed_this_frame {
                log::warn!(
                    "DebugLineRenderer: segment cap {} reached; truncating",
                    MAX_DEBUG_SEGMENTS
                );
                self.overflowed_this_frame = true;
            }
            return;
        }
        self.cpu_vertices.push(DebugLineVertex {
            position: start.to_array(),
            color: color_rgba,
        });
        self.cpu_vertices.push(DebugLineVertex {
            position: end.to_array(),
            color: color_rgba,
        });
    }

    pub fn push_aabb(&mut self, min: Vec3, max: Vec3, color_rgba: [u8; 4]) {
        for (a, b) in aabb_edges(min, max) {
            self.push_line(a, b, color_rgba);
        }
    }

    pub fn push_marker(&mut self, center: Vec3, size: f32, color_rgba: [u8; 4]) {
        for (a, b) in marker_segments(center, size) {
            self.push_line(a, b, color_rgba);
        }
    }

    pub fn push_line_overlay(&mut self, start: Vec3, end: Vec3, color_rgba: [u8; 4]) {
        if self.overlay_cpu_vertices.len() + 2 > MAX_DEBUG_OVERLAY_SEGMENTS * 2 {
            if !self.overlay_overflowed_this_frame {
                log::warn!(
                    "DebugLineRenderer: overlay segment cap {} reached; truncating",
                    MAX_DEBUG_OVERLAY_SEGMENTS
                );
                self.overlay_overflowed_this_frame = true;
            }
            return;
        }
        self.overlay_cpu_vertices.push(DebugLineVertex {
            position: start.to_array(),
            color: color_rgba,
        });
        self.overlay_cpu_vertices.push(DebugLineVertex {
            position: end.to_array(),
            color: color_rgba,
        });
    }

    pub fn push_aabb_overlay(&mut self, min: Vec3, max: Vec3, color_rgba: [u8; 4]) {
        for (a, b) in aabb_edges(min, max) {
            self.push_line_overlay(a, b, color_rgba);
        }
    }

    /// Push an upright capsule wireframe through the always-on-top overlay path
    /// (see [`capsule_edges`] for the topology and the `center`-is-pawn-position
    /// convention). Like [`push_aabb_overlay`](Self::push_aabb_overlay), it feeds
    /// `push_line_overlay`, so the capsule draws with `CompareFunction::Always` and
    /// stays visible through walls — the right behavior for a "where is the other
    /// player" marker, which must be locatable from anywhere rather than vanish
    /// behind world geometry.
    pub fn push_capsule_overlay(
        &mut self,
        center: Vec3,
        radius: f32,
        half_height: f32,
        color_rgba: [u8; 4],
    ) {
        for (a, b) in capsule_edges(center, radius, half_height) {
            self.push_line_overlay(a, b, color_rgba);
        }
    }

    pub fn render(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        color_view: &wgpu::TextureView,
        depth_view: &wgpu::TextureView,
        uniform_bind_group: &wgpu::BindGroup,
    ) {
        if self.cpu_vertices.is_empty() && self.overlay_cpu_vertices.is_empty() {
            return;
        }

        if !self.cpu_vertices.is_empty() {
            queue.write_buffer(
                &self.vertex_buffer,
                0,
                bytemuck::cast_slice(&self.cpu_vertices),
            );
        }
        if !self.overlay_cpu_vertices.is_empty() {
            queue.write_buffer(
                &self.overlay_vertex_buffer,
                0,
                bytemuck::cast_slice(&self.overlay_cpu_vertices),
            );
        }

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Debug Lines Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });

        pass.set_bind_group(0, uniform_bind_group, &[]);

        // Depth-tested first so overlay lines (drawn second) win at any pixel
        // both pipelines touch — bounding AABBs should never be visually
        // clipped by a depth-tested wire at the same pixel.
        if !self.cpu_vertices.is_empty() {
            pass.set_pipeline(&self.pipeline);
            let vertex_bytes =
                (self.cpu_vertices.len() * std::mem::size_of::<DebugLineVertex>()) as u64;
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(0..vertex_bytes));
            pass.draw(0..self.cpu_vertices.len() as u32, 0..1);
        }

        if !self.overlay_cpu_vertices.is_empty() {
            pass.set_pipeline(&self.overlay_pipeline);
            let vertex_bytes =
                (self.overlay_cpu_vertices.len() * std::mem::size_of::<DebugLineVertex>()) as u64;
            pass.set_vertex_buffer(0, self.overlay_vertex_buffer.slice(0..vertex_bytes));
            pass.draw(0..self.overlay_cpu_vertices.len() as u32, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Topology contract: an AABB emits exactly 12 edges, each corner is shared
    /// by exactly 3 edges, and every edge runs along a single axis (axis-aligned).
    #[test]
    fn aabb_edges_form_axis_aligned_box_with_twelve_edges() {
        let min = Vec3::new(-1.0, -2.0, -3.0);
        let max = Vec3::new(4.0, 5.0, 6.0);
        let edges = aabb_edges(min, max);

        assert_eq!(edges.len(), 12);

        // Every edge must vary along exactly one axis.
        for (a, b) in edges {
            let diffs = [
                (a.x - b.x).abs() > f32::EPSILON,
                (a.y - b.y).abs() > f32::EPSILON,
                (a.z - b.z).abs() > f32::EPSILON,
            ];
            let varying = diffs.iter().filter(|d| **d).count();
            assert_eq!(varying, 1, "edge {a:?} -> {b:?} is not axis-aligned");
        }

        // Each of the 8 corners must appear as an endpoint exactly 3 times
        // (3 edges per corner of a box).
        let mut endpoint_counts: std::collections::HashMap<[u32; 3], usize> =
            std::collections::HashMap::new();
        let key = |v: Vec3| [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()];
        for (a, b) in edges {
            *endpoint_counts.entry(key(a)).or_default() += 1;
            *endpoint_counts.entry(key(b)).or_default() += 1;
        }
        assert_eq!(endpoint_counts.len(), 8, "expected 8 distinct corners");
        for (corner, count) in &endpoint_counts {
            assert_eq!(
                *count, 3,
                "corner {corner:?} appears {count} times, expected 3"
            );
        }
    }

    /// Topology contract: a marker emits 3 axis-aligned segments of length
    /// `size`, all sharing `center` as midpoint.
    #[test]
    fn marker_segments_form_three_axis_crosshair_at_center() {
        let center = Vec3::new(10.0, 20.0, 30.0);
        let size = 0.5;
        let segs = marker_segments(center, size);

        assert_eq!(segs.len(), 3);

        // Each segment's midpoint is the center, and each runs along a unique axis.
        let mut axes_seen = [false; 3];
        for (a, b) in segs {
            let mid = (a + b) * 0.5;
            assert!((mid - center).length() < 1e-5, "segment midpoint != center");
            assert!(
                ((b - a).length() - size).abs() < 1e-5,
                "segment length != size"
            );

            let d = b - a;
            let axis = if d.x.abs() > f32::EPSILON {
                0
            } else if d.y.abs() > f32::EPSILON {
                1
            } else {
                2
            };
            assert!(!axes_seen[axis], "duplicate axis for marker segment");
            axes_seen[axis] = true;
        }
        assert!(axes_seen.iter().all(|&s| s), "marker missed an axis");
    }

    /// Topology contract: a capsule emits exactly [`CAPSULE_SEGMENT_COUNT`]
    /// segments — two rings, the vertical connectors, and two cap arcs per
    /// vertical.
    #[test]
    fn capsule_edges_emit_expected_segment_count() {
        let edges = capsule_edges(Vec3::new(1.0, 2.0, 3.0), 0.4, 0.8);
        assert_eq!(edges.len(), CAPSULE_SEGMENT_COUNT);
    }

    /// Capacity contract for the remote-entity marker fix (M15 Phase 1): the
    /// always-on-top overlay buffer must hold the per-frame capsule load without
    /// truncating. Remote-entity capsules route through the overlay path
    /// (`push_capsule_overlay`) so they draw through walls; each capsule costs
    /// [`CAPSULE_SEGMENT_COUNT`] (60) segments. A generous co-op remote-entity
    /// count must fit alongside the handful of SH/nav AABB overlays (12 segments
    /// each) under [`MAX_DEBUG_OVERLAY_SEGMENTS`], so a busy frame never silently
    /// drops a player marker.
    #[test]
    fn overlay_cap_holds_many_remote_entity_capsules() {
        // Phase 1 replicates the full Transform-bearing set, so the client can
        // see many ghosts on a populated map; 256 capsules is a comfortable
        // upper bound for a co-op scene and is the headroom this fix relies on.
        const REMOTE_CAPSULES: usize = 256;
        // A few AABB overlays may coexist (SH/delta diagnostics, 12 each).
        const COEXISTING_AABB_OVERLAYS: usize = 8;

        let segments = REMOTE_CAPSULES * CAPSULE_SEGMENT_COUNT + COEXISTING_AABB_OVERLAYS * 12;
        assert!(
            segments <= MAX_DEBUG_OVERLAY_SEGMENTS,
            "overlay segment budget {MAX_DEBUG_OVERLAY_SEGMENTS} must hold \
             {REMOTE_CAPSULES} remote-entity capsules ({CAPSULE_SEGMENT_COUNT} segs each) \
             plus {COEXISTING_AABB_OVERLAYS} AABB overlays; needed {segments}"
        );
    }

    /// Extent contract: the wireframe spans `[center.y - (half_height + radius),
    /// center.y + (half_height + radius)]` vertically (feet to head, matching the
    /// pawn capsule), reaches exactly `radius` horizontally from the center axis,
    /// and is centered on the given `center` x/z.
    #[test]
    fn capsule_edges_span_full_height_and_radius_centered() {
        // Tight epsilon: endpoints are computed from cos/sin, so compare
        // approximately rather than for bit-equality (testing_guide
        // §Floating-point).
        const EPSILON: f32 = 1e-5;

        let center = Vec3::new(2.0, 5.0, -3.0);
        let radius = 0.4;
        let half_height = 0.8;
        let edges = capsule_edges(center, radius, half_height);

        let mut min_y = f32::INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        let mut max_horiz = 0.0_f32;
        for (a, b) in &edges {
            for p in [a, b] {
                min_y = min_y.min(p.y);
                max_y = max_y.max(p.y);
                let horiz = ((p.x - center.x).powi(2) + (p.z - center.z).powi(2)).sqrt();
                max_horiz = max_horiz.max(horiz);
            }
        }

        let total_half = half_height + radius;
        assert!(
            (min_y - (center.y - total_half)).abs() < EPSILON,
            "bottom (feet) should sit at center.y - (half_height + radius), got {min_y}"
        );
        assert!(
            (max_y - (center.y + total_half)).abs() < EPSILON,
            "top (head) should sit at center.y + (half_height + radius), got {max_y}"
        );
        // The widest point is the cylinder ring at exactly `radius`; the caps
        // curve inward, so no point exceeds it.
        assert!(
            (max_horiz - radius).abs() < EPSILON,
            "max horizontal extent should equal radius, got {max_horiz}"
        );

        // The vertical span is symmetric about center.y, confirming center (not
        // feet or head) is the convention.
        let mid_y = (min_y + max_y) * 0.5;
        assert!(
            (mid_y - center.y).abs() < EPSILON,
            "vertical extent should be centered on center.y, got mid {mid_y}"
        );
    }
}
