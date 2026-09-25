//! Overhead questgiver markers: the reference's `Interface\Buttons\TalkToMe*` M2s over NPC heads,
//! gold `!` for a quest available, gold `?` for a turn-in, grey `!` and `?`, and the light-blue
//! `?`. This module renders them; [`query`] decides when to ask, and the answers land in
//! [`QuestGiver`].
//!
//! - Attach (`0x6074c0`): a child of the unit's body M2 at attachment 18, or 29
//!   (`PlayerNameMounted`) when a mount model exists and the body authors it ([`overhead_slot`]).
//!   With neither, the marker is never parented and so invisible. The reference re-runs the attach
//!   from the `UNIT_FIELD_MOUNTDISPLAYID` watch (`0x5ffa50`), so a mount moves the slot.
//! - Scale (`0x607570`): `1/L`, `L = ‖row0‖` of the attach point's world matrix, computed once at
//!   attach into the marker's base matrix (`marker+0xbc`); not distance-based, no clamp. The
//!   compose is `marker+0xfc = marker+0xbc × parentAttachMatrix` (`0x71439b`), so `1/L` cancels the
//!   whole attach basis, model scale included. [`bake_seat_scale`] has the reference's race.
//! - Animation (`0x6076c0`): the marker's own looping bob, anim 0 at the attach point (WoW z 0 to
//!   −0.089), anim 190 raised (+0.517 to +0.427) while the unit has a live overhead name
//!   (`unit+0xc7c`, via `0x6c7950`), clear of the text.
//!
//! Every marker M2 is one bone with a three-key translation bob. A seat entity under the unit's
//! overhead joint carries the `1/L`. The `!` models (a plain bone) animate through the doodad rail
//! ([`benilla_world::doodad_anim::spawn_anim_host`]), ungated, since markers are few. The `?`
//! models (a cylindrical billboard bone) are [`BillboardCard`]s under the identity root, which
//! [`re_seat_cards`] re-seats from the seat each frame, their bob played by the card's own loop.

mod query;

use query::query_statuses;

use std::collections::HashMap;

use bevy::prelude::*;

use benilla_assets::{BillboardInfo, M2Model};
use benilla_formats::BoneScaleAnim;

use crate::entities::BoneAttach;
use crate::nameplates::Nameplates;
use crate::net::GuidIndex;
use crate::ui_quest::QuestGiver;
use benilla_assets::m2_url;
use benilla_assets::WorldAssets;
use benilla_world::billboard::BillboardCard;
use benilla_world::mesh_tag::spawn_tag;
use bevy::mesh::MeshTag;

use crate::entities::overhead_slot;

/// The bobs `0x6076c0` arms: low, and raised while the unit shows a name ([`Nameplates::shows`]).
const ANIM_MARKER_LOW: u16 = 0;
const ANIM_MARKER_RAISED: u16 = 190;

/// The billboard bone's translation loop for `anim_id`; the one bone's loop serves every card.
fn seq_loop(info: &BillboardInfo, anim_id: u16) -> Option<BoneScaleAnim> {
    info.seq_translations
        .iter()
        .find(|(id, _)| *id == anim_id)
        .map(|(_, l)| l.clone())
}

/// The marker M2 per dialog status: the reference's file table `0xc4d9d8` through the status map
/// `0x80c454` = `{0,3,0,2,7,1,6,6}`, its `.mdx` names loaded as `.m2`. The statuses are
/// UNAVAILABLE 1, INCOMPLETE 3, REWARD_REP 4, AVAILABLE 5, REWARD_OLD 6 and REWARD2 7; NONE 0 and
/// CHAT 2 draw nothing. File index 4, the green `!`, comes from the flight-master status instead,
/// and index 5, the blue `!`, is unreachable in 1.12.1.
fn marker_model(status: u32) -> Option<&'static str> {
    match status {
        1 => Some("Interface\\Buttons\\TalkToMeGrey.m2"),
        3 => Some("Interface\\Buttons\\TalkToMeQuestion_Grey.m2"),
        4 => Some("Interface\\Buttons\\TalkToMeQuestion_LTBlue.m2"),
        5 => Some("Interface\\Buttons\\TalkToMe.m2"),
        6 | 7 => Some("Interface\\Buttons\\TalkToMeQuestionMark.m2"),
        _ => None,
    }
}

