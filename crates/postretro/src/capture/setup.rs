// Capture-only scene setup: light overrides, static camera, and level handoff.
// See: context/lib/rendering_pipeline.md §7.8

use std::collections::{BTreeMap, HashSet};
use std::mem::size_of;

use anyhow::{Context, Result, anyhow, bail};
use glam::{Mat4, Vec3};
use postretro_entities::ComponentKind;
use postretro_entities::EntityRegistry;
use postretro_entities::components::light::{FalloffKind, LightComponent, LightKind};

use super::scene::{CameraPose, ForcedAnimLight, ForcedAnimatedPromotion};
use crate::camera;
use crate::render::{LevelGeometry, Renderer, level_world_to_geometry};
use crate::scripting_systems::light_bridge::LightBridge;

/// Seed capture-only authored active states after the level install has restored
/// the baked descriptor mirror. `capture_frame_indirect` flushes these writes
/// in its first `update_per_frame_uniforms` call.
pub(super) fn install_forced_active_animation_descriptors(
    renderer: &mut Renderer,
    lights: &[postretro_level_loader::MapLight],
    forced_lights: Option<&[ForcedAnimLight]>,
) -> Result<Vec<(u32, [f32; 3])>> {
    let writes = resolve_forced_active_animation_slots(lights, forced_lights)?;
    validate_forced_animation_slot_bounds(&writes, renderer.animated_compose_descriptor_count())?;
    for &(slot, radiance) in &writes {
        renderer
            .write_animated_compose_descriptor(slot, &forced_active_animation_descriptor(radiance));
    }
    Ok(writes)
}

/// Capture normally remains the v1, delta-only path. A forced promoted still
/// is the one exception: materialize the same bridge tail windowed gameplay
/// emits, then let the renderer pin the already-assigned row's `w`. This keeps
/// the capture VM-free and single-instant while exercising the real forward
/// descriptor, rest cone, slot, and depth-cache seams. The bridge still starts
/// from capture's compact static-only list; the raw section-45 roster supplies
/// promotion identity without restoring the authored dynamic tier.
pub(super) fn install_capture_animated_promotion_bridge(
    renderer: &mut Renderer,
    world: &postretro_level_loader::LevelWorld,
    capture_lights: &[postretro_level_loader::MapLight],
    capture_light_influences: &[postretro_render_data::influence::LightInfluence],
    registry: &mut EntityRegistry,
    forced_active_writes: &[(u32, [f32; 3])],
) -> Result<()> {
    if capture_lights.iter().any(|light| light.is_dynamic) {
        bail!("capture promotion bridge requires the static-only capture light list");
    }
    let baked_descriptors = world
        .sh_volume
        .as_ref()
        .map(|volume| volume.animation_descriptors.as_slice())
        .unwrap_or(&[]);
    let roster = world
        .animated_direct_sh_delta_volumes
        .as_ref()
        .map(|section| section.animation_descriptor_indices.as_slice())
        .unwrap_or(&[]);
    let mut bridge = LightBridge::new();
    bridge.populate_from_level_with_influences(
        capture_lights,
        capture_light_influences,
        baked_descriptors,
        registry,
        (renderer.scripted_sample_byte_offset() / size_of::<f32>()) as u32,
    );
    bridge.set_animated_baked_promotion_roster(roster);
    apply_forced_active_capture_light_components(registry, forced_active_writes)?;
    let update = bridge
        .update(registry, 0.0, 1.0)
        .ok_or_else(|| anyhow!("capture promotion needs at least one map light"))?;
    if !update.has_dirty_data {
        bail!("capture promotion bridge did not emit its animated forward tail");
    }
    if !renderer.upload_light_bridge_snapshot(
        update.lights_bytes,
        update.influence_bytes,
        update.descriptor_bytes,
        update.samples_bytes,
        update.effective_brightness,
        update.animated_window_brightness,
        update.compose_descriptor_writes,
    ) {
        bail!("capture promotion light snapshot exceeds renderer capacity");
    }
    Ok(())
}

