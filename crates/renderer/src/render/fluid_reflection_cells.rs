// Cell-rate spike (debug only, never merged): the pixel reflection style
// evaluated once per water texel cell in a compute pass into a cell atlas,
// read by the reflecting fragment instead of per-fragment evaluation.
// See: context/lib/rendering_pipeline.md §7.3a (Reflection, Style)

use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};
use glam::{DVec2, DVec3, Vec3};
use wgpu::util::DeviceExt;

use postretro_level_format::environment_volumes::{
    EnvironmentSurfaceFace, EnvironmentVolumeRecord,
};
use postretro_render_cpu::fluid_surface::reflection::FLUID_REFLECTION_NO_PROBE;
use postretro_render_cpu::fluid_surface::{
    FluidShaderVariant, FluidSurfaceMesh, FluidSurfaceSlots, face_normal, material_index,
};

use super::fluid_reflection_probes::fluid_probe_bind_group_layout_entries;

/// Group 4 binding of the cell atlas in the render layout.
pub(crate) const FLUID_CELL_ATLAS_BINDING: u32 = 4;
/// Compute-only group 5 bindings.
const FLUID_CELL_PARAMS_BINDING: u32 = 8;
const FLUID_CELL_JOBS_BINDING: u32 = 9;
const FLUID_CELL_ATLAS_OUT_BINDING: u32 = 10;
/// Rect faces one level may hold; the params uniform holds this many.
pub(crate) const FLUID_CELL_MAX_FACES: usize = 512;
const FLUID_CELL_TILE: u32 = 8;
const FLUID_CELL_DISPATCH_ROW: u32 = 65_535;
const FLUID_CELL_PARAMS_HEADER_BYTES: u64 = 16;
const FLUID_CELL_FACE_BYTES: u64 = 96;
const FLUID_CELL_PARAMS_BYTES: u64 =
    FLUID_CELL_PARAMS_HEADER_BYTES + FLUID_CELL_MAX_FACES as u64 * FLUID_CELL_FACE_BYTES;
const FLUID_CELL_JOBS_HEADER_BYTES: u64 = 16;
const FLUID_CELL_FLAG_CULL: u32 = 1;
const FLUID_CELL_FLAG_COUNT: u32 = 2;
const FLUID_CELL_ATLAS_MAX: u32 = 8192;
const FLUID_CELL_ATLAS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const FLUID_CELL_TEXEL_BYTES: u64 = 8;
/// Texel-space margin that keeps a rect edge on an exact integer from
/// adding an empty row or column.
const FLUID_CELL_RECT_EPSILON: f64 = 1.0e-4;
/// A face counts as seen from above for dispatch slightly past its plane,
/// so float noise at the fragment gate never reads a stale cell.
const FLUID_CELL_ABOVE_SLACK: f32 = 1.0e-3;

/// One vertex of the fourth vertex buffer: the face's atlas offset (atlas
/// origin minus first cell) and its inclusive cell rect, zero for a face
/// with no rect.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub(crate) struct FluidCellVertex {
    pub offset: [i32; 2],
    pub bounds: [i32; 4],
}
pub(crate) const FLUID_CELL_VERTEX_STRIDE: usize = 24;
const _: () = assert!(std::mem::size_of::<FluidCellVertex>() == FLUID_CELL_VERTEX_STRIDE);

pub(crate) const FLUID_CELL_VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 2] = [
    wgpu::VertexAttribute {
        offset: 0,
        shader_location: 5,
        format: wgpu::VertexFormat::Sint32x2,
    },
    wgpu::VertexAttribute {
        offset: 8,
        shader_location: 6,
        format: wgpu::VertexFormat::Sint32x4,
    },
];

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct FluidCellFaceGpu {
    origin: [f32; 3],
    probe: u32,
    axis_u: [f32; 3],
    reflectivity: f32,
    axis_v: [f32; 3],
    _pad0: u32,
    outward: [f32; 3],
    _pad1: u32,
    cell_min: [i32; 2],
    cell_count: [u32; 2],
    atlas_origin: [u32; 2],
    _pad2: [u32; 2],
}
const _: () = assert!(std::mem::size_of::<FluidCellFaceGpu>() as u64 == FLUID_CELL_FACE_BYTES);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct FluidCellParamsHeader {
    viewport: [f32; 2],
    flags: u32,
    face_count: u32,
}