/// One live marker's root and model; a status change swaps the whole instance (`0x607480`).
struct MarkerInst {
    root: Entity,
    path: &'static str,
}

/// The marker root, at the world origin because its billboard cards write absolute transforms.
#[derive(Component)]
struct QuestMarkerRoot {
    npc: u64,
    handle: Handle<M2Model>,
    /// The seat under the unit's overhead joint, once built; outside this root's hierarchy.
    seat: Option<Entity>,
    /// The overhead slot [`Self::seat`] was parented at, which [`sync_markers`] compares against
    /// the live pick to catch a mount or dismount.
    slot: Option<u16>,
    /// The body model authors neither overhead attachment: never parented, invisible. Latched.
    no_anchor: bool,
    /// The armed bob: raised (anim 190), low (anim 0), or `None`, not yet posed.
    raised: Option<bool>,
}

/// The attach seat under the unit's overhead joint, at the attachment offset. It carries the
/// marker's joints and plain meshes, and [`bake_seat_scale`] bakes the `1/L` into its transform.
#[derive(Component)]
struct MarkerSeat {
    /// Latched once baked: the reference computes the scale once, at attach.
    scaled: bool,
}

/// A billboard card's model-local pivot, [`BillboardCard::re_place`]'s second argument.
#[derive(Component)]
struct MarkerCardPivot(Vec3);

/// Despawns the root (its cards cascade) and the seat, which lives under the unit's joint; a
/// despawned unit has already taken the seat with it.
fn despawn_marker(commands: &mut Commands, roots: &Query<&QuestMarkerRoot>, root: Entity) {
    if let Some(seat) = roots.get(root).ok().and_then(|m| m.seat) {
        if let Ok(mut e) = commands.get_entity(seat) {
            e.despawn();
        }
    }
    commands.entity(root).despawn();
}

/// Spawns, swaps and despawns marker roots to match two sources for the one overhead slot: the
/// questgiver status, and the flight master's green `!` while its node is unknown
/// ([`crate::ui_taxi::FlightMasterStatus`], `0x607480` with file index 4).
///
/// Deviation: a quest marker outranks the green, where the reference's two handlers race
/// last-write-wins on the slot, because a fixed order is deterministic; the two meet only on a
/// flight master with a live quest marker, invisible in practice.
fn sync_markers(
    mut commands: Commands,
    quest: Res<QuestGiver>,
    fm_status: Query<(&crate::net::Guid, &crate::ui_taxi::FlightMasterStatus)>,
    index: Res<GuidIndex>,
    asset_server: Res<AssetServer>,
    assets: Option<Res<WorldAssets>>,
    roots: Query<&QuestMarkerRoot>,
    seats: Query<(), With<MarkerSeat>>,
    anchors: Query<&BoneAttach>,
    mounts: Query<(), With<crate::entities::mount::MountChild>>,
    mut live: Local<HashMap<u64, MarkerInst>>,
) {
    if assets.is_none() {
        return; // no client data: nothing could build
    }
    // Rebuild the whole instance, never part of it (that duplicates the root's cards), when the
    // seat is gone with a rebuilt visual or the slot moved on a mount or dismount, which keeps the
    // seat. The reference re-runs the attach from the mount watch (`0x5ffa50` calls `0x6074c0`).
    let rebuild: Vec<u64> =
        live.iter()
            .filter(|(&npc, inst)| {
                let Ok(marker) = roots.get(inst.root) else {
                    return false;
                };
                let want = index.0.get(&npc).and_then(|&unit| {
                    overhead_slot(anchors.get(unit).ok()?, mounts.contains(unit))
                });
                let alive = marker.seat.is_some_and(|seat| seats.contains(seat));
                seat_is_stale(marker, alive, want)
            })
            .map(|(&npc, _)| npc)
            .collect();
    for npc in rebuild {
        if let Some(old) = live.remove(&npc) {
            despawn_marker(&mut commands, &roots, old.root);
        }
    }
    let mut desired_by_npc: HashMap<u64, &'static str> = HashMap::new();
    for (&npc, &status) in quest.statuses() {
        if index.0.contains_key(&npc) {
            if let Some(path) = marker_model(status) {
                desired_by_npc.insert(npc, path);
            }
        }
    }
    for (guid, status) in &fm_status {
        if !status.known && index.0.contains_key(&guid.0) {
            desired_by_npc
                .entry(guid.0)
                .or_insert("Interface\\Buttons\\TalkToMeGreen.m2");
        }
    }
    // Spawn / swap.
    for (&npc, &path) in &desired_by_npc {
        let current = live.get(&npc).map(|m| m.path);
        if current == Some(path) {
            continue;
        }
        if let Some(old) = live.remove(&npc) {
            despawn_marker(&mut commands, &roots, old.root);
        }
        info!("quest_markers: {} over {:#x}", path, npc);
        let root = commands
            .spawn((
                QuestMarkerRoot {
                    npc,
                    handle: asset_server.load::<M2Model>(m2_url(path)),
                    seat: None,
                    slot: None,
                    no_anchor: false,
                    raised: None,
                },
                Transform::IDENTITY,
                Visibility::default(),
            ))
            .id();
        live.insert(npc, MarkerInst { root, path });
    }
    // Prune markers no source wants anymore (status change, unit gone, session clear).
    let stale: Vec<u64> = live
        .keys()
        .filter(|npc| !desired_by_npc.contains_key(npc))
        .copied()
        .collect();
    for npc in stale {
        if let Some(old) = live.remove(&npc) {
            despawn_marker(&mut commands, &roots, old.root);
        }
    }
}