/// Keep one geometry handoff for capture. Task 7 can replace whole SH bodies
/// with the explicit legacy-or-streaming storage without changing this static
/// capture preparation or the renderer call order.
pub(super) fn capture_level_geometry<'a>(
    world: &'a postretro_level_loader::LevelWorld,
    texture_materials: &'a [postretro_render_data::material::Material],
    static_lights: &'a [postretro_level_loader::MapLight],
    static_light_influences: &'a [postretro_render_data::influence::LightInfluence],
    static_entity_shadow_lights: &'a [u32],
) -> LevelGeometry<'a> {
    LevelGeometry {
        lights: static_lights,
        light_influences: static_light_influences,
        entity_shadow_lights: static_entity_shadow_lights,
        ..level_world_to_geometry(world, texture_materials)
    }
}

/// The existing `force_active` descriptor only owns Pass B. When a forced
/// still also promotes, update the bridge-owned light so the forward term uses
/// identical radiance. The bridge then writes the same 48-byte compose
/// descriptor, proving a forced `w=0` scene remains byte-identical to v1.
fn apply_forced_active_capture_light_components(
    registry: &mut EntityRegistry,
    forced_active_writes: &[(u32, [f32; 3])],
) -> Result<()> {
    let forced_by_slot: BTreeMap<_, _> = forced_active_writes.iter().copied().collect();
    if forced_by_slot.is_empty() {
        return Ok(());
    }
    let light_ids: Vec<_> = registry
        .iter_with_kind(ComponentKind::Light)
        .map(|(id, _)| id)
        .collect();
    for id in light_ids {
        let mut component = registry
            .get_component::<LightComponent>(id)
            .with_context(|| format!("read capture light component {id:?}"))?
            .clone();
        let Some(&radiance) = component
            .animated_slot
            .and_then(|slot| forced_by_slot.get(&slot))
        else {
            continue;
        };
        component.color = radiance;
        component.intensity = 1.0;
        component.animation = None;
        registry
            .set_component(id, component)
            .with_context(|| format!("write forced capture light component {id:?}"))?;
    }
    Ok(())
}

/// Validate the complete batch before any write. The installed renderer count
/// is authoritative: an unusable SH section can leave only a dummy buffer.
fn validate_forced_animation_slot_bounds(
    writes: &[(u32, [f32; 3])],
    descriptor_count: u32,
) -> Result<()> {
    for &(slot, _) in writes {
        if slot >= descriptor_count {
            bail!(
                "force_active animated slot {slot} is outside the installed descriptor count {descriptor_count}"
            );
        }
    }
    Ok(())
}

/// Resolve authored tags against the complete map-light list. Capture's
/// static-only forward-light filter has a compacted index space, while
/// `animated_slot` names the independently indexed SH compose descriptor.
fn resolve_forced_active_animation_slots(
    lights: &[postretro_level_loader::MapLight],
    forced_lights: Option<&[ForcedAnimLight]>,
) -> Result<Vec<(u32, [f32; 3])>> {
    let Some(forced_lights) = forced_lights else {
        return Ok(Vec::new());
    };

    // Keying writes by slot both deduplicates multi-light tag matches and gives
    // the renderer a stable write order independent of `world.lights` order.
    let mut slot_radiance = BTreeMap::new();
    for forced in forced_lights {
        let mut tag_found = false;
        let mut animated_slot_found = false;
        for light in lights {
            if !light.tags.iter().any(|tag| tag == &forced.tag) {
                continue;
            }
            tag_found = true;
            let Some(slot) = light.animated_slot else {
                continue;
            };
            animated_slot_found = true;
            match slot_radiance.entry(slot) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(forced.radiance);
                }
                std::collections::btree_map::Entry::Occupied(entry)
                    if *entry.get() == forced.radiance => {}
                std::collections::btree_map::Entry::Occupied(_) => {
                    bail!(
                        "force_active tag `{}` resolves to an animated descriptor with conflicting radiance",
                        forced.tag
                    );
                }
            }
        }
        if !tag_found {
            bail!(
                "force_active tag `{}` does not match a map light",
                forced.tag
            );
        }
        if !animated_slot_found {
            bail!(
                "force_active tag `{}` does not match an animated map light",
                forced.tag
            );
        }
    }

    Ok(slot_radiance.into_iter().collect())
}

