//! Fixtures for the glossy overlay's orientation gates: a room of three glossy
//! world faces (a floor, the north wall and a down-facing ceiling), each bound
//! to its own probe, whose SH volume's animation descriptors are the level's
//! light roster; materials of any texel size, glow and Surface Depth; and the
//! CPU reference for the overlay's addition at a pixel, built from render-cpu
//! `glossy_reflection`.
//! See: context/lib/rendering_pipeline.md §7.3a (Glossy world faces, Live lighting)

use glam::{UVec2, Vec2, Vec3, Vec4};
use postretro_level_format::area_glow::AreaGlowLight;
use postretro_level_format::emissive::EmissiveVolumeRecord;
use postretro_level_format::geometry::Vertex;
use postretro_level_format::lightmap::{IRRADIANCE_FORMAT_RGBA16F, f32_to_f16_bits};
use postretro_level_format::octahedral::{
    irradiance_interior_texel_direction, irradiance_tile_source_texel,
};
use postretro_level_format::reflection_probes::{
    REFLECTION_PROBE_DISTANCE_F16_INVERSE_L1, REFLECTION_PROBE_LIGHTS_PER_PROBE,
    REFLECTION_PROBE_LOBE_COUNT, ReflectionProbeLightShare, ReflectionProbeRecord,
    ReflectionProbeWorldBinding, ReflectionProbesSection, reflection_probe_zero_distances,
};
use postretro_level_format::sh_volume::{AnimationDescriptor, OctahedralShVolumeSection};
use postretro_level_loader::CellData;
use postretro_render_cpu::fluid_surface::reflection::{
    FluidProbeGpu, FluidReflectionLights, REFLECTION_SCREEN_MISS, reflection_probe_sums,
};
use postretro_render_cpu::fluid_surface::reflection_hemisphere::reflection_hemisphere_axis;
use postretro_render_cpu::glossy_reflection::{
    glossy_composite, glossy_fresnel, glossy_probe_term, glossy_texel,
};
use postretro_render_cpu::reflection_probe_mips::reflection_probe_mip_colour;
use postretro_render_cpu::sh_compose::{AnimatedLightScaleDescriptor, f16_bits_to_f32};
use postretro_render_data::geometry::{BVH_NODE_FLAG_LEAF, BvhLeaf, BvhNode, BvhTree, WorldVertex};
use postretro_render_data::material::{Material, TextureMaterial};

use super::fluid_baked_frame_test_fixtures::sh_section;
use super::fluid_reflection_frame_test_fixtures::{FRAME, View, centre, read_scene, view_from};
use super::fluid_reflection_sums_frame_test_fixtures::frame_at;
use super::glossy_overlay_frame_test_fixtures::{GlossyFrame, read_static_diffuse};
use super::{LevelGeometry, LevelGeometryShStorage, Renderer};

/// The room's interior: x across, y up, z away from the eye.
pub(super) const ROOM_MIN: Vec3 = Vec3::new(-3.0, 0.0, 1.0);
pub(super) const ROOM_MAX: Vec3 = Vec3::new(3.0, 3.0, 9.0);
/// The capture point's lift off its face, as the compiler lifts it.
const LIFT: f32 = 0.002;
/// The neutral normal map's R and G byte (`NEUTRAL_NORMAL_PIXEL`).
pub(super) const NEUTRAL_NORMAL_BYTE: u8 = 127;

/// One glossy face of the room, each its own face index, leaf and probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Face {
    Floor,
    Wall,
    Ceiling,
}

impl Face {
    pub const ALL: [Face; 3] = [Face::Floor, Face::Wall, Face::Ceiling];

    pub fn index(self) -> usize {
        self as usize
    }

    /// The face's outward unit normal, into the room.
    pub fn normal(self) -> Vec3 {
        match self {
            Face::Floor => Vec3::Y,
            Face::Wall => Vec3::NEG_Z,
            Face::Ceiling => Vec3::NEG_Y,
        }
    }

