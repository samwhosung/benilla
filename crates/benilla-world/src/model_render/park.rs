//! Parking hidden world submeshes: a model part hidden for [`PARK_AFTER_FRAMES`] puts its `Mesh3d`
//! down, leaving every Bevy per-`Mesh3d` sweep, and picks it up once its root is shown again.
//! This runs in `Update` on last frame's verdict, so a re-shown part draws a frame after its
//! election but its `Mesh3d` is back before specialization and the visibility check: inserted
//! after the visibility check on a visible entity, it trips Bevy's `specialize_material_meshes`
//! tick unwrap. Parts on a render layer of their own (a portrait booth's, whose bake window is
//! four frames) never park.

use std::any::TypeId;

use bevy::camera::visibility::VisibilityClass;
use bevy::ecs::world::EntityWorldMut;
use bevy::prelude::*;

use super::ModelPart;

/// Consecutive hidden frames before a part parks, half a second at 60 Hz, so a glance costs
/// nothing.
const PARK_AFTER_FRAMES: u16 = 30;

/// The mesh a parked part put down, restored verbatim on unpark.
#[derive(Component)]
pub struct ParkedMesh(pub Handle<Mesh>);

/// Consecutive frames this part has been hidden (its own or an ancestor's verdict).
#[derive(Component, Default)]
pub struct HiddenFrames(pub u16);

/// Counts hidden frames, parks the long-hidden and unparks the shown, by last `PostUpdate`'s
/// propagated `InheritedVisibility`.
#[allow(clippy::type_complexity)]
pub(super) fn park_hidden_parts(
    mut commands: Commands,
    mut parts: Query<
        (
            Entity,
            &InheritedVisibility,
            Option<&mut HiddenFrames>,
            Option<&Mesh3d>,
            Option<&ParkedMesh>,
        ),
        (
            With<ModelPart>,
            Without<bevy::camera::visibility::RenderLayers>,
        ),
    >,
) {
    for (entity, inherited, frames, mesh, parked) in &mut parts {
        if inherited.get() {
            if let Some(parked) = parked {
                commands
                    .entity(entity)
                    .insert(Mesh3d(parked.0.clone()))
                    .remove::<ParkedMesh>()
                    // Bevy's `Mesh3d` add hook pushes a `VisibilityClass` entry on every re-add
                    // and `check_visibility` queues an entity once per entry, so each cycle would
                    // draw the part once more; dedup after the hook has run.
                    .queue(dedup_visibility_class);
            }
            if let Some(mut f) = frames {
                if f.0 != 0 {
                    f.0 = 0;
                }
            }
            continue;
        }
        let Some(mut frames) = frames else {
            commands.entity(entity).insert(HiddenFrames(1));
            continue;
        };
        frames.0 = frames.0.saturating_add(1);
        if frames.0 == PARK_AFTER_FRAMES {
            if let Some(mesh) = mesh {
                commands
                    .entity(entity)
                    .insert(ParkedMesh(mesh.0.clone()))
                    .remove::<Mesh3d>();
            }
        }
    }
}

/// Leaves one entry per class in an entity's [`VisibilityClass`].
fn dedup_visibility_class(mut e: EntityWorldMut) {
    if let Some(mut class) = e.get_mut::<VisibilityClass>() {
        let mut seen: Vec<TypeId> = Vec::with_capacity(class.len());
        class.retain(|t| {
            if seen.contains(t) {
                false
            } else {
                seen.push(*t);
                true
            }
        });
    }
}

/// `WOW_NO_MESH_PARK=1` turns parking off.
pub(super) fn enabled() -> bool {
    std::env::var_os("WOW_NO_MESH_PARK").is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> (World, Entity) {
        let mut w = World::new();
        let e = w
            .spawn((
                ModelPart {
                    kind: super::super::ModelKind::Doodad,
                    blend: benilla_formats::ModelBlend::Opaque,
                },
                Mesh3d(Handle::default()),
                InheritedVisibility::HIDDEN,
            ))
            .id();
        (w, e)
    }

    fn run(w: &mut World, n: usize) {
        let mut sys = IntoSystem::into_system(park_hidden_parts);
        sys.initialize(w);
        for _ in 0..n {
            sys.run((), w).unwrap();
            sys.apply_deferred(w);
        }
    }

    #[test]
    fn a_part_parks_after_the_hysteresis_and_unparks_when_shown() {
        let (mut w, e) = world();
        run(&mut w, PARK_AFTER_FRAMES as usize - 1);
        assert!(w.entity(e).contains::<Mesh3d>(), "not yet");
        run(&mut w, 1);
        assert!(!w.entity(e).contains::<Mesh3d>(), "parked");
        assert!(w.entity(e).contains::<ParkedMesh>());
        w.entity_mut(e).insert(InheritedVisibility::VISIBLE);
        run(&mut w, 1);
        assert!(
            w.entity(e).contains::<Mesh3d>(),
            "unparked on the shown frame"
        );
        assert!(!w.entity(e).contains::<ParkedMesh>());
        assert_eq!(w.entity(e).get::<HiddenFrames>().unwrap().0, 0);
    }

    /// The world registers the `Mesh3d` hook and required component as Bevy's plugin does.
    #[test]
    fn an_unparked_part_keeps_one_visibility_class_entry() {
        let mut w = World::new();
        w.register_required_components::<Mesh3d, VisibilityClass>();
        w.register_component_hooks::<Mesh3d>()
            .on_add(bevy::camera::visibility::add_visibility_class::<Mesh3d>);
        let e = w
            .spawn((
                ModelPart {
                    kind: super::super::ModelKind::Doodad,
                    blend: benilla_formats::ModelBlend::Blend,
                },
                Mesh3d(Handle::default()),
                InheritedVisibility::HIDDEN,
            ))
            .id();
        assert_eq!(w.entity(e).get::<VisibilityClass>().unwrap().len(), 1);
        run(&mut w, PARK_AFTER_FRAMES as usize);
        assert!(!w.entity(e).contains::<Mesh3d>(), "parked");
        w.entity_mut(e).insert(InheritedVisibility::VISIBLE);
        run(&mut w, 1);
        assert!(w.entity(e).contains::<Mesh3d>(), "unparked");
        assert_eq!(
            w.entity(e).get::<VisibilityClass>().unwrap().len(),
            1,
            "the re-added Mesh3d's hook pushed a second class entry — the part draws twice"
        );
    }

    #[test]
    fn a_glance_never_parks() {
        let (mut w, e) = world();
        run(&mut w, 10);
        w.entity_mut(e).insert(InheritedVisibility::VISIBLE);
        run(&mut w, 1);
        w.entity_mut(e).insert(InheritedVisibility::HIDDEN);
        run(&mut w, 10);
        assert!(w.entity(e).contains::<Mesh3d>(), "the count restarted");
    }
}
