//! Overhead questgiver markers — the client's own `Interface\Buttons\TalkToMe*` M2s floating
//! above NPC heads: gold `!` (quest available), gold `?` (turn-in ready), grey `!`/`?` and the
//! light-blue `?` per the status map.
//!
//! The data plane is the era wire: every visible creature flagged `UNIT_NPC_FLAG_QUESTGIVER`
//! gets one `CMSG_QUESTGIVER_STATUS_QUERY` — the server only ever *answers* queries (vmangos
//! `QuestHandler.cpp:36-77`), never pushes, so every refresh point is the client's own to trigger,
//! and a status that is never re-asked for is a marker frozen at first sight. The re-ask law —
//! the reference's self-player descriptor **field watch**, and which of it we implement — is its
//! own concern and lives in [`query`]. Answers land in [`QuestGiver`]'s
//! per-guid status map (`net/apply`); THIS module is the render half: attach, scale, animate.
//!
//! The render law:
//! - **Attach** (`0x6074c0`): the marker is a CHILD of the unit's own body M2 at attachment slot
//!   **18** (0x12), or **29** (`PlayerNameMounted`) when a mount model exists and the body authors
//!   it — [`overhead_slot`], one definition shared with the overhead name/plate/FCT readers. No
//!   slot ⇒ the marker is created but never parented — invisible; we render nothing.
//!
//!   **That pick is LIVE.** `0x6074c0` is a call site of `0x5ffa50`, the
//!   `UNIT_FIELD_MOUNTDISPLAYID` field-watch handler, so the reference re-runs the whole attach —
//!   slot included — every time a unit mounts or dismounts. Our seat is *parented* rather than
//!   read per frame, so re-picking means rebuilding the instance: [`sync_markers`] compares the
//!   slot the seat was built at against the live pick on every pass.
//! - **Scale** (`0x607570`): `1 / |attach-bone basis|`, computed once at attach and baked into the
//!   marker's base matrix (`marker+0xbc`). Not distance-based, not unit scale, no clamp or floor.
//!
//!   **The `1/L` is a SNAPSHOT, not an invariant** (`0x607570` executed on the reference's own
//!   bytes, not only read). The arithmetic is confirmed —
//!   `L = ‖row0‖` of the attach point's WORLD matrix, so the compensation cancels the whole attach
//!   basis (`s` and any bone-chain scale alike). What was wrong is the *published consequence*: the
//!   old sentence here — "a constant world size regardless of the NPC's model scale, a gnome and an
//!   ogre get the same-sized `!`" — was unconditional and **is not**. `0x607570` reads the parent's
//!   `model+0xbc` **as it stands at attach**, and the reference has legs where it has not been
//!   stamped yet, each leaving the ctor identity ⇒ **no counter-scale at all**:
//!
//!   | ordering at attach | `L` | `marker+0xbc` | final world scale at `s = 3` |
//!   |---|---|---|---|
//!   | parent already placed (**case B**) | `3.0` | `diag(1/3)` | **1.0 — constant** |
//!   | parent still ctor identity (**case A**) | `≈1.0` | identity (skip) | **3.0 — proportional** |
//!
//!   Case A fires when `SMSG_QUESTGIVER_STATUS` lands while the body model is still streaming (the
//!   query goes out at object-create, in the same breath as the load starts, so it is a race), and
//!   *structurally* for a mounted questgiver whose mount M2 is still resident-pending (`0x6075ac`
//!   tests the MOUNT model too, which `0x6074c0` does not). Nothing re-runs the bake afterwards, so
//!   case A is permanent for that unit. Both are invisible at `s = 1`, which is why this went
//!   unnoticed — it is visible only on a scaled creature, which is exactly where B301 came from.
//!
//!   **benilla is case B by construction**: [`bake_seat_scale`] deliberately waits for propagation
//!   to produce a settled joint matrix. **That is the right leg, and it is settled by observation,
//!   not by argument** — the director ran the A/B against the reference and 1.12's `?` looks the
//!   same as ours (closing 1541's open question). B301 is `not-a-bug`: a
//!   gnome-sized `?` over a building-sized NPC is what the real client shows too.
//!
//!   So do **not** "fix" this by dropping the counter-scale. The report is real, the case-A leg is
//!   real, and the change is one line — which is exactly why this paragraph exists.
//!
//!   One correction worth carrying: the post-multiply operand is `CM2Scene+0xdc` = **inverse(view)**,
//!   not "the model's world transform" (`CM2Shared+0xdc`). The bone matrices
//!   live in VIEW space and already carry `model+0xbc`; the final compose is
//!   `marker+0xfc = marker+0xbc × parentAttachMatrix` at `0x71439b`.
//! - **Animation** (`0x6076c0`): the marker's own M2 animation is armed looping — anim **0**
//!   normally, anim **190** while the unit has a live overhead-name object (`unit+0xc7c` via
//!   `0x6c7950`). The two bands hold the same 3-key bob SHAPE at different heights: anim 0 at the
//!   attach point (WoW z 0 → −0.089), anim 190 raised (+0.517 → +0.427) — the authored push-up
//!   that lifts the marker clear of the name text (m2bones key-value probe, all five models; an
//!   earlier count-only probe recorded them as "the same bob" — falsified by the director's
//!   reference eye, decision record with this change).
//!
//! benilla's translation (m2bones-probed: every marker M2 is ONE bone with a 3-key translation
//! bob): a **seat** entity parented under the unit's overhead joint (18 or 29 — the held-items
//! rail) carries the one-time `1/L` counter-scale; the `!` models (plain bone) animate through
//! the doodad rail
//! ([`benilla_world::doodad_anim::spawn_anim_host`] — skinned twin + the one-time sequence-0 arm); the `?`
//! models (cylindrical-billboard bone) render as [`BillboardCard`]s under the identity root —
//! cards write ABSOLUTE world transforms, so they can't sit under the moving seat; instead
//! [`re_seat_cards`] re-seats them from the seat's world placement each frame (PostUpdate, the
//! same-frame propagated pose), and the card's own
//! armed [`seq_translation`](BillboardCard::with_seq_translation) loop plays the bob. Markers are
//! few and player-adjacent, so the anim host runs ungated (no draw-gate `DoodadAnimHost`).
//! [`pose_markers`] swaps every marker between the low and raised bob as the unit's overhead
//! elements toggle (the floating name [`Nameplates::shows`] OR a live V-plate — our `unit+0xc7c`;
//! the plate leg is a director-pinned deviation, rationale on [`pose_markers`], 2274/2275):
//! cards re-arm the matching loop, hosts switch the playing clip.

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