    /// The face's corners, in UV order (0,0), (1,0), (1,1), (0,1).
    fn corners(self) -> [Vec3; 4] {
        let (min, max) = (ROOM_MIN, ROOM_MAX);
        match self {
            Face::Floor => [
                Vec3::new(min.x, min.y, min.z),
                Vec3::new(max.x, min.y, min.z),
                Vec3::new(max.x, min.y, max.z),
                Vec3::new(min.x, min.y, max.z),
            ],
            Face::Wall => [
                Vec3::new(min.x, max.y, max.z),
                Vec3::new(max.x, max.y, max.z),
                Vec3::new(max.x, min.y, max.z),
                Vec3::new(min.x, min.y, max.z),
            ],
            Face::Ceiling => [
                Vec3::new(min.x, max.y, min.z),
                Vec3::new(max.x, max.y, min.z),
                Vec3::new(max.x, max.y, max.z),
                Vec3::new(min.x, max.y, max.z),
            ],
        }
    }

    /// World step per unit of u and of v: u runs +X across the face.
    fn axes(self) -> (Vec3, Vec3) {
        let c = self.corners();
        (c[1] - c[0], c[3] - c[0])
    }

    /// The world point on the face at `uv`.
    pub fn at_uv(self, uv: Vec2) -> Vec3 {
        let (u, v) = self.axes();
        self.corners()[0] + u * uv.x + v * uv.y
    }

    /// The face's UV at world point `p` on it.
    pub fn uv(self, p: Vec3) -> Vec2 {
        let (u, v) = self.axes();
        let d = p - self.corners()[0];
        Vec2::new(d.dot(u) / u.length_squared(), d.dot(v) / v.length_squared())
    }

    /// Where its probe captured from: 2 mm in front of the face's centre.
    pub fn capture_point(self) -> Vec3 {
        let c = self.corners();
        (c[0] + c[2]) * 0.5 + self.normal() * LIFT
    }

    /// The world point of this face under `pixel`'s centre, at least
    /// `margin` metres inside its edges, when the eye sees this face there
    /// (no other face nearer along the ray).
    pub fn point_at(self, view: View, pixel: UVec2, margin: f32) -> Option<Vec3> {
        let hit = |face: Face| {
            let (origin, dir) = ray_through(view, centre(pixel));
            let c = face.corners();
            let t = (c[0] - origin).dot(face.normal()) / dir.dot(face.normal());
            (t > 0.0 && t.is_finite()).then(|| (t, origin + dir * t))
        };
        let (t, point) = hit(self)?;
        let uv = self.uv(point);
        let (u, v) = self.axes();
        let inside = |x: f32, length: f32| x * length > margin && (1.0 - x) * length > margin;
        if !(inside(uv.x, u.length()) && inside(uv.y, v.length())) {
            return None;
        }
        let nearer = Face::ALL.iter().any(|&other| {
            other != self
                && hit(other).is_some_and(|(s, p)| {
                    let w = other.uv(p);
                    s < t + 0.05 && (0.0..=1.0).contains(&w.x) && (0.0..=1.0).contains(&w.y)
                })
        });
        (!nearer).then_some(point)
    }

    /// Its probe's lookup hemisphere axis, as the overlay takes it from the
    /// face's normal.
    pub fn axis(self) -> Vec3 {
        reflection_hemisphere_axis(self.normal())
    }

    /// The vertex the room's buffer holds at `corner`.
    fn vertex(self, corner: Vec3) -> WorldVertex {
        let (u, v) = self.axes();
        let normal = self.normal();
        // The bitangent sign makes cross(N, T) × sign run along +v.
        let positive = normal.cross(u.normalize()).dot(v) > 0.0;
        let packed = Vertex::new(
            corner.to_array(),
            self.uv(corner).to_array(),
            normal.to_array(),
            u.normalize().to_array(),
            positive,
            [0.0; 2],
            0,
        );
        WorldVertex {
            position: packed.position,
            base_uv: packed.uv,
            normal_oct: packed.normal_oct,
            tangent_packed: packed.tangent_packed,
            lightmap_uv: [0; 2],
            lightmap_block: 0,
            animated_block: 0,
        }
    }