/// The spike's debug levers, all off by default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FluidCellLevers {
    /// Read pixel-style reflections from the cell atlas.
    pub buffer: bool,
    /// Skip cells whose four corners all lie outside one frustum plane.
    pub cull: bool,
    /// Skip the live-lighting ratio (per fragment and per cell).
    pub ratio_one: bool,
    /// Count evaluated cells and occlusion-query the fluid pass (untimed runs).
    pub count: bool,
    /// Baseline: every draw takes the reflection-free pipeline under the
    /// frame's mask, so the fluid's lighting is exactly the reflecting draws'.
    pub reflection_off: bool,
    /// Two-draw cells: reflection-free surface, then a premultiplied overlay.
    pub overlay: bool,
}

/// What the last cell frame dispatched and evaluated (capture only).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FluidCellStats {
    /// Rect faces the last frame dispatched.
    pub faces_dispatched: u32,
    /// Cells of those faces' rects.
    pub cells_dispatched: u64,
    /// Cells the compute evaluated (the atomic); equals dispatched with cull off.
    pub cells_evaluated: u32,
    /// Samples the fluid pass's draws passed (occlusion query).
    pub fluid_samples_passed: u64,
    pub atlas_width: u32,
    pub atlas_height: u32,
    pub atlas_bytes: u64,
    /// Cells of every rect in the level.
    pub cells_total: u64,
}

/// One probe-bound drawable face with a cell rect.
#[derive(Clone, Copy, Debug)]
struct CellFace {
    slot: u32,
    probe: u32,
    outward: Vec3,
    /// World position of texel coordinate `cell_min` (first cell's corner).
    origin: Vec3,
    axis_u: Vec3,
    axis_v: Vec3,
    cell_min: [i32; 2],
    cell_count: [u32; 2],
    atlas_origin: [u32; 2],
}

impl CellFace {
    fn tiles(&self) -> [u32; 2] {
        [
            self.cell_count[0].div_ceil(FLUID_CELL_TILE),
            self.cell_count[1].div_ceil(FLUID_CELL_TILE),
        ]
    }

    fn cells(&self) -> u64 {
        u64::from(self.cell_count[0]) * u64::from(self.cell_count[1])
    }
}

/// The level's install-time cell layout: every rect face, packed into one
/// atlas.
pub(crate) struct FluidCellLayout {
    faces: Vec<CellFace>,
    /// Face indices per draw slot.
    slot_faces: Vec<Vec<u32>>,
    /// A slot reads cells only if every probe-bound drawable face got a rect.
    slot_capable: Vec<bool>,
    atlas: [u32; 2],
    cells_total: u64,
    tiles_total: u32,
}

/// The exact affine texel-to-world map of a face's ring: world =
/// p0 + a·(u − u0) + b·(v − v0), fitted in f64 from the three ring vertices
/// spanning the largest texel-space area.
fn fit_texel_map(face: &EnvironmentSurfaceFace) -> Option<(DVec3, DVec2, DVec3, DVec3)> {
    let ring: Vec<(DVec3, DVec2)> = face
        .vertices
        .iter()
        .map(|vertex| {
            (
                Vec3::from(vertex.position).as_dvec3(),
                DVec2::new(f64::from(vertex.uv[0]), f64::from(vertex.uv[1])),
            )
        })
        .collect();
    let n = ring.len();
    let mut best = (0.0f64, 0, 1, 2);
    for i in 0..n {
        for j in i + 1..n {
            for k in j + 1..n {
                let (d1, d2) = (ring[j].1 - ring[i].1, ring[k].1 - ring[i].1);
                let det = d1.x * d2.y - d2.x * d1.y;
                if det.abs() > best.0.abs() {
                    best = (det, i, j, k);
                }
            }
        }
    }
    let (det, i, j, k) = best;
    if det.abs() < 1.0e-9 {
        return None;
    }
    let (p0, uv0) = ring[i];
    let (dp1, d1) = (ring[j].0 - p0, ring[j].1 - uv0);
    let (dp2, d2) = (ring[k].0 - p0, ring[k].1 - uv0);
    let a = (dp1 * d2.y - dp2 * d1.y) / det;
    let b = (dp2 * d1.x - dp1 * d2.x) / det;
    Some((p0, uv0, a, b))
}