/// Resolve capture promotion tags through the section-45 raw roster. A
/// `MapLight::animated_slot` is only the compose-descriptor lookup key; the
/// renderer's promotion state is keyed by the roster position, so never pass
/// the slot itself to the capture override. Duplicate slots use the runtime
/// bridge's first-static-light identity.
pub(super) fn resolve_forced_animated_promotion_rows(
    lights: &[postretro_level_loader::MapLight],
    section: Option<
        &postretro_level_format::animated_direct_sh_delta_volumes::AnimatedDirectShDeltaVolumesSection,
    >,
    forced_promotions: Option<&[ForcedAnimatedPromotion]>,
) -> Result<Vec<(usize, f32)>> {
    let Some(forced_promotions) = forced_promotions else {
        return Ok(Vec::new());
    };
    if forced_promotions.is_empty() {
        return Ok(Vec::new());
    }
    let section = section.ok_or_else(|| {
        anyhow!("force_promotion requires an AnimatedDirectShDeltaVolumes section")
    })?;
    let delta_rows: HashSet<u32> = section.affinity_lights.iter().copied().collect();

    let first_static_light_for_slot = |slot: u32| {
        lights
            .iter()
            .enumerate()
            .find(|(_, light)| !light.is_dynamic && light.animated_slot == Some(slot))
    };

    let mut rows = BTreeMap::new();
    for forced in forced_promotions {
        let mut tag_found = false;
        let mut animated_light_found = false;
        let mut roster_row_found = false;
        for (light_index, light) in lights.iter().enumerate() {
            if !light.tags.iter().any(|tag| tag == &forced.tag) {
                continue;
            }
            tag_found = true;
            if light.is_dynamic {
                continue;
            }
            let Some(slot) = light.animated_slot else {
                continue;
            };
            animated_light_found = true;
            let Some((runtime_light_index, _)) = first_static_light_for_slot(slot) else {
                continue;
            };
            if runtime_light_index != light_index {
                bail!(
                    "force_promotion tag `{}` matches duplicate animated slot {slot} at map-light index {light_index}, but runtime section-45 identity resolves to the first static map light at index {runtime_light_index}",
                    forced.tag
                );
            }
        }
        if !tag_found {
            bail!(
                "force_promotion tag `{}` does not match a map light",
                forced.tag
            );
        }
        if !animated_light_found {
            bail!(
                "force_promotion tag `{}` does not match an animated baked map light",
                forced.tag
            );
        }

        // Resolve each raw row through the same first-static descriptor-slot
        // join used by the runtime bridge and renderer candidate roster.
        for (animated_baked_index, &descriptor_index) in
            section.animation_descriptor_indices.iter().enumerate()
        {
            let Some((_, runtime_light)) = first_static_light_for_slot(descriptor_index) else {
                continue;
            };
            if !runtime_light.tags.iter().any(|tag| tag == &forced.tag) {
                continue;
            }
            roster_row_found = true;
            if !delta_rows.contains(&(animated_baked_index as u32)) {
                bail!(
                    "force_promotion tag `{}` resolves to section-45 AnimatedBakedLights row {animated_baked_index}, but that row has no affinity/direct delta and is not promotion-eligible",
                    forced.tag
                );
            }
            match rows.entry(animated_baked_index) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(forced.weight);
                }
                std::collections::btree_map::Entry::Occupied(entry)
                    if (*entry.get() - forced.weight).abs() <= 1.0e-6 => {}
                std::collections::btree_map::Entry::Occupied(_) => {
                    bail!(
                        "force_promotion tag `{}` resolves to an animated-baked row with conflicting weights",
                        forced.tag
                    );
                }
            }
        }
        if !roster_row_found {
            bail!(
                "force_promotion tag `{}` has no section-45 AnimatedBakedLights row",
                forced.tag
            );
        }
    }
    Ok(rows.into_iter().collect())
}