    /// Forward's shading normal on this face over the neutral normal map:
    /// `reconstruct_tbn_normal` of the decoded normal, tangent and sign.
    pub fn shading_normal(self) -> Vec3 {
        let (u, v) = self.axes();
        let packed = Vertex::new(
            [0.0; 3],
            [0.0; 2],
            self.normal().to_array(),
            u.normalize().to_array(),
            self.normal().cross(u.normalize()).dot(v) > 0.0,
            [0.0; 2],
            0,
        );
        let n = Vec3::from(packed.decode_normal()).normalize();
        let t = Vec3::from(packed.decode_tangent());
        let sign = if packed.bitangent_sign() { 1.0 } else { -1.0 };
        let rg = f32::from(NEUTRAL_NORMAL_BYTE) / 255.0 * 2.0 - 1.0;
        let n_ts = Vec3::new(rg, rg, (1.0 - 2.0 * rg * rg).max(0.0).sqrt()).normalize();
        let t = (t - n * t.dot(n)).normalize();
        let b = n.cross(t) * sign;
        (t * n_ts.x + b * n_ts.y + n * n_ts.z).normalize()
    }
}

/// The view ray through screen position `at` (FRAME pixels): origin, unit
/// direction.
pub(super) fn ray_through(view: View, at: Vec2) -> (Vec3, Vec3) {
    let ndc = Vec2::new(
        at.x / FRAME as f32 * 2.0 - 1.0,
        1.0 - at.y / FRAME as f32 * 2.0,
    );
    let inverse = view.view_proj.inverse();
    let near = inverse.project_point3(ndc.extend(0.0));
    let far = inverse.project_point3(ndc.extend(1.0));
    (near, (far - near).normalize())
}

/// The room view: from the room's near end at standing height, looking at
/// the north wall's centre, with floor and ceiling in view.
pub(super) fn room_view() -> View {
    view_from(Vec3::new(0.0, 1.5, 2.0), Vec3::new(0.0, 1.5, 9.0))
}

/// Looking down at the floor from the room's near end, so floor texels near
/// the eye reflect the ceiling straight above the floor's capture point.
pub(super) fn room_floor_view() -> View {
    view_from(Vec3::new(0.0, 1.5, 2.0), Vec3::new(0.0, 0.0, 4.5))
}

/// The SH lattice every room carries, only so its animation descriptors are
/// the level's roster: one probe over the whole room.
const SH_ORIGIN: [f32; 3] = [-4.0, -1.0, 0.0];
const SH_CELL: [f32; 3] = [8.0, 5.0, 10.0];

/// A probe colour as a function of direction from its capture point, and
/// the blur each further mip spreads it by (a texel's direction pulled
/// toward `+Y` and `-Y` alike would read a wider cap), as a synthetic
/// prefilter: mip `m` takes `colour(direction, m)`.
pub(super) type TileColour = fn(Vec3, u32) -> Vec3;

/// A room level: its light roster, and per face (floor, wall, ceiling) its
/// probe's stored lights, capture colour and glow id.
#[derive(Clone)]
pub(super) struct Room {
    pub roster: Vec<AnimationDescriptor>,
    pub lights: [Vec<ReflectionProbeLightShare>; 3],
    pub colours: [TileColour; 3],
    pub tile: u32,
    pub mips: u32,
    /// Each face's glow id (0 none, volume index + 1, 257 + area entry).
    pub glow_ids: [u32; 3],
    pub emissive_volumes: Vec<EmissiveVolumeRecord>,
    pub area_lights: Vec<AreaGlowLight>,
}