/// Per-vertex cell data for the whole mesh, and the layout when the level
/// can read cells (`None`: too many faces, an atlas past the cap, or no rect
/// face).
pub(crate) fn build_cell_layout(
    volumes: &[EnvironmentVolumeRecord],
    mesh: &FluidSurfaceMesh,
    face_probes: &[u32],
    texture_dimensions: &[[u32; 2]],
    material_count: usize,
) -> (Vec<FluidCellVertex>, Option<FluidCellLayout>) {
    let mut vertices = vec![FluidCellVertex::default(); mesh.vertices.len()];
    let mut slot_of_volume = vec![None; volumes.len()];
    for (slot, volume) in mesh.volumes.iter().enumerate() {
        if let Some(entry) = slot_of_volume.get_mut(volume.volume_index as usize) {
            *entry = Some(slot as u32);
        }
    }
    let mut faces = Vec::new();
    // Mesh vertex range per rect face, for the per-vertex attribute.
    let mut ranges = Vec::new();
    let mut slot_capable = vec![true; mesh.volumes.len()];
    let mut global = 0usize;
    for (volume_index, volume) in volumes.iter().enumerate() {
        for face in &volume.surface_faces {
            let face_global = global;
            global += 1;
            let (Some(Some(map)), Some(&probe), Some(Some(slot))) = (
                mesh.face_drift_maps.get(face_global),
                face_probes.get(face_global),
                slot_of_volume.get(volume_index),
            ) else {
                continue;
            };
            if probe == FLUID_REFLECTION_NO_PROBE {
                continue;
            }
            let slot = *slot;
            let rect = (|| {
                // The shader keys cells on `uv * dims` of the bound texture;
                // the record's texel UV is that only when the face's own
                // texture is bound and its size known.
                let index = face.texture_index as usize;
                let &[width, height] = texture_dimensions.get(index)?;
                if width == 0
                    || height == 0
                    || material_index(face.texture_index, material_count) != index
                {
                    return None;
                }
                let outward = face_normal(face)?;
                let (p0, uv0, a, b) = fit_texel_map(face)?;
                let (mut lo, mut hi) =
                    (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY));
                for vertex in &face.vertices {
                    let uv = DVec2::new(f64::from(vertex.uv[0]), f64::from(vertex.uv[1]));
                    lo = lo.min(uv);
                    hi = hi.max(uv);
                }
                let min = (lo + DVec2::splat(FLUID_CELL_RECT_EPSILON)).floor();
                let max = (hi - DVec2::splat(FLUID_CELL_RECT_EPSILON)).floor();
                let count = max - min + DVec2::ONE;
                if !(count.x >= 1.0 && count.y >= 1.0)
                    || count.x > f64::from(FLUID_CELL_ATLAS_MAX)
                    || count.y > f64::from(FLUID_CELL_ATLAS_MAX)
                    || min.x.abs() > 1.0e7
                    || min.y.abs() > 1.0e7
                {
                    return None;
                }
                let origin = p0 + a * (min.x - uv0.x) + b * (min.y - uv0.y);
                Some(CellFace {
                    slot,
                    probe,
                    outward,
                    origin: origin.as_vec3(),
                    axis_u: a.as_vec3(),
                    axis_v: b.as_vec3(),
                    cell_min: [min.x as i32, min.y as i32],
                    cell_count: [count.x as u32, count.y as u32],
                    atlas_origin: [0, 0],
                })
            })();
            match rect {
                Some(rect) => {
                    faces.push(rect);
                    ranges.push((map.vertex_start as usize, map.vertex_count as usize));
                }
                None => slot_capable[slot as usize] = false,
            }
        }
    }
    if faces.is_empty() {
        return (vertices, None);
    }
    if faces.len() > FLUID_CELL_MAX_FACES {
        log::warn!(
            "[Renderer] Fluid reflection cells disabled: {} rect faces exceed the {} cap",
            faces.len(),
            FLUID_CELL_MAX_FACES
        );
        return (vertices, None);
    }
    // Shelf pack, tallest first.
    let mut order: Vec<usize> = (0..faces.len()).collect();
    order.sort_by_key(|&index| std::cmp::Reverse(faces[index].cell_count[1]));
    let cells_total: u64 = faces.iter().map(CellFace::cells).sum();
    let widest = faces
        .iter()
        .map(|face| face.cell_count[0])
        .max()
        .unwrap_or(1);
    let square = ((cells_total as f64).sqrt().ceil() as u32).div_ceil(64) * 64;
    let width = widest.max(square).max(1);
    let (mut x, mut y, mut shelf) = (0u32, 0u32, 0u32);
    for &index in &order {
        let [w, h] = faces[index].cell_count;
        if x + w > width {
            y += shelf;
            x = 0;
            shelf = 0;
        }
        faces[index].atlas_origin = [x, y];
        x += w;
        shelf = shelf.max(h);
    }
    let height = y + shelf;
    if width > FLUID_CELL_ATLAS_MAX || height > FLUID_CELL_ATLAS_MAX {
        log::warn!(
            "[Renderer] Fluid reflection cells disabled: atlas {width}x{height} exceeds \
             {FLUID_CELL_ATLAS_MAX}"
        );
        return (vertices, None);
    }
    let mut slot_faces = vec![Vec::new(); mesh.volumes.len()];
    for (index, (face, &(start, count))) in faces.iter().zip(&ranges).enumerate() {
        slot_faces[face.slot as usize].push(index as u32);
        let offset = [
            face.atlas_origin[0] as i32 - face.cell_min[0],
            face.atlas_origin[1] as i32 - face.cell_min[1],
        ];
        let bounds = [
            face.cell_min[0],
            face.cell_min[1],
            face.cell_min[0] + face.cell_count[0] as i32 - 1,
            face.cell_min[1] + face.cell_count[1] as i32 - 1,
        ];
        if let Some(range) = vertices.get_mut(start..start + count) {
            range.fill(FluidCellVertex { offset, bounds });
        }
    }
    let tiles_total = faces
        .iter()
        .map(|face| {
            let [tx, ty] = face.tiles();
            tx * ty
        })
        .sum();
    for (slot, capable) in slot_capable.iter_mut().enumerate() {
        *capable &= !slot_faces[slot].is_empty();
    }
    log::info!(
        "[Renderer] Fluid reflection cells: {} faces with rects, {} cells, atlas {}x{} ({} B)",
        faces.len(),
        cells_total,
        width,
        height,
        u64::from(width) * u64::from(height) * FLUID_CELL_TEXEL_BYTES,
    );
    (
        vertices,
        Some(FluidCellLayout {
            faces,
            slot_faces,
            slot_capable,
            atlas: [width, height],
            cells_total,
            tiles_total,
        }),
    )
}