/// [`sync_markers`]' rebuild test for one instance. Unbuilt (`seat: None`) is never stale:
/// [`build_markers`] owns it, and a rebuild would despawn a root still waiting on its M2. A dead
/// seat is stale, and so is a `want` that differs from the built slot; but a `None` want is
/// unreadable this pass (the unit streaming or gone), not a move, or the instance would rebuild
/// every frame its unit's attachment table is out of reach.
fn seat_is_stale(marker: &QuestMarkerRoot, seat_alive: bool, want: Option<u16>) -> bool {
    marker.seat.is_some() && (!seat_alive || (want.is_some() && want != marker.slot))
}

/// Builds a marker once its M2 and its unit's [`BoneAttach`] have both loaded: the seat under the
/// overhead joint with the `!` models' plain meshes, and the `?` models' cards under the root.
/// Latches [`QuestMarkerRoot::no_anchor`] when the body has no overhead point.
fn build_markers(
    mut commands: Commands,
    mut roots: Query<(Entity, &mut QuestMarkerRoot)>,
    m2s: Res<Assets<M2Model>>,
    mut forms: ResMut<benilla_world::model_forms::ModelForms>,
    mut mesh_assets: ResMut<Assets<Mesh>>,
    mut mats: benilla_world::model_render::M2BatchMaterials,
    mut palettes: ResMut<benilla_world::rig_palette::RigPalettes>,
    index: Res<GuidIndex>,
    anchors: Query<&BoneAttach>,
    // The overhead joint spawns on first demand (`RigPose::anchor_for`).
    mut poses: Query<&mut benilla_world::rig_anim::RigPose>,
    mounts: Query<(), With<crate::entities::mount::MountChild>>,
    time: Res<Time>,
) {
    for (root, mut marker) in &mut roots {
        if marker.seat.is_some() || marker.no_anchor {
            continue;
        }
        let Some(model) = m2s.get(&marker.handle) else {
            continue; // marker M2 still loading
        };
        // Render forms built now, uncapped, like the booths': two tiny models, and a marker a
        // frame late over its questgiver would be seen.
        forms.ensure_now_rigged(&marker.handle, &model.submeshes, &mut mesh_assets);
        // Absent while the unit's model loads, and for good on a unit without one.
        let Some((unit, anchor)) = index
            .0
            .get(&marker.npc)
            .and_then(|&e| Some((e, anchors.get(e).ok()?)))
        else {
            continue;
        };
        // This pass's slot, `0x6074c0`'s pick: 29 while mounted if the body authors it, else 18.
        // Neither: never parented, so invisible, as in the reference.
        let Some((slot, joint, offset)) =
            overhead_slot(anchor, mounts.contains(unit)).and_then(|slot| {
                let &(bone, offset) = anchor.points.get(&slot)?;
                let mut pose = poses.get_mut(unit).ok()?;
                let joint = pose.anchor_for(&mut commands, unit, bone)?;
                Some((slot, joint, offset))
            })
        else {
            debug!(
                "quest_markers: {:#x} has no overhead attachment (18/29) — marker never parents (invisible, the client's own behavior)",
                marker.npc
            );
            marker.no_anchor = true;
            continue;
        };

        // The seat rides the attach joint's live bone. The `!` models animate through the doodad
        // rail (sequence 0, anim 0); the all-billboard `?` models have nothing to skin.
        let seat_tf = Transform::from_translation(offset);
        let host = model
            .submeshes
            .iter()
            .any(|s| s.billboard.is_none())
            .then(|| benilla_world::doodad_anim::spawn_anim_host(&mut commands, model, seat_tf))
            .flatten();
        // Allocated eagerly: this lane has no draw gate to promote lazily. Slot 0 (table full)
        // falls back to the static mesh below.
        let marker_slot = host.as_ref().map_or(0, |h| {
            benilla_world::rig_palette::RigSkin::allocate_bones(
                &mut palettes,
                h.bones(),
                h.inverse_bindposes.clone(),
            )
            .map_or(0, |rig| {
                let slot = rig.slot;
                commands.entity(h.root).insert(rig);
                slot
            })
        });
        let seat = match &host {
            Some(h) => h.root,
            None => commands.spawn((seat_tf, Visibility::default())).id(),
        };
        commands.entity(seat).insert(MarkerSeat { scaled: false });
        commands.entity(joint).add_child(seat);
        debug!(
            "quest_markers: {:#x} attached under the slot-{slot} joint (animated: {})",
            marker.npc,
            host.is_some()
        );

        // The reference arms the animation at status receive; the cards' loop starts here, on
        // the clock `face_billboards` samples.
        let arm_ms = time.elapsed().as_millis() as u32;
        let built = forms.slices(&marker.handle);
        let (stat_forms, skin_forms) = (built.stat, built.skin.unwrap_or(&[]));
        for (pi, sub) in model.submeshes.iter().enumerate() {
            // An ordinary world-lit batch: one steady material, batch order 0 (a `?` is one
            // 353-vertex mesh, not a coplanar stack).
            let Some(material) = mats.steady(sub, sub.texture.clone(), 0) else {
                continue; // no shared light buffer yet
            };
            match &sub.billboard {
                Some(info) => {
                    let child = commands
                        .spawn((
                            Mesh3d(
                                stat_forms
                                    .get(pi)
                                    .map(|(h, _)| h.clone())
                                    .unwrap_or_default(),
                            ),
                            MeshMaterial3d(material),
                            MeshTag(spawn_tag(0, 1.0)),
                            Transform::IDENTITY,
                            BillboardCard::new(info, Transform::IDENTITY)
                                .with_seq_translation(seq_loop(info, ANIM_MARKER_LOW), arm_ms),
                            MarkerCardPivot(info.pivot),
                        ))
                        .id();
                    commands.entity(root).add_child(child);
                }
                None => {
                    // Plain geometry under the seat: the skinned twin on the host's palette rig
                    // when the model animates, else the static mesh (capture mode, a full palette).
                    let use_rig = marker_slot != 0;
                    let mesh = if use_rig {
                        skin_forms.get(pi).cloned().unwrap_or_default()
                    } else {
                        stat_forms
                            .get(pi)
                            .map(|(h, _)| h.clone())
                            .unwrap_or_default()
                    };
                    let child = commands
                        .spawn((
                            Mesh3d(mesh),
                            MeshMaterial3d(material),
                            MeshTag(spawn_tag(marker_slot, 1.0)),
                            Transform::IDENTITY,
                        ))
                        .id();
                    if let Some(h) = &host {
                        if use_rig {
                            commands
                                .entity(child)
                                .insert(benilla_world::rig_palette::RigPart(h.root));
                        }
                    }
                    commands.entity(seat).add_child(child);
                }
            }
        }
        // `RigFrame`: the host root is a rig inside the unit's anchor subtree, so re-seating the
        // overhead anchor must re-finalize the marker rig the same frame (`finalize_rig_worlds`).
        if let Some(h) = host {
            commands
                .entity(h.root)
                .insert(benilla_world::rig_anim::RigFrame(h.root));
            h.finish(&mut commands);
        }
        marker.seat = Some(seat);
        marker.slot = Some(slot);
    }
}

