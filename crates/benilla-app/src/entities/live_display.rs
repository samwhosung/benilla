//! Live descriptor **appearance** changes: a `Values` delta that moves
//! `UNIT_FIELD_DISPLAYID` / `GAMEOBJECT_DISPLAYID` swaps the entity's model in place, and one that
//! moves `OBJECT_FIELD_SCALE_X` eases its render scale — the druid-shapeshift / GM-morph gap
//! (ledger B69/F04) and `NetEntity::scale`'s old standing deferral, closed together because they
//! are one family: the create path interpreted both fields once and nothing ever re-read them.
//!
//! The reference watches both fields through its field-change registry. The DISPLAYID handler
//! reaches the model rebuild (`0x60abe0`), self-gated by "the display record actually changed"
//! (`0x60ae10` against the per-unit display cache `[unit+0xb34]`), re-resolves every model fact
//! from the new display (`0x60afb0 ResolveDisplayInfo`), and re-selects the stand/ride animation
//! (`0x60ce70`) — an **instant** swap, no morph transition (the ghost→alive revive swap rides the
//! same path; a shapeshift's green flash is the spell visual kit, a separate system). The SCALE_X
//! handler instead **eases the render scale over 2 s with a cosine smoothstep** (`0x614bbf`).
//!
//! Our shape is a diff-and-rebuild: the visual was BUILT with [`AppliedDisplay`], the live truth
//! is the descriptor store, and a difference tears the visual down for `attach_entity_visuals` to
//! rebuild — fade-skipped (a shapeshift isn't a spawn), waiting out the new model's async load
//! rather than flashing a cube. The collision height restamps **in the same commit** as the swap —
//! the 0645 rule that the collision box and the drawn body can never disagree is exactly why
//! neither restamped alone before this.
//!
//! A **teardown is right here and only here**: a display swap is a different model, so there is
//! nothing to keep. The two siblings that used to share this shape no longer do — a gear change
//! re-dresses in place and a mount transition re-seats in place, because in the
//! reference neither touches the body model at all. What survives a teardown is the unit's own
//! per-unit state, and one piece of it is load-bearing: its [`super::spell_fx::FxAttached`] list
//! outlives the model exactly as the reference's `+0xb4` does, so its persistent instances
//! re-spawn onto the new body rather than being lost with the old one.

use benilla_protocol::EntityKind;
use bevy::prelude::*;

use crate::net::{Guid, NetEntity, ObjectStore};

use super::collision_height::{collision_height_for, CollisionHeight};
use super::{Creatures, VisualAttached};

/// The reference's scale-ease window: 2 s, cosine smoothstep (`0x614bbf`).
const SCALE_EASE_SECS: f32 = 2.0;

/// On the entity: the display id its current visual was BUILT with — [`refresh_live_display`]'s
/// diff key (the `AppliedEquipment` pattern). Stamped by the attach path on every (re)build,
/// cube fallback included (same read, no churn for a model-less unit); torn down with the visual.
#[derive(Component)]
pub(super) struct AppliedDisplay(pub(super) Option<u32>);

/// A live display-id swap happened on this entity — the visual was torn down and will rebuild.
/// The reference's rebuild `0x60abe0` ends by REPLAYING the pending-morph latch's impact kit
/// (`0x60ad67`); this edge is what carries "the rebuild ran" to that
/// replay (`crate::creature_anim`'s morph-latch watcher), which owns the latch and the kit
/// resolve. Written only when the rebuild actually fires (the diff branch — our `0x60ae10`).
#[derive(Message, Clone, Copy)]
pub(crate) struct DisplaySwapped {
    pub(crate) entity: Entity,
}

/// A live render-scale ease toward [`NetEntity::scale`]: the reference's 2 s cosine smoothstep
/// (`0x614bbf`), ticked by [`tick_scale_ease`] as absolute writes (a mid-ease visual rebuild's
/// snap is simply overwritten next frame, so the ease survives it).
#[derive(Component)]
pub(super) struct ScaleEase {
    from: f32,
    to: f32,
    elapsed: f32,
}