impl Room {
    /// A room whose three probes capture `colours` with one mip of a 16²
    /// tile, store no light, and whose faces glow nothing.
    pub fn new(colours: [TileColour; 3]) -> Self {
        Self {
            roster: Vec::new(),
            lights: [Vec::new(), Vec::new(), Vec::new()],
            colours,
            tile: 16,
            mips: 1,
            glow_ids: [0; 3],
            emissive_volumes: Vec::new(),
            area_lights: Vec::new(),
        }
    }

    /// Face `face`'s probe colour mips, border included, f16-rounded, as
    /// the GPU holds them.
    pub fn mip_tiles(&self, face: Face) -> Vec<Vec<Vec3>> {
        let colour = self.colours[face.index()];
        (0..self.mips)
            .map(|mip| {
                let dimension = self.tile >> mip;
                (0..dimension)
                    .flat_map(|y| (0..dimension).map(move |x| (x, y)))
                    .map(|(x, y)| {
                        let [ix, iy] = irradiance_tile_source_texel(x, y, dimension, 1);
                        let direction =
                            Vec3::from(irradiance_interior_texel_direction(ix, iy, dimension, 1));
                        colour(direction, mip)
                            .to_array()
                            .map(|c| f16_bits_to_f32(f32_to_f16_bits(c)))
                            .into()
                    })
                    .collect()
            })
            .collect()
    }

    /// Face `face`'s probe record.
    pub fn probe(&self, face: Face) -> ReflectionProbeRecord {
        let lights = &self.lights[face.index()];
        assert!(lights.len() <= REFLECTION_PROBE_LIGHTS_PER_PROBE);
        let mut stored = [ReflectionProbeLightShare::UNUSED; REFLECTION_PROBE_LIGHTS_PER_PROBE];
        stored[..lights.len()].copy_from_slice(lights);
        ReflectionProbeRecord {
            source_entity: face.index() as u32,
            box_min: ROOM_MIN.to_array(),
            box_max: ROOM_MAX.to_array(),
            capture_point: face.capture_point().to_array(),
            layer: face.index() as u32,
            lights: stored,
        }
    }

    /// Id 54: one probe per face, each face bound to its own.
    pub fn section(&self) -> ReflectionProbesSection {
        let tiles = Face::ALL
            .iter()
            .flat_map(|&face| self.mip_tiles(face).into_iter().flatten())
            .flat_map(|texel| {
                [texel.x, texel.y, texel.z, 1.0]
                    .map(f32_to_f16_bits)
                    .into_iter()
                    .flat_map(u16::to_le_bytes)
            })
            .collect();
        ReflectionProbesSection {
            tile_dimension: self.tile,
            tile_border: 1,
            atlas_format: IRRADIANCE_FORMAT_RGBA16F,
            colour_mip_count: self.mips,
            world_bindings: Face::ALL
                .iter()
                .map(|&face| ReflectionProbeWorldBinding {
                    face: face.index() as u32,
                    probe: face.index() as u32,
                })
                .collect(),
            probes: Face::ALL.iter().map(|&face| self.probe(face)).collect(),
            fluid_bindings: Vec::new(),
            tiles,
            distance_format: REFLECTION_PROBE_DISTANCE_F16_INVERSE_L1,
            distances: reflection_probe_zero_distances(Face::ALL.len(), self.tile),
        }
    }

    fn sh(&self) -> OctahedralShVolumeSection {
        let mut sh = sh_section(SH_ORIGIN, SH_CELL, [1, 1, 1], |_| Vec3::splat(0.2));
        sh.animation_descriptors = self.roster.clone();
        sh
    }

    /// Install the room on the main thread, then its probes (no fluid), with
    /// `material` on every face.
    pub fn install(&self, renderer: &mut Renderer, material: &RoomMaterial) {
        let section = self.section();
        let sh = self.sh();
        self.with_geometry(&section, &sh, |geometry| {
            renderer
                .install_level_geometry(geometry, Default::default())
                .expect("level resources install");
        });
        renderer.install_fluid_surfaces(&[], Some(&section));
        assert_eq!(
            renderer.animated_compose_descriptor_count(),
            self.roster.len() as u32,
            "the room's roster installs"
        );
        set_room_material(renderer, material);
    }

