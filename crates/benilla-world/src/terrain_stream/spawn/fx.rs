//! A placed model's point lights, particle emitters and ribbon trails, baked at its placement.

use benilla_assets::coords::wow_to_bevy;
use benilla_assets::{ModelEmitter, ModelLight};
use benilla_formats::WmoLight;
use bevy::prelude::*;

use crate::lighting::WorldPointLight;
use crate::particles;

/// Unit intensity in the `PointLight` convention, which the packer's `/(4π)` undoes to exactly
/// `diffuse_color × diffuse_intensity`; `wow_model.wgsl` applies the falloff `1/(0.7d + 0.03d²)`.
const POINT_LIGHT_INTENSITY: f32 = 4.0 * std::f32::consts::PI;
/// The ≤3-nearest selection radius (yd) around the lit unit's anchor, not a cutoff (a GL light has
/// none; att(48 yd) ≈ 0.01), wide enough to reach a hall's fixtures ~20 yd from its centre.
const POINT_LIGHT_RANGE: f32 = 48.0;

/// The [`WorldPointLight`] for an authored M2 or WMO MOLT light, `color` its linear RGB. As in the
/// reference it lights terrain, M2s and WMO walls, each taking its unit's ≤3 nearest, diffuse only.
pub fn point_light(color: [f32; 3], intensity_scale: f32) -> WorldPointLight {
    WorldPointLight {
        color,
        intensity: POINT_LIGHT_INTENSITY * intensity_scale.max(0.0),
        range: POINT_LIGHT_RANGE,
    }
}

fn spawn_point_light(
    commands: &mut Commands,
    world: Vec3,
    color: [f32; 3],
    intensity_scale: f32,
    // The light's rooms inside a building, `None` outdoors: a torch prop in a culled room is not
    // in the reference's scene that frame, so it lights nothing.
    room: Option<crate::wmo_portal::WmoGroupVis>,
    out: &mut Vec<Entity>,
) {
    let mut e = commands.spawn((
        point_light(color, intensity_scale),
        Transform::from_translation(world),
    ));
    if let Some(room) = room {
        e.insert(crate::lighting::LightRooms(room));
    }
    out.push(e.id());
}

/// Spawns a light for each casting M2 point light (`type == 1`) at its rest position, since a
/// placed prop never animates its light bone; `room` comes off the placement's [`emitter_fade`].
pub(super) fn spawn_lights_for(
    commands: &mut Commands,
    lights: &[ModelLight],
    transform: Transform,
    room: Option<&crate::wmo_portal::WmoGroupVis>,
    out: &mut Vec<Entity>,
) {
    for l in lights.iter().map(|l| &l.def) {
        if !l.casts() {
            continue; // directional lights feed the ambient; a static `0` visibility key is dark
        }
        let world = transform.transform_point(wow_to_bevy(l.position));
        spawn_point_light(
            commands,
            world,
            l.diffuse_color,
            l.diffuse_intensity,
            room.cloned(),
            out,
        );
    }
}

/// Spawns a light for each omni MOLT light (`type == 0`) under the M2 falloff, which the disk
/// `attenStart`/`attenEnd` do not shape (`0x695c00`). A light belongs to the rooms whose MOLR
/// names it; one that no group names is ungated.
pub(super) fn spawn_wmo_lights_for(
    commands: &mut Commands,
    lights: &[WmoLight],
    group_light_refs: &[Vec<u16>],
    instance: Option<Entity>,
    transform: Transform,
    out: &mut Vec<Entity>,
) {
    for (i, l) in lights.iter().enumerate() {
        if !l.is_omni() {
            continue; // spot/directional/ambient MOLT types are not point sources
        }
        let i = i as u16;
        let groups: std::sync::Arc<[u16]> = group_light_refs
            .iter()
            .enumerate()
            .filter(|(_, refs)| refs.contains(&i))
            .map(|(g, _)| g as u16)
            .collect();
        let world = transform.transform_point(wow_to_bevy(l.position));
        spawn_point_light(
            commands,
            world,
            l.color,
            l.intensity,
            instance
                .filter(|_| !groups.is_empty())
                .map(|instance| crate::wmo_portal::WmoGroupVis { instance, groups }),
            out,
        );
    }
}