/// Swaps each built marker between the low (anim 0) and raised (anim 190) bob, `0x6076c0`'s
/// selector: raised while the unit's name object (`unit+0xc7c`) is live. The reference arms at
/// attach and re-arms on the frame the name's shown state flips (an edge in `0x6c6e90`), never
/// per frame.
///
/// Deviation: a live V-plate ([`VPlates`](crate::vplates::VPlates)) raises the marker too,
/// because the reference's marker sits low behind the plate: it arms anim 0 under a plate, whose
/// name suppression nulls the `desc+0x8` handle `0x6c7950` tests.
#[allow(clippy::type_complexity)] // a Bevy system: each param is one resource, the app's convention
fn pose_markers(
    plates: Res<Nameplates>,
    vplates: Res<crate::vplates::VPlates>,
    index: Res<GuidIndex>,
    m2s: Res<Assets<M2Model>>,
    time: Res<Time>,
    // Optional: a `!` marker's meshes live under the seat, so its root may have no children.
    mut roots: Query<(&mut QuestMarkerRoot, Option<&Children>)>,
    mut cards: Query<&mut BillboardCard, With<MarkerCardPivot>>,
    mut players: Query<&mut AnimationPlayer>,
) {
    for (mut marker, children) in &mut roots {
        let Some(seat) = marker.seat else {
            continue; // not built yet; posed the frame it builds
        };
        let raised = index
            .0
            .get(&marker.npc)
            .is_some_and(|&unit| plates.shows(unit) || vplates.0.contains(&unit));
        if marker.raised == Some(raised) {
            continue;
        }
        let Some(model) = m2s.get(&marker.handle) else {
            continue; // a swap mid-load: retry next frame
        };
        marker.raised = Some(raised);
        let anim_id = if raised {
            ANIM_MARKER_RAISED
        } else {
            ANIM_MARKER_LOW
        };
        debug!(
            "quest_markers: {:#x} pose → anim {anim_id} ({})",
            marker.npc,
            if raised { "raised" } else { "low" }
        );
        let arm_ms = time.elapsed().as_millis() as u32;
        // `?`: re-arm each card's loop from a fresh cursor; a model without the raised band keeps
        // its low bob rather than going still.
        let bob = model
            .submeshes
            .iter()
            .find_map(|s| s.billboard.as_ref())
            .and_then(|info| seq_loop(info, anim_id).or_else(|| seq_loop(info, ANIM_MARKER_LOW)));
        for &child in children.into_iter().flatten() {
            if let Ok(mut card) = cards.get_mut(child) {
                card.arm_seq_translation(bob.clone(), arm_ms);
            }
        }
        // `!`: switch the clip on the anim host, which is the seat entity. `stop_all` first:
        // `play` adds to the active set, and two live clips blend to a half raise.
        if let Ok(mut player) = players.get_mut(seat) {
            if let Some(clip) = model.animations.as_ref().and_then(|a| a.find(anim_id)) {
                player.stop_all();
                player.play(clip.node).repeat();
            }
        }
    }
}