    fn with_geometry<R>(
        &self,
        section: &ReflectionProbesSection,
        sh: &OctahedralShVolumeSection,
        use_geometry: impl FnOnce(&LevelGeometry<'_>) -> R,
    ) -> R {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let mut leaves = Vec::new();
        let mut glow = Vec::new();
        for face in Face::ALL {
            let base = vertices.len() as u32;
            let corners = face.corners();
            vertices.extend(corners.map(|corner| face.vertex(corner)));
            glow.extend([self.glow_ids[face.index()]; 4]);
            let first = indices.len() as u32;
            // Both windings: back-face culling keeps the one facing the room.
            indices.extend([0, 1, 2, 0, 2, 3, 0, 2, 1, 0, 3, 2].map(|index| base + index));
            let (lo, hi) = corners.iter().fold(
                (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
                |(lo, hi), c| (lo.min(*c), hi.max(*c)),
            );
            leaves.push(BvhLeaf {
                aabb_min: (lo - 0.1).to_array(),
                aabb_max: (hi + 0.1).to_array(),
                material_bucket_id: 0,
                index_offset: first,
                index_count: 12,
                cell_id: 0,
                chunk_range_start: 0,
                chunk_range_count: 0,
            });
        }
        let (aabb_min, aabb_max) = ((ROOM_MIN - 0.1).to_array(), (ROOM_MAX + 0.1).to_array());
        let mut nodes = vec![BvhNode {
            aabb_min,
            aabb_max,
            skip_index: leaves.len() as u32 + 1,
            left_child_or_leaf_index: 1,
            flags: 0,
        }];
        nodes.extend(leaves.iter().enumerate().map(|(slot, leaf)| BvhNode {
            aabb_min: leaf.aabb_min,
            aabb_max: leaf.aabb_max,
            skip_index: slot as u32 + 2,
            left_child_or_leaf_index: slot as u32,
            flags: BVH_NODE_FLAG_LEAF,
        }));
        let bvh = BvhTree {
            nodes,
            leaves,
            root_node_index: 0,
        };
        let cells = [CellData {
            bounds_min: Vec3::from(aabb_min),
            bounds_max: Vec3::from(aabb_max),
            face_start: 0,
            face_count: Face::ALL.len() as u32,
            portal_ref_start: 0,
            portal_ref_count: 0,
            is_solid: false,
            is_exterior: false,
            is_drawable: true,
        }];
        use_geometry(&LevelGeometry {
            vertices: &vertices,
            vertex_spec_owners: &[],
            vertex_emissive_volumes: &glow,
            reflection_probes: Some(section),
            emissive_volumes: &self.emissive_volumes,
            area_glow_lights: &self.area_lights,
            indices: &indices,
            bvh: &bvh,
            lights: &[],
            light_influences: &[],
            sh_volume: Some(sh),
            sh_storage: LevelGeometryShStorage::Legacy,
            probe_depth_tiles: None,
            lightmap: None,
            lightmap_streaming: None,
            chunk_light_list: None,
            animated_light_chunks: None,
            animated_light_weight_maps: None,
            delta_sh_volumes: None,
            direct_sh_volume: None,
            direct_sh_delta_volumes: None,
            animated_direct_sh_delta_volumes: None,
            billboard_direct_scatter_volume: None,
            animated_billboard_direct_scatter_delta_volumes: None,
            mover_probe_channel: None,
            entity_shadow_lights: &[],
            shadowmask_atlas: None,
            sdf_atlas: None,
            lightmap_mode: postretro_level_loader::LightmapMode::Shadowed,
            cell_draw_index: None,
            kinematic_geometry: None,
            cells: &cells,
            texture_materials: &[],
        })
    }

    /// Every probe's sums from the CPU reference over `roster` (the
    /// descriptors as the GPU holds them) at animation time `time`.
    pub fn cpu_sums(
        &self,
        roster: &[AnimationDescriptor],
        time: f32,
    ) -> Vec<[Vec3; REFLECTION_PROBE_LOBE_COUNT]> {
        let descriptors: Vec<_> = roster
            .iter()
            .map(|light| AnimatedLightScaleDescriptor {
                period: light.period,
                phase: light.phase,
                base_color: light.base_color,
                brightness: &light.brightness,
                color: &light.color,
                is_active: light.start_active != 0,
            })
            .collect();
        let lights = FluidReflectionLights {
            descriptors: &descriptors,
            time,
        };
        Face::ALL
            .iter()
            .map(|&face| {
                reflection_probe_sums(&FluidProbeGpu::pack(&self.probe(face)).lights, &lights)
            })
            .collect()
    }
}

/// One texture's RGBA8 bytes at texel `(x, y)`.
pub(super) type TexelFn = fn(u32, u32) -> [u8; 4];

/// A material for every room face: `size`² texels of albedo, specular slot
/// (R `_s`, G height, B smoothness, A tint) and glow, sRGB albedo and glow,
/// under the prefix `material` (its Surface Depth tuning) and glow strength.
#[derive(Clone, Copy)]
pub(super) struct RoomMaterial {
    pub size: u32,
    pub albedo: TexelFn,
    pub slot: TexelFn,
    pub glow: Option<(TexelFn, f32)>,
    pub material: Material,
}

impl RoomMaterial {
    /// One white texel at full `_s`, the exponent cap and no tint: the
    /// reflection is uncoloured and weighs water's Fresnel at 0.04 alone.
    pub const WHITE_SHARP: Self = Self {
        size: 1,
        albedo: |_, _| [255; 4],
        slot: |_, _| [255, 127, 255, 255],
        glow: None,
        material: Material::Default,
    };

    /// [`Self::WHITE_SHARP`] at full tint: F0 = 1, so W = 1 at every angle
    /// and the reflection replaces D_static outright.
    pub const WHITE_MIRROR: Self = Self {
        slot: |_, _| [255, 127, 255, 254],
        ..Self::WHITE_SHARP
    };

    fn texels(&self, texel: TexelFn) -> Vec<u8> {
        (0..self.size)
            .flat_map(|y| (0..self.size).map(move |x| (x, y)))
            .flat_map(|(x, y)| texel(x, y))
            .collect()
    }

    /// The texel's slot sample at texel `(x, y)`, as unorm.
    pub fn slot_sample(&self, x: u32, y: u32) -> Vec4 {
        Vec4::from((self.slot)(x, y).map(|byte| f32::from(byte) / 255.0))
    }
}

/// Replace bucket 0's material with `material`, marked glossy as a material
/// whose specular slot passes the opt-in would be, its Surface Depth relief
/// measured from the slot's height bytes and its uniform at the renderer's
/// Surface Depth setting.
pub(super) fn set_room_material(renderer: &mut Renderer, material: &RoomMaterial) {
    use super::loaded_texture::{placeholder_loaded_texture, upload_texture_data};
    use postretro_render_cpu::surface_depth::surface_relief_from_surface_map;
    let device = renderer.device.clone();
    let queue = renderer.queue.raw().clone();
    let size = material.size;
    let upload = |format, bytes: &[u8], label| {
        upload_texture_data(&device, &queue, format, &[(size, size, bytes)], label)
    };
    let mut texture = placeholder_loaded_texture(&device, &queue);
    (texture.diffuse_texture, texture.diffuse_view) = upload(
        wgpu::TextureFormat::Rgba8UnormSrgb,
        &material.texels(material.albedo),
        "glossy room albedo",
    );
    let slot = material.texels(material.slot);
    (texture.specular_texture, texture.specular_view) =
        upload(wgpu::TextureFormat::Rgba8Unorm, &slot, "glossy room slot");
    texture.surface_relief = surface_relief_from_surface_map(&[(size, size, &slot[..])], 4);
    let neutral = [NEUTRAL_NORMAL_BYTE, NEUTRAL_NORMAL_BYTE, 255, 255];
    (texture.normal_texture, texture.normal_view) = upload_texture_data(
        &device,
        &queue,
        wgpu::TextureFormat::Rgba8Unorm,
        &[(1, 1, &neutral[..])],
        "glossy room normal",
    );
    let mut texture_material = TextureMaterial {
        material: material.material,
        emissive_strength: 0.0,
    };
    if let Some((glow, strength)) = material.glow {
        (texture.emissive_texture, texture.emissive_view) = upload(
            wgpu::TextureFormat::Rgba8UnormSrgb,
            &material.texels(glow),
            "glossy room glow",
        );
        texture_material.emissive_strength = strength;
    }
    texture.reflects = true;
    let surface_depth_quality = renderer.surface_depth_quality;
    let full = renderer.full();
    let sampler = full
        .samplers
        .aniso
        .get(&1)
        .expect("the one-mip sampler is seeded")
        .clone();
    let binding = super::material_plan::build_material_bind_group(
        &device,
        &full.texture_bind_group_layout,
        &texture,
        &sampler,
        texture_material,
        surface_depth_quality,
        "Glossy Room Material",
    );
    let level = renderer.level_mut();
    level.loaded_textures[0] = texture;
    level.gpu_textures[0] = binding.into();
}

/// Run `frame` with the overlay's fragment `entry` from `source` (extra WGSL
/// composed after the overlay's own) in both style pipelines' place,
/// restored after.
pub(super) fn with_room_variant<R>(
    renderer: &mut Renderer,
    source: &str,
    entry: &str,
    frame: impl FnOnce(&mut Renderer) -> R,
) -> R {
    use super::glossy_overlay::GlossyOverlayPipelines;
    let variant = renderer
        .full()
        .glossy_overlay
        .variant_pipeline(&renderer.device, source, entry);
    let production = renderer
        .full_mut()
        .glossy_overlay
        .swap_pipelines(GlossyOverlayPipelines::both(variant));
    let result = frame(renderer);
    renderer
        .full_mut()
        .glossy_overlay
        .swap_pipelines(production);
    result
}

/// One frame through `view` at animation time `time`, read back.
pub(super) fn room_frame(renderer: &mut Renderer, view: View, time: f32) -> GlossyFrame {
    frame_at(renderer, view, time);
    GlossyFrame {
        scene: read_scene(renderer),
        static_diffuse: read_static_diffuse(renderer),
    }
}

/// The CPU reference for the overlay over a room in smooth style: the
/// probe term through the sums, the texel's weight and colour, and the
/// composite over forward's D_static.
pub(super) struct RoomReference<'a> {
    room: &'a Room,
    view: View,
    sums: Vec<[Vec3; REFLECTION_PROBE_LOBE_COUNT]>,
    material: RoomMaterial,
    /// Each face's probe colour mips, as the GPU holds them.
    mips: [Vec<Vec<Vec3>>; 3],
}

/// What the overlay adds at one pixel, and the reflection's own share of it.
pub(super) struct Addition {
    pub added: Vec3,
    pub reflected: Vec3,
    pub weight: f32,
}

impl<'a> RoomReference<'a> {
    /// The reference over `room` seen through `view`, its probes' sums over
    /// `roster` at animation time `time`, every face in `material`.
    pub fn new(
        room: &'a Room,
        view: View,
        roster: &[AnimationDescriptor],
        time: f32,
        material: RoomMaterial,
    ) -> Self {
        Self {
            room,
            view,
            sums: room.cpu_sums(roster, time),
            material,
            mips: Face::ALL.map(|face| room.mip_tiles(face)),
        }
    }