/// The compute pass's group 4: the probe entries, compute-visible.
pub(crate) fn cell_probe_bind_group_layout_entries() -> [wgpu::BindGroupLayoutEntry; 3] {
    fluid_probe_bind_group_layout_entries().map(|mut entry| {
        entry.visibility = wgpu::ShaderStages::COMPUTE;
        entry
    })
}

/// The compute pass's group 5: params, jobs and the atlas storage view.
pub(crate) fn cell_bind_group_layout_entries() -> [wgpu::BindGroupLayoutEntry; 3] {
    [
        wgpu::BindGroupLayoutEntry {
            binding: FLUID_CELL_PARAMS_BINDING,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: NonZeroU64::new(FLUID_CELL_PARAMS_BYTES),
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: FLUID_CELL_JOBS_BINDING,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: false },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: FLUID_CELL_ATLAS_OUT_BINDING,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: FLUID_CELL_ATLAS_FORMAT,
                view_dimension: wgpu::TextureViewDimension::D2,
            },
            count: None,
        },
    ]
}

/// The render layout's group 4 binding 4: the cell atlas, FRAGMENT.
pub(crate) fn cell_atlas_bind_group_layout_entry() -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding: FLUID_CELL_ATLAS_BINDING,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

/// A 1×1 atlas stand-in for a level without cells.
pub(crate) fn create_dummy_atlas(device: &wgpu::Device) -> (wgpu::Texture, wgpu::TextureView) {
    let texture =
        device.create_texture(&atlas_descriptor("Fluid Reflection Cell Atlas Dummy", 1, 1));
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

fn atlas_descriptor(
    label: &'static str,
    width: u32,
    height: u32,
) -> wgpu::TextureDescriptor<'static> {
    wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FLUID_CELL_ATLAS_FORMAT,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    }
}