/// Bakes each new seat's `1/L` once, as `0x607570` does at attach, skipped at exactly 1.0 (the
/// reference's identity guard).
///
/// Deviation: we wait for propagation to settle the joint matrix, so the marker is always constant
/// size. The reference reads the parent's `model+0xbc` as it stands at attach, and a parent still
/// at its constructor identity (a body still streaming, or a mount model pending, `0x6075ac`)
/// leaves the marker unscaled and proportional to the NPC for good. We keep the settled read
/// because it is deterministic and is what the reference shows over a placed NPC.
fn bake_seat_scale(
    mut seats: Query<(&mut MarkerSeat, &ChildOf, &mut Transform)>,
    joints: Query<&GlobalTransform, Without<MarkerSeat>>,
) {
    for (mut seat, parent, mut tf) in &mut seats {
        if seat.scaled {
            continue;
        }
        let Ok(joint) = joints.get(parent.parent()) else {
            continue;
        };
        let l = joint.affine().matrix3.x_axis.length();
        if l <= 0.0 {
            continue; // not propagated yet: retry next frame
        }
        #[allow(clippy::float_cmp)] // the reference's exact identity guard (fcomp 1.0)
        if l != 1.0 {
            tf.scale = Vec3::splat(1.0 / l);
        }
        seat.scaled = true;
    }
}