/// The live descriptor's display id for this entity kind — the values-delta twin of the protocol's
/// create-time interpretation (`events/decode.rs` `display_id`): per-kind field, `0`/absent → `None`
/// (a real morph never zeroes it; the create block's absent-is-zero fold means `0` also reads
/// "never sent", so neither tears a visual down to a cube).
fn live_display_id(kind: EntityKind, store: &ObjectStore) -> Option<u32> {
    match kind {
        EntityKind::Unit | EntityKind::Player => store
            .0
            .unit_displayid()
            .filter(|&d| d > 0)
            .map(|d| d as u32),
        EntityKind::GameObject => store
            .0
            .gameobject_displayid()
            .filter(|&d| d > 0)
            .map(|d| d as u32),
        // A corpse's body display. It is not expected to move — the field is a
        // death-time snapshot — but it is the same field the create interpreted, so the same
        // differ answers for it. **Not** covered here: the flesh→bones flip, which the reference
        // reacts to with a full model reload (its `CORPSE_FIELD_FLAGS` mirror handler
        // `0x5d5fa0`→`0x5d6d60`). vmangos cannot produce it — the conversion destroys the corpse
        // object and creates a *separate* bones object under a new guid (`Map.cpp:3625`,
        // `RemoveCorpses`) — so the reload path is deliberately unbuilt rather than speculatively
        // modelled; see decision 1706's named omissions.
        EntityKind::Corpse => store.0.corpse_display_id(),
        _ => None,
    }
}

/// Diff each attached entity's live descriptor appearance against what its visual was built with,
/// and apply the change: a **display-id** move swaps the model (teardown → rebuild — the one
/// transition that still earns one) and a **scale** move arms the 2 s ease — both restamp
/// [`CollisionHeight`] in the same commit (its two inputs are exactly these two fields; decision
/// 0645's stamp-once rule was correct only while neither could change).
///
/// The self-avatar needs nothing special: it is the streamed entity, so the swap
/// rebuilds its body like any other unit and `player::mirror_self_collision_height` re-syncs the
/// swim lines from the restamp next frame. Mount children carry no [`ObjectStore`], so they can
/// never take this path (their display is the host's field, diffed by `mount::reseat_mounts`).
#[allow(clippy::type_complexity)]
pub(super) fn refresh_live_display(
    mut commands: Commands,
    creatures: Option<Res<Creatures>>,
    mut entities: Query<
        (
            Entity,
            &Guid,
            &mut NetEntity,
            &ObjectStore,
            &AppliedDisplay,
            Option<&CollisionHeight>,
            &Transform,
        ),
        With<VisualAttached>,
    >,
    mut swapped: MessageWriter<DisplaySwapped>,
) {
    for (entity, guid, mut net, store, applied, height, tf) in &mut entities {
        // ── The display-id swap ──────────────────────────────────────────────────────────────
        let live = live_display_id(net.kind, store);
        if let Some(live) = live {
            if applied.0 != Some(live) {
                info!(
                    "display swap: guid {:016x} {:?} {:?} -> {} (instant, the 0x60abe0 rebuild)",
                    guid.0, net.kind, applied.0, live
                );
                net.display_id = Some(live);
                swapped.write(DisplaySwapped { entity });
                // The full visual teardown set + our own diff key: children (parts, anchors,
                // held roots, mount child) despawn, the per-instance visual components strip, and
                // `attach_entity_visuals` rebuilds next frame(s) with the new display —
                // fade-skipped via `Reattached` (a shapeshift isn't a spawn).
                commands
                    .entity(entity)
                    .despawn_related::<Children>()
                    .remove::<(
                        VisualAttached,
                        AppliedDisplay,
                        super::equipment::AppliedEquipment,
                        super::mount::AppliedMount,
                        super::mount::MountChild,
                        AnimationPlayer,
                        bevy::animation::transition::AnimationTransitions,
                        AnimationGraphHandle,
                        benilla_assets::ModelAnimations,
                        crate::creature_anim::AnimDriver,
                        (
                            benilla_world::rig_anim::RigPose,
                            crate::creature_anim::BodyTwist,
                            benilla_world::rig_anim::GlobalSeqDrive,
                        ),
                        benilla_world::rig_palette::RigSkin,
                        super::BoneAttach,
                        super::equipment::HeldAttached,
                    )>()
                    .insert(super::equipment::Reattached);
            }
        }

        // ── The scale ease ───────────────────────────────────────────────────────────────────
        // The same kinds the create path scales (`events/decode.rs` `object_scale`): a kind whose
        // create ignored the field must keep ignoring its deltas, or the first delta would "fix"
        // a scale the create deliberately floored to 1.0.
        let scaled_kind = matches!(
            net.kind,
            EntityKind::Unit | EntityKind::Player | EntityKind::GameObject | EntityKind::Corpse
        );
        if let Some(live) = store.0.object_scale_x().filter(|s| *s > 0.0 && scaled_kind) {
            if live != net.scale {
                info!(
                    "scale change: guid {:016x} {} -> {} (2 s cosine ease, 0x614bbf)",
                    guid.0, net.scale, live
                );
                net.scale = live;
                commands.entity(entity).insert(ScaleEase {
                    from: tf.scale.x,
                    to: live,
                    elapsed: 0.0,
                });
            }
        }

        // ── The collision prism ──────────────────────────────────────────────────────────────
        // Derived from the unit's **native** display, so the rendered swap above is NOT one of its
        // inputs (the reference reads `NATIVEDISPLAYID` at `[unit+0x110]+0x1f8`).
        // Its two real inputs are `UNIT_FIELD_NATIVEDISPLAYID` and `SCALE_X`, and neither has a
        // change-gate we can piggyback on, so it is recomputed and diffed against the stamped
        // component instead: two `HashMap` lookups per attached unit per frame, the same order as
        // the two store reads this loop already does above (~0.02 ms at 300 units — a twentieth of
        // the standing-idler cost decision 1445 gated away, and unlike a gate it cannot miss a
        // native-display change that moved no pixel).
        //
        // Snapped to the TARGET scale immediately (the swim/wade/splash lines move once) — easing
        // a collision plane would drag the resolver through two seconds of intermediate depths.
        let prism = super::collision_height::prism_display_id(Some(store), net.display_id);
        let h = collision_height_for(creatures.as_deref(), prism, net.scale);
        if height != Some(&h) {
            debug!(
                "collision height restamp: guid {:016x} {:?} -> {:.3} (native display {:?}, \
                 rendered {:?}, scale {:.3})",
                guid.0,
                height.map(|c| c.0),
                h.0,
                prism,
                net.display_id,
                net.scale
            );
            commands.entity(entity).insert(h);
        }
    }
}