/// One level's cell atlas, buffers, compute bind groups and per-frame
/// scratch.
pub(crate) struct FluidCellResources {
    layout: FluidCellLayout,
    _atlas: wgpu::Texture,
    atlas_view: wgpu::TextureView,
    params_buffer: wgpu::Buffer,
    jobs_buffer: wgpu::Buffer,
    probe_bind_group: wgpu::BindGroup,
    cell_bind_group: wgpu::BindGroup,
    face_scratch: Vec<FluidCellFaceGpu>,
    tile_scratch: Vec<[u32; 2]>,
    /// Per slot: this frame's draw reads cells.
    slot_cells: Vec<bool>,
    frame_tiles: u32,
    frame_faces: u32,
    frame_cells: u64,
}

impl FluidCellResources {
    pub(crate) fn new(
        device: &wgpu::Device,
        layout: FluidCellLayout,
        probe_entries: [wgpu::BindGroupEntry<'_>; 3],
        probe_bgl: &wgpu::BindGroupLayout,
        cell_bgl: &wgpu::BindGroupLayout,
    ) -> Self {
        let [width, height] = layout.atlas;
        let atlas = device.create_texture(&atlas_descriptor(
            "Fluid Reflection Cell Atlas",
            width,
            height,
        ));
        let atlas_view = atlas.create_view(&wgpu::TextureViewDescriptor::default());
        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Fluid Reflection Cell Params"),
            size: FLUID_CELL_PARAMS_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let jobs_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Fluid Reflection Cell Jobs"),
            contents: &vec![
                0u8;
                (FLUID_CELL_JOBS_HEADER_BYTES + 8 * u64::from(layout.tiles_total.max(1)))
                    as usize
            ],
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
        });
        let probe_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Fluid Reflection Cell Probe Bind Group (compute group 4)"),
            layout: probe_bgl,
            entries: &probe_entries,
        });
        let cell_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Fluid Reflection Cell Bind Group (compute group 5)"),
            layout: cell_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: FLUID_CELL_PARAMS_BINDING,
                    resource: params_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: FLUID_CELL_JOBS_BINDING,
                    resource: jobs_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: FLUID_CELL_ATLAS_OUT_BINDING,
                    resource: wgpu::BindingResource::TextureView(&atlas_view),
                },
            ],
        });
        let slots = layout.slot_capable.len();
        Self {
            face_scratch: Vec::with_capacity(layout.faces.len()),
            tile_scratch: Vec::with_capacity(layout.tiles_total as usize),
            slot_cells: vec![false; slots],
            layout,
            _atlas: atlas,
            atlas_view,
            params_buffer,
            jobs_buffer,
            probe_bind_group,
            cell_bind_group,
            frame_tiles: 0,
            frame_faces: 0,
            frame_cells: 0,
        }
    }

    pub(crate) fn atlas_view(&self) -> &wgpu::TextureView {
        &self.atlas_view
    }

    /// Whether `slot`'s draw reads cells this frame.
    pub(crate) fn slot_reads_cells(&self, slot: u32) -> bool {
        self.slot_cells.get(slot as usize).copied().unwrap_or(false)
    }

    pub(crate) fn frame_tiles(&self) -> u32 {
        self.frame_tiles
    }

    /// Clear this frame's cell draws (cells inactive).
    pub(crate) fn clear_frame(&mut self) {
        self.slot_cells.fill(false);
        self.frame_tiles = 0;
        self.frame_faces = 0;
        self.frame_cells = 0;
    }

    /// Build and upload this frame's face and tile lists: every rect face of
    /// each reflecting, cell-capable draw seen from above.
    pub(crate) fn prepare(
        &mut self,
        queue: &crate::render::uploads::UploadQueue,
        slots: &FluidSurfaceSlots,
        light_term_mask: u32,
        camera: Vec3,
        viewport: [u32; 2],
        levers: FluidCellLevers,
    ) {
        self.clear_frame();
        self.face_scratch.clear();
        self.tile_scratch.clear();
        for (slot, variant) in slots.frame_draws(light_term_mask) {
            if variant != FluidShaderVariant::Reflecting || !self.layout.slot_capable[slot as usize]
            {
                continue;
            }
            self.slot_cells[slot as usize] = true;
            let reflectivity = slots.row(slot).reflectivity();
            if reflectivity <= 0.0 {
                continue;
            }
            for &index in &self.layout.slot_faces[slot as usize] {
                let face = &self.layout.faces[index as usize];
                if face.outward.dot(camera - face.origin) <= -FLUID_CELL_ABOVE_SLACK {
                    continue;
                }
                let face_index = self.face_scratch.len() as u32;
                self.face_scratch.push(FluidCellFaceGpu {
                    origin: face.origin.to_array(),
                    probe: face.probe,
                    axis_u: face.axis_u.to_array(),
                    reflectivity,
                    axis_v: face.axis_v.to_array(),
                    outward: face.outward.to_array(),
                    cell_min: face.cell_min,
                    cell_count: face.cell_count,
                    atlas_origin: face.atlas_origin,
                    ..Default::default()
                });
                let [tx, ty] = face.tiles();
                for y in 0..ty {
                    for x in 0..tx {
                        self.tile_scratch.push([face_index, (y << 16) | x]);
                    }
                }
                self.frame_cells += face.cells();
            }
        }
        self.frame_faces = self.face_scratch.len() as u32;
        self.frame_tiles = self.tile_scratch.len() as u32;
        if self.frame_tiles == 0 {
            if levers.count {
                queue.write_buffer(&self.jobs_buffer, 0, bytemuck::cast_slice(&[0u32; 4]));
            }
            return;
        }
        let header = FluidCellParamsHeader {
            viewport: [viewport[0] as f32, viewport[1] as f32],
            flags: if levers.cull { FLUID_CELL_FLAG_CULL } else { 0 }
                | if levers.count {
                    FLUID_CELL_FLAG_COUNT
                } else {
                    0
                },
            face_count: self.frame_faces,
        };
        queue.write_buffer(&self.params_buffer, 0, bytemuck::bytes_of(&header));
        queue.write_buffer(
            &self.params_buffer,
            FLUID_CELL_PARAMS_HEADER_BYTES,
            bytemuck::cast_slice(&self.face_scratch),
        );
        queue.write_buffer(
            &self.jobs_buffer,
            0,
            bytemuck::cast_slice(&[0u32, self.frame_tiles, 0, 0]),
        );
        queue.write_buffer(
            &self.jobs_buffer,
            FLUID_CELL_JOBS_HEADER_BYTES,
            bytemuck::cast_slice(&self.tile_scratch),
        );
    }

    /// Record the cell compute pass for this frame's tiles.
    pub(crate) fn record(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pipeline: &wgpu::ComputePipeline,
        camera_bind_group: &wgpu::BindGroup,
        sh_bind_group: &wgpu::BindGroup,
        timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>,
    ) {
        if self.frame_tiles == 0 {
            return;
        }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Fluid Reflection Cell Pass"),
            timestamp_writes,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, camera_bind_group, &[]);
        pass.set_bind_group(3, sh_bind_group, &[]);
        pass.set_bind_group(4, &self.probe_bind_group, &[]);
        pass.set_bind_group(5, &self.cell_bind_group, &[]);
        let x = self.frame_tiles.min(FLUID_CELL_DISPATCH_ROW);
        let y = self.frame_tiles.div_ceil(FLUID_CELL_DISPATCH_ROW);
        pass.dispatch_workgroups(x, y, 1);
    }

    /// Copy the jobs header (evaluated, tile count) into `target` at 0.
    pub(crate) fn copy_counter(&self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::Buffer) {
        encoder.copy_buffer_to_buffer(&self.jobs_buffer, 0, target, 0, 8);
    }

    /// The last frame's dispatch figures and the layout's atlas, before the
    /// GPU counters fill in.
    pub(crate) fn stats(&self) -> FluidCellStats {
        let [width, height] = self.layout.atlas;
        FluidCellStats {
            faces_dispatched: self.frame_faces,
            cells_dispatched: self.frame_cells,
            atlas_width: width,
            atlas_height: height,
            atlas_bytes: u64::from(width) * u64::from(height) * FLUID_CELL_TEXEL_BYTES,
            cells_total: self.layout.cells_total,
            ..Default::default()
        }
    }
}