/// Re-seats every card from its seat's world placement. Runs in PostUpdate after propagation and
/// before [`benilla_world::billboard::BillboardPlace`], so a card rides the same frame's seat.
fn re_seat_cards(
    seats: Query<&GlobalTransform, With<MarkerSeat>>,
    roots: Query<(&QuestMarkerRoot, &Children)>,
    mut cards: Query<(&mut BillboardCard, &MarkerCardPivot)>,
) {
    for (marker, children) in &roots {
        let Some(seat_e) = marker.seat else { continue };
        let Ok(seat_global) = seats.get(seat_e) else {
            continue;
        };
        let placement = seat_global.compute_transform();
        for &child in children {
            if let Ok((mut card, pivot)) = cards.get_mut(child) {
                card.re_place(placement, pivot.0);
            }
        }
    }
}

pub(crate) struct QuestMarkersPlugin;

impl Plugin for QuestMarkersPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                query_statuses,
                sync_markers,
                build_markers,
                pose_markers,
                bake_seat_scale,
            )
                .chain(),
        )
        .add_systems(
            PostUpdate,
            re_seat_cards
                .after(bevy::transform::TransformSystems::Propagate)
                .before(benilla_world::billboard::BillboardPlace),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(seat: Option<Entity>, slot: Option<u16>) -> QuestMarkerRoot {
        QuestMarkerRoot {
            npc: 0xdead,
            handle: Handle::default(),
            seat,
            slot,
            no_anchor: false,
            raised: None,
        }
    }

    /// A mount keeps the rider's visual and so the seat, so only this comparison sees the slot
    /// move between 18 and 29.
    #[test]
    fn a_seat_goes_stale_when_its_slot_moves_but_never_on_a_reading_it_could_not_take() {
        let built_at_18 = root(
            Some(Entity::from_raw_u32(1).expect("a valid entity id")),
            Some(18),
        );
        assert!(
            seat_is_stale(&built_at_18, true, Some(29)),
            "the unit mounted — the seat hangs at 18 and nothing else would notice"
        );
        assert!(!seat_is_stale(&built_at_18, true, Some(18)), "settled");
        assert!(
            !seat_is_stale(&built_at_18, true, None),
            "the attachment table was unreadable this pass — not a move; churning on it would \
             rebuild the instance every frame a unit is out of reach"
        );
        assert!(
            seat_is_stale(&built_at_18, false, Some(18)),
            "the seat entity is gone — the visual was rebuilt out from under it"
        );

        // Unbuilt is never stale, even with the seat reported dead: it has none.
        let unbuilt = root(None, None);
        assert!(!seat_is_stale(&unbuilt, false, Some(29)));
        assert!(!seat_is_stale(&unbuilt, true, Some(29)));
    }

    /// `0x607570`: `1/L` baked once, never re-baked on a later rescale, and skipped at 1.0.
    #[test]
    fn seat_counter_scale_bakes_once_from_the_attach_basis() {
        let mut app = App::new();
        app.add_systems(Update, bake_seat_scale);
        let joint = app
            .world_mut()
            .spawn((
                Transform::default(),
                GlobalTransform::from(Transform::from_scale(Vec3::splat(2.0))),
            ))
            .id();
        let seat = app
            .world_mut()
            .spawn((
                MarkerSeat { scaled: false },
                Transform::IDENTITY,
                GlobalTransform::default(),
                ChildOf(joint),
            ))
            .id();
        app.update();
        let scale = |app: &App, e: Entity| app.world().entity(e).get::<Transform>().unwrap().scale;
        assert_eq!(scale(&app, seat), Vec3::splat(0.5), "1/L off the basis");

        // A later joint rescale: the latch holds.
        *app.world_mut()
            .entity_mut(joint)
            .get_mut::<GlobalTransform>()
            .unwrap() = GlobalTransform::from(Transform::from_scale(Vec3::splat(4.0)));
        app.update();
        assert_eq!(scale(&app, seat), Vec3::splat(0.5), "one-time latch");

        // An identity basis skips the write (`fcomp 1.0`) but still latches.
        let plain_joint = app
            .world_mut()
            .spawn((Transform::default(), GlobalTransform::IDENTITY))
            .id();
        let plain_seat = app
            .world_mut()
            .spawn((
                MarkerSeat { scaled: false },
                Transform::from_scale(Vec3::splat(3.0)), // a sentinel the no-op must not touch
                GlobalTransform::default(),
                ChildOf(plain_joint),
            ))
            .id();
        app.update();
        assert_eq!(scale(&app, plain_seat), Vec3::splat(3.0), "identity skips");
        assert!(
            app.world()
                .entity(plain_seat)
                .get::<MarkerSeat>()
                .unwrap()
                .scaled
        );
    }
}