/// Build the active/no-curve compose descriptor through the shared descriptor
/// packer. With `animation: None` and `active_without_animation: true`, the
/// radiance lands in `base_color` and `color_count` remains zero.
fn forced_active_animation_descriptor(
    radiance: [f32; 3],
) -> [u8; postretro_render_cpu::sh_volume::ANIMATION_DESCRIPTOR_SIZE] {
    let component = LightComponent {
        origin: [0.0; 3],
        light_type: LightKind::Point,
        intensity: 1.0,
        color: radiance,
        falloff_model: FalloffKind::Linear,
        falloff_range: 0.0,
        cone_angle_inner: None,
        cone_angle_outer: None,
        cone_direction: None,
        is_dynamic: false,
        animated_slot: None,
        follow_transform: false,
        carrier: None,
        animation: None,
    };
    crate::scripting_systems::light_bridge::pack_animation_descriptor(
        &component, 0, 0, radiance, true, None,
    )
}

/// Preserve capture's static-only light input while translating global PRL
/// selection indices into the same compact static-light index space. Keep one
/// output selection entry per input entry so shadowmask channels stay aligned.
pub(super) fn capture_static_lights_and_shadow_selection(
    lights: &[postretro_level_loader::MapLight],
    influences: &[postretro_render_data::influence::LightInfluence],
    entity_shadow_lights: &[u32],
) -> (
    Vec<postretro_level_loader::MapLight>,
    Vec<postretro_render_data::influence::LightInfluence>,
    Vec<u32>,
) {
    let mut global_to_static = vec![u32::MAX; lights.len()];
    let mut static_lights = Vec::with_capacity(lights.len());
    let mut static_influences = Vec::with_capacity(lights.len());
    for (global_index, light) in lights.iter().enumerate() {
        if light.is_dynamic {
            continue;
        }
        global_to_static[global_index] = static_lights.len() as u32;
        static_lights.push(light.clone());
        static_influences.push(influences.get(global_index).cloned().unwrap_or(
            postretro_render_data::influence::LightInfluence {
                center: glam::Vec3::ZERO,
                radius: f32::MAX,
            },
        ));
    }

    let static_entity_shadow_lights = entity_shadow_lights
        .iter()
        .map(|&global_index| {
            global_to_static
                .get(global_index as usize)
                .copied()
                .unwrap_or(u32::MAX)
        })
        .collect();

    (
        static_lights,
        static_influences,
        static_entity_shadow_lights,
    )
}

pub(super) fn derive_texture_materials(
    texture_names: &[String],
) -> Vec<postretro_render_data::material::Material> {
    let mut warned = HashSet::new();
    texture_names
        .iter()
        .map(|name| {
            let warned_count = warned.len();
            let material = postretro_render_data::material::derive_material(name, &mut warned);
            let prefix = postretro_render_data::material::parse_prefix(name);
            if material == postretro_render_data::material::Material::Default
                && !prefix.is_empty()
                && warned.len() > warned_count
            {
                log::warn!(
                    "[Material] Unknown prefix '{}' in texture '{}' — using default material",
                    prefix,
                    name,
                );
            }
            material
        })
        .collect()
}

/// Build the static capture camera directly so the scene's independently
/// authored FOV is honored rather than adding a transient presentation offset.
pub(super) fn capture_view_projection(camera: &CameraPose, width: u32, height: u32) -> Mat4 {
    let aspect = width as f32 / height as f32;
    let fov = camera.fov_deg.to_radians();
    let vfov = 2.0 * ((fov / 2.0).tan() / aspect).atan();
    let yaw = camera.yaw_deg.to_radians();
    let pitch = camera.pitch_deg.to_radians();
    let look_dir = Vec3::new(
        -yaw.sin() * pitch.cos(),
        pitch.sin(),
        -yaw.cos() * pitch.cos(),
    );
    let eye = Vec3::from_array(camera.position);
    Mat4::perspective_rh(vfov, aspect, camera::NEAR, camera::FAR)
        * Mat4::look_at_rh(eye, eye + look_dir, Vec3::Y)
}