/// The marker anim ids the client arms (`0x6076c0`): the low bob, and the raised bob played while
/// the unit shows an overhead name (`unit+0xc7c` live — [`Nameplates::shows`] here).
const ANIM_MARKER_LOW: u16 = 0;
const ANIM_MARKER_RAISED: u16 = 190;

/// The billboard bone's translation loop for `anim_id`, if authored. The marker models are
/// single-bone, so every card of one marker shares the bone's loop.
fn seq_loop(info: &BillboardInfo, anim_id: u16) -> Option<BoneScaleAnim> {
    info.seq_translations
        .iter()
        .find(|(id, _)| *id == anim_id)
        .map(|(_, l)| l.clone())
}

/// The marker M2 per dialog status — the client's own dispatch (file table `0xc4d9d8` × status
/// map `0x80c454` = `{0,3,0,2,7,1,6,6}`; the binary ships `.mdx` names, the loader maps them to
/// `.m2`):
/// UNAVAILABLE(1) → grey `!`, INCOMPLETE(3) → grey `?`, REWARD_REP(4) → light-blue `?`,
/// AVAILABLE(5) → gold `!`, REWARD_OLD/REWARD2(6/7) → gold `?`; NONE(0)/CHAT(2) → nothing.
/// (The Green/Blue `!` variants are driven by sibling handlers off NPC_FLAGS, not this status
/// packet — the flight-master green (`TalkToMeGreen`, table index 4) now feeds [`sync_markers`]
/// from [`crate::ui_taxi::FlightMasterStatus`]; the rest stay out of scope, same as the client.)
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