/// Tick every live [`ScaleEase`]: `scale(t) = from + (to − from) · (0.5 − 0.5·cos(π·t/2s))` — the
/// reference's cosine smoothstep (`0x614bbf`) — then land exactly on the target and retire.
pub(super) fn tick_scale_ease(
    mut commands: Commands,
    time: Res<Time>,
    mut easing: Query<(Entity, &mut Transform, &mut ScaleEase)>,
) {
    for (entity, mut tf, mut ease) in &mut easing {
        ease.elapsed += time.delta_secs();
        let t = (ease.elapsed / SCALE_EASE_SECS).min(1.0);
        let w = 0.5 - 0.5 * (std::f32::consts::PI * t).cos();
        tf.scale = Vec3::splat(ease.from + (ease.to - ease.from) * w);
        if t >= 1.0 {
            tf.scale = Vec3::splat(ease.to);
            commands.entity(entity).remove::<ScaleEase>();
        }
    }
}

/// Palette headroom below which the healer waits: rebuilding into a still-tight table would just
/// re-starve, and the doodad reaper (`doodad_anim::lazy`) is what makes room. Deliberately under
/// the reaper's low-water (256) so the reaper engages first and the heal follows into the space
/// it opened.
const HEAL_MIN_HEADROOM: usize = 128;

/// Visual rebuilds per frame — a starved *population* (a whole stream-in burst denied at once)
/// heals over a second or two instead of one spike of teardown+reattach commands.
const HEAL_PER_FRAME: usize = 2;