    /// The probe lookup at `point` on `face`, about the face's hemisphere,
    /// as the overlay's search ends (zero distances: box projection).
    pub fn lookup(&self, face: Face, point: Vec3) -> Vec3 {
        use postretro_render_cpu::fluid_surface::reflection::reflection_mirror;
        use postretro_render_cpu::fluid_surface::reflection_hemisphere::reflection_above_hemisphere;
        use postretro_render_cpu::fluid_surface::reflection_search::reflection_search_about;
        let view = (self.view.eye - point).normalize();
        let ray = reflection_above_hemisphere(
            reflection_mirror(view, face.shading_normal()),
            face.axis(),
        );
        reflection_search_about(
            point,
            ray,
            ROOM_MIN,
            ROOM_MAX,
            face.capture_point(),
            face.axis(),
            [self.room.tile, 1],
            |_| 0.0,
        )
        .lookup
    }

    /// The overlay's addition at `point` on `face` over D_static
    /// `static_diffuse`, the texel's albedo `albedo` (linear) at texel
    /// `(x, y)`, with the probe's hemisphere about `axis`.
    pub fn addition_about(
        &self,
        face: Face,
        point: Vec3,
        texel: (u32, u32),
        albedo: Vec3,
        static_diffuse: Vec3,
        axis: Vec3,
    ) -> Addition {
        let view = (self.view.eye - point).normalize();
        let texel = glossy_texel(self.material.slot_sample(texel.0, texel.1), albedo);
        let fresnel = glossy_fresnel(&texel, view.dot(face.normal()));
        let chain: Vec<&[Vec3]> = self.mips[face.index()].iter().map(Vec::as_slice).collect();
        let (capture, weight) = glossy_probe_term(
            &self.room.probe(face),
            &self.sums[face.index()],
            point,
            view,
            face.shading_normal(),
            axis,
            [self.room.tile, 1],
            |direction| {
                reflection_probe_mip_colour(&chain, self.room.tile, 1, direction, texel.exponent)
            },
            |_| 0.0,
        );
        Addition {
            added: glossy_composite(
                static_diffuse,
                capture,
                weight,
                REFLECTION_SCREEN_MISS,
                fresnel,
                texel.colour,
            ),
            reflected: glossy_composite(
                Vec3::ZERO,
                capture,
                weight,
                REFLECTION_SCREEN_MISS,
                fresnel,
                texel.colour,
            ),
            weight: fresnel * weight,
        }
    }