/// One live marker: the root entity (billboard-card children live here) and which model it shows —
/// a status change swaps the whole thing (the client's model-swap `0x607480`).
struct MarkerInst {
    root: Entity,
    path: &'static str,
}

/// The marker root: the lifecycle handle. Stays at the WORLD origin (identity) — billboard cards
/// under it write absolute world transforms. The attach seat (under the UNIT's joint) is tracked
/// here so a swap/prune can despawn it too.
#[derive(Component)]
struct QuestMarkerRoot {
    npc: u64,
    handle: Handle<M2Model>,
    /// The seat under the unit's overhead joint, once built. Lives OUTSIDE this root's hierarchy —
    /// [`sync_markers`] despawns it explicitly (guarded: a despawned unit already cascaded it).
    seat: Option<Entity>,
    /// The overhead slot ([`overhead_slot`]) the live [`Self::seat`] was parented at — `None`
    /// until it is built. A parent link bakes the slot in, so this is what [`sync_markers`]
    /// watches against the live pick to catch a mount or dismount moving it.
    slot: Option<u16>,
    /// The unit's body model resolved but authors NEITHER overhead attachment: the client's
    /// marker is created but never parented — invisible. Latched so we stop retrying.
    no_anchor: bool,
    /// The armed pose: `Some(true)` = the raised (anim 190) bob, `Some(false)` = the low (anim 0)
    /// bob, `None` = not yet posed (fresh build) — [`pose_markers`] arms on the first pass and
    /// re-arms on every name-visibility flip.
    raised: Option<bool>,
}

/// The attach seat: a child of the unit's overhead joint (18/29) at the attachment offset.
/// Carries the marker joints + plain submeshes as children; [`bake_seat_scale`] bakes the `1/L`
/// counter-scale into its local transform (the client's `0x607570` base matrix).
#[derive(Component)]
struct MarkerSeat {
    /// The one-time scale latch (the client computes it once at attach, not per frame).
    scaled: bool,
}

/// A marker billboard card's model-local pivot — [`BillboardCard::re_place`]'s second argument
/// each frame.
#[derive(Component)]
struct MarkerCardPivot(Vec3);

/// Despawn one marker instance: the root (cards cascade) and its seat — which lives under the
/// UNIT's joint, so it needs the explicit, guarded kill (a despawned unit already cascaded it).
fn despawn_marker(commands: &mut Commands, roots: &Query<&QuestMarkerRoot>, root: Entity) {
    if let Some(seat) = roots.get(root).ok().and_then(|m| m.seat) {
        if let Ok(mut e) = commands.get_entity(seat) {
            e.despawn();
        }
    }
    commands.entity(root).despawn();
}