/// A placed model's one draw-set gate, shared by its mesh, emitters, ribbons, lights and anim host.
/// `fade` is `(radius, model-space centre)`; `None` or empty `groups` means no room gate.
pub(super) fn emitter_fade(
    transform: Transform,
    fade: (f32, Vec3),
    instance: Option<Entity>,
    groups: Option<&std::sync::Arc<[u16]>>,
) -> particles::EmitterFade {
    let room = instance.zip(groups.filter(|g| !g.is_empty()));
    particles::EmitterFade {
        instance,
        room: room.map(|(instance, groups)| crate::wmo_portal::WmoGroupVis {
            instance,
            groups: groups.clone(),
        }),
        ..particles::EmitterFade::sphere(fade.0, transform.transform_point(fade.1))
    }
}

/// Spawns a model's emitters into `out`. With an anim `host` each rides its bone's anchor, which
/// holds a bone that never animates at its rest pivot, so riding is exact for every emitter.
pub(super) fn spawn_emitters_for(
    commands: &mut Commands,
    emitters: &[ModelEmitter],
    transform: Transform,
    host: Option<&super::assemble::PlacementHost>,
    fade: &particles::EmitterFade,
    out: &mut Vec<Entity>,
) {
    let arm = host.and_then(|h| h.arm);
    for em in emitters {
        let owner = host
            .and_then(|h| h.anchor(em.def.bone))
            .map(|a| (a, em.bone_pivot));
        if let Some(e) = particles::spawn_emitter(
            commands,
            em,
            transform,
            particles::EmitterFrames {
                owner,
                anchor: None, // anchor at the placement: an animated bone never drags the cloud
                // An animated doodad's rig goes only when its placement unloads, the model's end,
                // so the pool is freed rather than left draining at the old spot.
                on_owner_loss: particles::OwnerLoss::Free,
                ..default()
            },
            // A placed doodad re-rolls its variation every play window, so its emitters read the
            // slot and clip time off the host's live player, as the reference's animate kernel
            // (`0x714260`) does for every model.
            match arm {
                Some(root) => particles::EmitClock::Host(root),
                None => particles::EmitClock::Pinned,
            },
        ) {
            commands.entity(e).insert(fade.clone());
            out.push(e);
        }
    }
}

/// Spawns a model's ribbon trails, each riding its host bone's anchor or, on a static placement,
/// `fallback_owner`; a trail despawns with its owner, so none joins a despawn list.
pub(super) fn spawn_ribbons_for(
    commands: &mut Commands,
    ribbons: &[benilla_assets::ModelRibbon],
    transform: Transform,
    host: Option<&super::assemble::PlacementHost>,
    fallback_owner: Option<Entity>,
    // The emitters' gate, carried because a joint or submesh owner states no fade sphere or rooms.
    fade: &particles::EmitterFade,
) {
    let arm = host.and_then(|h| h.arm);
    for rb in ribbons {
        let (owner, use_pivot) = match host.and_then(|h| h.anchor(rb.def.bone)) {
            Some(a) => (a, true),
            None => match fallback_owner {
                Some(e) => (e, false),
                None => continue, // nothing to ride (an entity-less placement)
            },
        };
        // The `+0xc0` enable gate reads the live sequence, as the emitters do; an unanimated
        // placement has no clock and rests in `Stand`.
        crate::ribbons::spawn_ribbon(
            commands,
            rb,
            owner,
            use_pivot,
            transform.scale.max_element(),
            arm.map_or(
                crate::ribbons::RibbonSeq::Fixed(0),
                crate::ribbons::RibbonSeq::Host,
            ),
            // No model-alpha source: a placed prop / effect instance is always drawn.
            None,
            Some(fade.clone()),
        );
    }
}