    /// [`Self::addition_about`] about the face's own hemisphere, for a
    /// one-texel white material.
    pub fn addition(&self, face: Face, point: Vec3, static_diffuse: Vec3) -> Addition {
        self.addition_about(face, point, (0, 0), Vec3::ONE, static_diffuse, face.axis())
    }
}

/// Every pixel of the capture showing `face` at least `margin` inside it,
/// with its world point.
pub(super) fn face_pixels(view: View, face: Face, margin: f32) -> Vec<(UVec2, Vec3)> {
    super::fluid_reflection_frame_test_fixtures::pixels()
        .filter_map(|pixel| {
            face.point_at(view, pixel, margin)
                .map(|point| (pixel, point))
        })
        .collect()
}

/// A flat roster light of `base_color`, on or off.
pub(super) fn flat(base_color: [f32; 3], on: bool) -> AnimationDescriptor {
    super::fluid_reflection_sums_frame_test_fixtures::flat_light(base_color, on)
}

/// A stored light naming roster light `roster_index` captured at
/// `reference`, holding `shares[k]` of the capture's light on lobe k (+Y,
/// +X, −X, +Z, −Z, −Y), every channel alike.
pub(super) fn stored(
    roster_index: u32,
    reference: [f32; 3],
    shares: [f32; REFLECTION_PROBE_LOBE_COUNT],
) -> ReflectionProbeLightShare {
    ReflectionProbeLightShare {
        roster_index,
        reference,
        shares: shares.map(|share| [share; 3]),
    }
}