/// Reconcile the marker set with the per-guid statuses: spawn/swap/despawn marker roots. Two
/// sources feed the one overhead slot: the questgiver dialog status (this module's own wire),
/// and the flight-master node status ([`crate::ui_taxi::FlightMasterStatus`], decision 0497 —
/// `known = false` shows `TalkToMeGreen`, the client's `0x607480` with resource index 4). The
/// client's two handlers race last-write-wins on the same attach slot; we compose
/// deterministically instead — a quest marker, when the status yields one, outranks the green
/// (a named deviation, invisible in practice: a flight master carrying an active quest marker).
fn sync_markers(
    mut commands: Commands,
    quest: Res<QuestGiver>,
    fm_status: Query<(&crate::net::Guid, &crate::ui_taxi::FlightMasterStatus)>,
    index: Res<GuidIndex>,
    asset_server: Res<AssetServer>,
    assets: Option<Res<WorldAssets>>,
    roots: Query<&QuestMarkerRoot>,
    seats: Query<(), With<MarkerSeat>>,
    // The live overhead-slot pick's two inputs, read per pass off the unit (a handful of markers
    // exist at once, so this is a few map lookups a frame).
    anchors: Query<&BoneAttach>,
    mounts: Query<(), With<crate::entities::mount::MountChild>>,
    mut live: Local<HashMap<u64, MarkerInst>>,
) {
    if assets.is_none() {
        return; // no client data — nothing could build anyway
    }
    // Rebuild the whole instance whenever the built seat has stopped describing the unit — all
    // or nothing, because a partial rebuild would duplicate the root's billboard cards. The next
    // pass then re-attaches under the joints and the slot that are true now. Two triggers:
    //
    // - **the seat entity is gone** — the unit's visual was rebuilt out from under it, taking the
    //   joint tree the seat lived under with it (a redress, a display change);
    // - **the overhead slot has moved** (18 ↔ 29) — the unit mounted or dismounted since the
    //   build. Nothing else would notice, and this is the trigger benilla was missing (1871): a
    //   mount transition deliberately leaves the rider's visual intact — `reseat_mounts`
    //   RE-PARENTS the rig's joint anchors onto the seat frame instead of despawning them — so
    //   the marker's seat survives the transition, riding the right bone at the WRONG slot,
    //   permanently. The reference re-runs the whole attach here: `0x6074c0` is a call site of
    //   `0x5ffa50`, the `UNIT_FIELD_MOUNTDISPLAYID` watch handler.
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
    // The desired marker per guid, composed from both sources (still-streamed units only).
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

/// [`sync_markers`]' rebuild test for one live instance, split out because each of its three legs
/// has a trap in it:
///
/// - **not built yet** (`seat: None`) is never stale — [`build_markers`] owns that instance, and
///   calling it stale would despawn a root that is still waiting on its M2.
/// - **`seat_alive == false`** is stale: the unit's visual was rebuilt out from under the seat,
///   taking the joint tree it lived under with it.
/// - **`want` differs from the slot the seat was built at** is stale: the unit mounted or
///   dismounted. `want` is [`overhead_slot`]'s live pick, and `None` means *unreadable this pass*
///   (the unit is still streaming, or gone) — which must NOT count as a move, or a marker would
///   rebuild its whole instance every frame its unit's attachment table is out of reach. A unit
///   that really is gone is pruned below on its own terms.
fn seat_is_stale(marker: &QuestMarkerRoot, seat_alive: bool, want: Option<u16>) -> bool {
    marker.seat.is_some() && (!seat_alive || (want.is_some() && want != marker.slot))
}

/// Build a marker once its M2 AND its unit's joint set ([`BoneAttach`]) have both landed: the seat
/// under the slot-18 joint, the anim host + skinned plain submeshes on it (the `!` bob), and the
/// billboard cards under the identity root with their bob loop armed (the `?`). Retries silently
/// while either half still loads; latches [`QuestMarkerRoot::no_anchor`] when the body model has
/// no overhead point (the client's marker never parents — invisible).
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
    // The unit's pose buffer: the overhead joint the marker parents under spawns on first
    // demand (`RigPose::anchor_for`).
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
        // The marker's render forms, built NOW: two tiny models per map, on the
        // booth-lane exception — a marker popping a frame late over a questgiver would be a
        // regression nothing here needs.
        forms.ensure_now_rigged(&marker.handle, &model.submeshes, &mut mesh_assets);
        // The unit's joint set + attachment table — absent while its own model still loads (or
        // forever on a boneless/model-less unit, which then simply never shows a marker).
        let Some((unit, anchor)) = index
            .0
            .get(&marker.npc)
            .and_then(|&e| Some((e, anchors.get(e).ok()?)))
        else {
            continue;
        };
        // The overhead slot as it stands THIS pass — 29 while a mount model is attached and the
        // body authors it, else 18 (`0x6074c0`'s own pick). It is recorded on the root below so
        // `sync_markers` can rebuild the instance when a mount or dismount moves it. Neither slot
        // ⇒ never parented ⇒ invisible, like the client.
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

        // The seat: a child of the attach joint, so the marker rides the live bone with zero lag
        // (the held-items rail). The `!` models' plain geometry animates through the doodad rail —
        // the client's one-time load arm IS sequence 0 here (anim id 0, m2bones-verified, the same
        // file-order-first clip). The all-billboard `?` models skip the host (nothing to skin).
        let seat_tf = Transform::from_translation(offset);
        let host = model
            .submeshes
            .iter()
            .any(|s| s.billboard.is_none())
            .then(|| benilla_world::doodad_anim::spawn_anim_host(&mut commands, model, seat_tf))
            .flatten();
        // EAGER slot allocation — this lane has no draw gate to promote lazily (decision 0863
        // made laziness the terrain-stream caller's policy, not the host's). A handful of
        // markers exist at once, so eager is the right spend; slot 0 (table full, warned)
        // falls back to the static mesh below exactly as before. `allocate_bones` — the
        // collapsed shape: the world pass writes the rows off the host's
        // `RigPose`, no joint list exists.
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

        // The client arms the marker's animation at status receive — the cards' bob loop starts
        // its cursor here (the same clock `face_billboards` samples).
        let arm_ms = time.elapsed().as_millis() as u32;
        let built = forms.slices(&marker.handle);
        let (stat_forms, skin_forms) = (built.stat, built.skin.unwrap_or(&[]));
        for (pi, sub) in model.submeshes.iter().enumerate() {
            // A floating marker is an ordinary world-lit batch: one steady material, batch
            // order 0 (a `?` is a single 353-vert mesh, not a coplanar stack).
            let Some(material) = mats.steady(sub, sub.texture.clone(), 0) else {
                continue; // no shared light buffer yet
            };
            match &sub.billboard {
                Some(info) => {
                    // Cards write ABSOLUTE world transforms (`face_billboards`), so they live
                    // under the identity root and `re_seat_cards` re-seats them from the seat's
                    // world placement each frame; the armed loop plays the bob at the pivot.
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
                    // Plain geometry under the seat: the skinned twin bound to the host's palette
                    // rig when the model animates (the `!` bob); the static mesh
                    // otherwise (capture mode keeps every marker static, like the doodad rail),
                    // including the palette-full fallback (slot 0).
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
        // Attach the pose buffer, plus the seat-frame cascade marker: the
        // marker host root is a rig's `joints_root` living inside the UNIT's anchor subtree, so
        // a patch walk that re-seats the overhead anchor must re-finalize the marker rig the
        // same frame (`RigFrame` — the mount seat's law, `finalize_rig_worlds`).
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

/// Swap each built marker between its LOW (anim 0) and RAISED (anim 190) bob as the unit's
/// overhead elements toggle — the client's `0x6076c0` selector: anim 190 while the unit's name
/// object (`unit+0xc7c`) is live, anim 0 otherwise. The 190 band is the authored raised loop
/// (WoW z +0.427..+0.517 vs anim 0's −0.089..0), lifting the marker clear of the name text.
/// Benilla raises for the floating name ([`Nameplates::shows`]) OR a live V-plate
/// ([`VPlates`](crate::vplates::VPlates)) — the plate leg is a **director-pinned deviation**
/// (2275): the reference arms anim 0 under a live plate (ShouldShowName's plate suppression
/// destroys the rendered name and nulls the `desc+0x8` handle the selector `0x6c7950` tests),
/// leaving its marker low behind the plate; the director rejected that overlap on sight. The
/// re-arm law is settled faithful: the reference arms at attach AND re-arms on the frame the name
/// shown-state flips (an edge inside `0x6c6e90`, never per-frame) — exactly our re-arm-on-flip.
#[allow(clippy::type_complexity)] // a Bevy system: each param is one resource, the app's convention
fn pose_markers(
    plates: Res<Nameplates>,
    vplates: Res<crate::vplates::VPlates>,
    index: Res<GuidIndex>,
    m2s: Res<Assets<M2Model>>,
    time: Res<Time>,
    // `Children` is OPTIONAL: a `!` marker's plain meshes live under the SEAT, so its root has no
    // children at all — a `&Children` query silently skipped every `!` marker (caught by the live
    // run: two Northshire TalkToMe markers, zero pose lines).
    mut roots: Query<(&mut QuestMarkerRoot, Option<&Children>)>,
    mut cards: Query<&mut BillboardCard, With<MarkerCardPivot>>,
    mut players: Query<&mut AnimationPlayer>,
) {
    for (mut marker, children) in &mut roots {
        let Some(seat) = marker.seat else {
            continue; // not built yet — first pose lands right after the build, same frame
        };
        let raised = index
            .0
            .get(&marker.npc)
            .is_some_and(|&unit| plates.shows(unit) || vplates.0.contains(&unit));
        if marker.raised == Some(raised) {
            continue;
        }
        let Some(model) = m2s.get(&marker.handle) else {
            continue; // a swap mid-load — retry next frame
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
        // The `?` path: re-arm each card's loop with a fresh cursor (the client's arm law). A
        // model without the raised band keeps its low bob rather than going static.
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
        // The `!` path: switch the anim host's playing clip (the host, when one exists, IS the
        // seat entity). `stop_all` first — bevy's `play` only ADDS to the active set, and two
        // live clips BLEND (the director caught the `!` floating at half raise: anim 0 + 190
        // averaged). No 190 clip ⇒ re-arm what's there.
        if let Ok(mut player) = players.get_mut(seat) {
            if let Some(clip) = model.animations.as_ref().and_then(|a| a.find(anim_id)) {
                player.stop_all();
                player.play(clip.node).repeat();
            }
        }
    }
}

/// Bake each new seat's one-time counter-scale — the client's `0x607570` computes
/// `L = |attach-bone basis|` once at attach and writes `diag(1/L)` into the marker's base matrix
/// (the exact-`1.0` skip is the client's own identity guard). One-time per seat (latched).
///
/// **This is the module doc's "case B" leg, and choosing it is a decision, not a transcription.**
/// We retry until propagation yields a real joint matrix, so our `L` is always the *settled* world
/// basis and the marker is always constant-size. The reference reads whatever `model+0xbc` happens
/// to hold at attach, and on a still-streaming parent that is the ctor identity — no counter-scale,
/// a marker proportional to the NPC, permanently. So the old note here ("the joint global being a
/// frame old is immaterial — bone basis length doesn't animate") was true about the *bone* and
/// wrong about the *risk*: what varies between the two legs is not the bone, it is whether the
/// parent's world matrix exists yet. We keep the settled read — it is the reference's intent, the
/// only leg that is deterministic, and the leg the reference was observed on.
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
            continue; // propagation hasn't produced a real matrix yet — retry next frame
        }
        #[allow(clippy::float_cmp)] // the client's own exact-identity guard (fcomp 1.0)
        if l != 1.0 {
            tf.scale = Vec3::splat(1.0 / l);
        }
        seat.scaled = true;
    }
}

/// Re-seat every billboard card from its seat's world placement (cards write absolute world
/// transforms, so they can't be parented under the moving seat). Runs in PostUpdate after
/// transform propagation and before [`benilla_world::billboard::BillboardPlace`] writes the card
/// transforms — the card
/// rides the SAME-frame posed seat, no trailing frame.
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

    /// The rebuild test the mounted marker turns on (1871). A seat bakes its slot into a parent
    /// link, and a mount transition leaves the rider's visual — and therefore the seat — intact,
    /// so this comparison is the ONLY thing that can notice 18 ↔ 29 moving.
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

        // Not built yet: `build_markers` owns the instance, and a rebuild would despawn a root
        // still waiting on its M2. True even when the seat is reported dead — it has none.
        let unbuilt = root(None, None);
        assert!(!seat_is_stale(&unbuilt, false, Some(29)));
        assert!(!seat_is_stale(&unbuilt, true, Some(29)));
    }

    /// The seat's counter-scale is the client's `0x607570`: `1/L` from the attach joint's world
    /// basis, baked ONCE at attach — a later joint rescale does not re-bake (the one-time latch),
    /// and an exactly-1.0 basis skips the write (the client's own identity guard).
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

        // Rescale the joint afterwards: the latch holds (the client computes once at attach).
        *app.world_mut()
            .entity_mut(joint)
            .get_mut::<GlobalTransform>()
            .unwrap() = GlobalTransform::from(Transform::from_scale(Vec3::splat(4.0)));
        app.update();
        assert_eq!(scale(&app, seat), Vec3::splat(0.5), "one-time latch");

        // An identity basis skips the write entirely (`fcomp 1.0`'s no-op leg) but still latches.
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