/// Rebuild the visuals of units whose attach was DENIED a palette rig (the
/// [`RigStarved`](benilla_world::rig_palette::RigStarved) marker): the same teardown set as the
/// display-id swap above (the reference's `0x60abe0` rebuild), fade-skipped via `Reattached` —
/// a heal is not a spawn. `attach_entity_visuals` rebuilds next frame(s), allocating with the
/// headroom this system waited for; if the table filled again in between, the attach re-marks
/// and the healer comes back. Before this system, a full-table denial froze the unit at bind
/// pose for its whole life — the "statue mobs at the stream-in boundary" bug.
#[allow(clippy::type_complexity)] // one query's two-marker filter
pub(super) fn heal_rig_starved(
    mut commands: Commands,
    palettes: Res<benilla_world::rig_palette::RigPalettes>,
    starved: Query<
        (Entity, &Guid),
        (
            With<benilla_world::rig_palette::RigStarved>,
            With<VisualAttached>,
        ),
    >,
) {
    if starved.is_empty() || palettes.slot_headroom() < HEAL_MIN_HEADROOM {
        return;
    }
    for (entity, guid) in starved.iter().take(HEAL_PER_FRAME) {
        info!(
            "rig heal: guid {:016x} was denied a palette rig at attach — rebuilding (0x60abe0 shape)",
            guid.0
        );
        commands
            .entity(entity)
            .despawn_related::<Children>()
            .remove::<(
                VisualAttached,
                AppliedDisplay,
                super::equipment::AppliedEquipment,
                super::mount::AppliedMount,
                super::mount::MountChild,
                AnimationPlayer,
                bevy::animation::transition::AnimationTransitions,
                AnimationGraphHandle,
                benilla_assets::ModelAnimations,
                crate::creature_anim::AnimDriver,
                (
                    benilla_world::rig_anim::RigPose,
                    crate::creature_anim::BodyTwist,
                    benilla_world::rig_anim::GlobalSeqDrive,
                ),
                benilla_world::rig_palette::RigSkin,
                super::BoneAttach,
                super::equipment::HeldAttached,
                benilla_world::rig_palette::RigStarved,
            )>()
            .insert(super::equipment::Reattached);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The heal law: a rig-starved unit rebuilds — visual children despawned,
    /// the attach trigger re-armed (`VisualAttached` off, `Reattached` on), the marker consumed
    /// — but ONLY when the palette has real headroom; against a still-tight table the healer
    /// waits (rebuilding into it would just re-starve).
    #[test]
    fn a_starved_unit_rebuilds_when_the_table_has_room_and_waits_when_it_has_not() {
        let mut app = App::new();
        app.init_resource::<benilla_world::rig_palette::RigPalettes>();
        app.add_systems(Update, heal_rig_starved);
        let child = app.world_mut().spawn_empty().id();
        let unit = app
            .world_mut()
            .spawn((
                benilla_world::rig_palette::RigStarved,
                VisualAttached,
                crate::net::Guid(0xB0B),
                AppliedDisplay(Some(7)),
            ))
            .add_child(child)
            .id();

        // Choke the table under the heal's minimum headroom: the unit is left alone.
        let hoard: Vec<benilla_world::rig_palette::RigSkin> = {
            let mut palettes = app
                .world_mut()
                .resource_mut::<benilla_world::rig_palette::RigPalettes>();
            (0..(benilla_world::mesh_tag::MAX_RIG_SLOTS - 1 - HEAL_MIN_HEADROOM / 2))
                .filter_map(|_| {
                    benilla_world::rig_palette::RigSkin::allocate_bones(
                        &mut palettes,
                        1,
                        Handle::default(),
                    )
                })
                .collect()
        };
        app.update();
        assert!(
            app.world().entity(unit).contains::<VisualAttached>(),
            "no headroom ⇒ the healer waits"
        );

        // Room opens (the reaper's doing, in the live app): the rebuild teardown lands.
        {
            let mut palettes = app
                .world_mut()
                .resource_mut::<benilla_world::rig_palette::RigPalettes>();
            for rig in hoard.iter().take(HEAL_MIN_HEADROOM) {
                let slot = rig.slot;
                palettes.free(slot);
            }
        }
        app.update();
        let e = app.world().entity(unit);
        assert!(!e.contains::<VisualAttached>(), "attach re-armed");
        assert!(
            !e.contains::<benilla_world::rig_palette::RigStarved>(),
            "marker consumed"
        );
        assert!(
            e.contains::<super::super::equipment::Reattached>(),
            "fade-skipped rebuild — a heal is not a spawn"
        );
        assert!(
            app.world().get_entity(child).is_err(),
            "the old visual's children despawned"
        );
    }

    /// The ease's shape is the reference's (`0x614bbf`): starts at `from`, cosine-smooth (half-way
    /// in value at half-way in time), lands exactly on `to` at 2 s and holds.
    #[test]
    fn scale_ease_is_the_2s_cosine_smoothstep() {
        let w = |elapsed: f32| {
            let t = (elapsed / SCALE_EASE_SECS).min(1.0);
            0.5 - 0.5 * (std::f32::consts::PI * t).cos()
        };
        assert_eq!(w(0.0), 0.0);
        assert!((w(1.0) - 0.5).abs() < 1e-6); // cos(π/2) = 0 → half-way in value at 1 s
        assert_eq!(w(2.0), 1.0);
        assert_eq!(w(3.0), 1.0); // clamped past the window
                                 // Smoothstep, not linear: the first quarter of the window covers less than a quarter
                                 // of the value (the eased head), symmetric with the tail.
        assert!(w(0.5) < 0.25);
        assert!(w(1.5) > 0.75);
    }
}
