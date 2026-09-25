//! GameObject open/close animation (chest lid folded in by 2271) — a **client-side**
//! `GAMEOBJECT_STATE` drives a skeletal M2 sequence, so a **door** swings, a **button** depresses, and a
//! **chest lid** opens/closes on its state machine.
//!
//! **The model (2271):** the real client keeps *one* stored state per GameObject (the binary's
//! `go+0x27c`) and *one* `SetGoState` (`0x5f8bd0`) that all callers funnel through; a change of
//! that state plays the transition. benilla mirrors that exactly — [`GoAnim::state`] is the single
//! source of truth, written by the **three callers** of that setter:
//!
//! 1. **the wire** ([`sync_wire_go_state`]) — a `GAMEOBJECT_STATE` UpdateField change. This is the
//!    door/button driver (the server flips their state over the wire) and the first-sight rest-pose seed.
//! 2. **the open-lock spell-go** ([`open_go_lid`]) — `SMSG_SPELL_GO` for an open-lock cast targeting the
//!    GO opens it (`SetGoState(ACTIVE)`). A chest's lid pops when the *Opening* cast goes off, not on the
//!    click — the faithful timing. The server never flips a chest's wire state (its `Use(CHEST)` runs
//!    scripts only; loot is spell-driven), so this — not the wire — is what opens a chest.
//! 3. **the loot-release** ([`close_go_lid`]) — the loot window closing (`CMSG_LOOT_RELEASE`) drops the
//!    state to READY, closing the lid, with no server round-trip (the client's loot-frame close handler).
//!
//! This supersedes 0244's "chests aren't animated": 0244 was right that the *wire* state never changes on
//! loot, but wrong to conclude the chest is off the machine — it runs identically, just fed from
//! loot events instead of the wire. Wiring a chest to the *wire* watch alone gave it a state that never
//! changed — the "instant open" glitch; feeding it from the loot events is the fix.
//!
//! The mechanism (`0x5f3c30`, `0x5f3cb0`): the
//! `GAMEOBJECT_STATE` value maps — **with no inversion**, the same polarity the sound path uses — to a
//! held rest pose, and a *change* of state plays a one-shot transition motion that settles onto the new
//! rest pose. The client keys the played sequence by its **AnimationData.dbc id** (the door-machine's
//! internal index and the debug state-name strings at `0x860850` are both stale/off-by-one — the
//! central trap; we key by id):
//!
//! | wire state | rest pose (held)      | entered by motion (one-shot) |
//! |------------|-----------------------|------------------------------|
//! | 1 READY    | `0x93` Closed         | `0x92` Close  (from open)     |
//! | 0 ACTIVE   | `0x95` Opened         | `0x94` Open   (from closed)  |
//! | 2 ALT      | `0x97` Destroyed      | `0x96` Destroy / `0x98` Rebuild |
//!
//! benilla reuses the creature skeletal path wholesale: an animated GameObject is instanced with the same
//! joints + `AnimationPlayer` + graph + [`ModelAnimations`] a creature gets ([`crate::entities::attach`]),
//! but tagged with [`GoAnim`] instead of `AnimDriver`, so this driver — not `creature_anim` — owns it.
//! Clips are keyed by AnimationData.dbc id, so a resolved id becomes a clip by a scan of
//! [`ModelAnimations::clips`], exactly as `creature_anim` does.
//!
//! **A transition motion is a TRANSIENT substate, and the settle is explicit**.
//! The kernel's `flags` bit 0 (`0x714585`) says nothing about how long
//! a transition lasts: bit 0 clear means the *pose* wraps its band for ever, and the whole
//! door family (`G_Crate01`, every `World\Goober\` prop, the books) authors Close/Open/Destroy
//! that way. What ends a swing is the **object layer**: the model's completion callback fires once
//! at the arm's baked window end — span × replay, bit 0 ignored — and slot 14 `0x5f4120` advances
//! the machine off the motion substate onto its rest one (2 Open → 3 Opened, 4 Close → 1 Closed,
//! 5 Destroy → 6 Destroyed, 7 Rebuild → 1 Closed), arming that pose over the motion. So benilla
//! arms a motion for exactly ONE window ([`RepeatAnimation::Never`]) and
//! [`retire_transient_anim`] re-runs the state machine at its end — the same endpoint, and the
//! same shape the Custom channel already uses. Honouring the loop bit
//! instead is a chest lid that springs open and slams shut ~1.5×/s for ever, which is the report
//! this record closes.
//!
//! The **missing-sequence fallback** (`0x5f3930`) is no longer deferred: [`remap_missing`]
//! implements the four-way remap, including the two legs that freeze a *motion* clip at frame 0 to
//! stand in for an absent rest pose. It is not a corner case — the Ahn'Qiraj gate's roots
//! (`AHN_QIRAJ_DOORROOTS`, Stand + Open only) took the "play nothing" path and rendered its 42-bone
//! tangle at bind pose.
//!
//! Still deferred (noted, not this slice): the **ANIMPROGRESS** half — the field selects the *motion*
//! substate rather than the rest one at spawn, and seeks the clip to `duration × progress / 100`
//! (`0x5f3c30`, `0x5f3ac5`); benilla reads
//! only `GAMEOBJECT_STATE` and always plays from frame 0. And the mid-flight **reverse blend**
//! (interrupting a half-open door).

use avian3d::prelude::{Collider, ColliderDisabled};
use benilla_assets::ModelAnimations;
use bevy::animation::transition::AnimationTransitions;
use bevy::animation::RepeatAnimation;
use bevy::prelude::*;
use std::time::Duration;

use crate::creature_anim::{advance_track, scan_events, AnimSoundEvent};
use crate::net::{GuidIndex, ObjectStore};
use benilla_world::schedule::WorldStage;

/// `GO_STATE_ACTIVE` (vmangos `GOState`) — the **open** state (door swung, chest lid up). Passable.
const GO_STATE_ACTIVE: u32 = 0;
/// `GO_STATE_READY` (vmangos `GOState`) — the **closed / solid** state. A door/button blocks movement
/// only in this state; `GO_STATE_ACTIVE` (0, open) and `GO_STATE_ACTIVE_ALTERNATIVE` (2) are passable.
const GO_STATE_READY: u32 = 1;

/// `AnimationData.dbc` **157 Despawn** — the id the one-shot channel's code 6 resolves to
/// (`0x80b0e0[6]` = substate 12, `0x8607e4[12]` = 157). The
/// object plays this once and *then* goes away; see [`DespawnAnimAnnounced`].
const ANIM_DESPAWN: u16 = 157;

/// Marker + client-side state for an animated GameObject. Instanced by
/// [`crate::entities::attach`] on an animatable GO type whose model authors sequences; driven by
/// [`drive_go_anim`]. Distinct from creatures' `AnimDriver` so the two drivers never touch one entity.
#[derive(Component, Default)]
pub(crate) struct GoAnim {
    /// Client-authoritative `GAMEOBJECT_STATE` (the binary's stored `go+0x27c`) — the single source of
    /// truth for the animation + collision. Written by the three "SetGoState callers": the wire
    /// sync, the open-lock spell-go, and the loot-release. `None` until first sight.
    state: Option<u32>,
    /// The state we last *animated* to — resolves which transition motion to play next. Distinct from
    /// `state` so a caller can change the target while [`drive_go_anim`] still knows the pose we're
    /// leaving (first sight settles the resting pose **silently** — a door that streams in already open
    /// must not replay its swing).
    shown: Option<u32>,
    /// A pending **one-shot** play, as an `AnimationData.dbc` id — the second, disjoint arm
    /// channel, never the lid family. The reference
    /// has ONE such slot fed by ONE entry point (`0x5f8c50(GO, code)` → slot 15), whose 7-entry
    /// code table `0x80b0e0` is the whole channel:
    ///
    /// | code | substate | id | producer |
    /// |------|----------|----|----------|
    /// | 1    | 0        | 145 Spawn      | object CREATE on wire update-type 3 (**not built**) |
    /// | 2..5 | 8..11    | 153..156 Custom0-3 | opcode `0xb3` (`anim_id >= 4` rejected) |
    /// | 6    | 12       | 157 Despawn    | opcode `0x215` — [`DespawnAnimAnnounced`] |
    ///
    /// Written by [`queue_custom_anim`] / [`arm_despawn_anim`], consumed by [`drive_go_anim`]
    /// AFTER the state arm, so the bobber's same-frame pair (the forced `READY → ACTIVE` flip +
    /// the splash) resolves with the splash on top. Slot 15 pre-gates on the model
    /// OWNING the resolved id, so this channel takes no remap.
    one_shot: Option<u16>,
    /// The clip armed for the current **TRANSIENT substate**, if any — the completion-retire's
    /// watch. The reference keeps exactly one current substate in
    /// `[handler+0x10]`, and its
    /// per-model completion callback fires ONCE at the arm's baked window end (span × replay, the
    /// loop bit ignored); slot 14 `0x5f4120` then advances a transient substate onto its rest one
    /// and arms that pose over the transient clip. **Two families reach it, and they share this
    /// one slot exactly as the reference's does**: the transition motions (2 Open / 4 Close /
    /// 5 Destroy / 7 Rebuild) and the Custom0..3 block (8..11). [`retire_transient_anim`] models
    /// the advance for both; without it a bit-0-clear clip runs for ever — the bobber's splash
    /// looping ~1.3 s (1100's 2-3 audible splashes) and the crate lid never settling shut (1151).
    transient: Option<Transient>,
    /// The **rest** pose's armed clip node, when that pose is on the re-arm cycle below. Distinct
    /// from [`Self::transient`], which is the reference's ONE transient-substate slot
    /// (`[handler+0x10]`) and drives the *advance*: a rest pose never advances, it re-arms
    /// **itself**. `None` while a transient owns the model, on the rate-0 frozen leg (which never
    /// completes), and once the already-playing check (`0x5f39fa`) has refused the re-arm and the
    /// pose has settled for good.
    rest_window: Option<AnimationNodeIndex>,
    /// The state the machine last **dispatched** — the reference's `0x5f3cb0(old, new)` entry,
    /// distinct from [`Self::shown`] because the completion retire clears `shown` to force a
    /// re-resolve and that re-resolve is emphatically *not* a dispatch (slot 14 routes a resting
    /// substate through `0x5f4167` → `0x5f3930`, straight past it). Only a genuine change of
    /// [`Self::state`] — and the completion of a Spawn/Custom substate, slot 14's `0x5f4190` leg —
    /// re-enters the dispatch, whose first act is `0x5f3cc8 call 0x5f40c0`: drop the object's
    /// display-sound loop registration ([`GoStateDispatch`]).
    dispatched: Option<u32>,
    /// The **requested** animation id currently armed — the reference's `[block0+0xf8]`, written by
    /// op4 (`0x71252f`) and read back by `0x712090(model, -1)`. The *remap* legs consult it
    /// (`0x5f39fa: cmp esi,eax; je 0x5f3b32`) and refuse an arm that would request what is already
    /// playing; the model-owns-it leg (`0x5f396c jne`) and the collapse-to-Stand leg
    /// (`0x5f3a54 jmp 0x5f3a0b`, past the check) do not. That asymmetry is the whole difference
    /// between a prop that **settles** and one that **cycles** — see [`drive_go_anim`].
    armed_id: Option<u16>,
}

/// The GameObject **state-machine dispatch** ran on this entity — the reference's `0x5f3cb0`,
/// whose first act is `0x5f3cc8 call 0x5f40c0`: **release whatever display-sound loop the object
/// holds**. Consumed by
/// [`crate::sound::gameobject`], which owns that registration; the animation half of the dispatch
/// is [`drive_go_anim`]'s own work and needs no message.
///
/// Emitted on a genuine `SetGoState` and on the completion of a **Spawn or Custom** substate
/// (slot 14's `0x5f4190` leg re-enters the dispatch with `old == new`) — and deliberately *not*
/// on a resting substate's own re-arm, which slot 14 sends to `0x5f4167` instead, past the
/// dispatch entirely. That distinction is the whole reason a brazier's `TorchLoop` hums
/// continuously instead of restarting every band pass.
#[derive(Message)]
pub(crate) struct GoStateDispatch(pub(crate) Entity);

/// Which completed substates re-enter the state dispatch — slot 14's table
/// `[0]=0x5f4190 … [8..11]=0x5f4190`, i.e. **Spawn (145) and Custom0..3 (153..156)**, as
/// `AnimationData.dbc` ids. Every other completed substate is either a transition motion (which
/// advances to its rest pose through `0x5f413d`/`0x5f414b`/`0x5f4159`) or a rest pose re-arming
/// itself (`0x5f4167`); neither touches the display-sound loop.
fn completion_redispatches(id: u16) -> bool {
    matches!(id, 145 | 153..=156)
}

/// The **armed** transient clip — its `AnimationData.dbc` id *and* the graph node the arm actually
/// landed on. The id alone was enough while every arm took a model's **head** variation; it stops
/// being enough now that the GameObject arm rolls one (`variationIdx = -1`, see [`drive_go_anim`]),
/// because two sequences sharing an id are two different nodes and only one of them is playing.
/// [`retire_transient_anim`] must watch the node that was armed, or it retires against a sibling
/// variation's (idle) player state and ends the substate on the wrong frame — immediately, for a
/// variation that was never played.
#[derive(Clone, Copy)]
struct Transient {
    /// The `AnimationData.dbc` id — the reference's `[handler+0x10]` substate identity, what the
    /// inspector and the despawn pin compare against.
    id: u16,
    /// The graph node of the clip actually armed: this id's *rolled* variation, not its head.
    node: AnimationNodeIndex,
}

/// The GameObject's **stored** state — the binary's `go+0x27c`, which is what every consumer reads
/// (the animation, `usable`'s state pre-gate, and the lock chain's per-slot Action gate,
/// decision 0752). [`GoAnim::state`] when the object is on the animation machine (it carries the
/// client-side predictions the wire never sends — a chest's lid), else the wire field, else the
/// wire default `0` = ACTIVE for a field vmangos omitted because it was zero.
pub(crate) fn go_state(anim: Option<&GoAnim>, store: &ObjectStore) -> u32 {
    anim.and_then(|a| a.state)
        .or_else(|| store.0.gameobject_state())
        .unwrap_or(GO_STATE_ACTIVE)
}

/// The inspector's GameObject **animation** readout: which sequence the state arm
/// is actually playing on the object under the cursor — its `AnimationData` id, whether it is the
/// state's held **rest** pose or a **transient** one (a transition motion, or a Custom block, which
/// [`retire_transient_anim`]'s advance ends at its window end), and the repeat the player is
/// running it under. `None` when the object renders as a static mesh, or nothing is armed yet.
///
/// The line that closes the loop on this whole class of report. "The crate is stuck open/closing"
/// and "the lid settles shut" are indistinguishable from every card line we had: the GO line said
/// `state 1 closed(READY)`, which was **true**, while a `Forever` Close motion swung underneath
/// it. `anim Close(146) · transition · loops` is that bug stated in one hover; `anim Closed(147) ·
/// rest` is the fix, read the same way.
///
/// The newest arm is the **smallest seek** — a cross-fade's fading source is older by construction
/// — the same pick [`fire_go_anim_events`] makes for the event scan.
pub(crate) fn armed_anim(
    go: &GoAnim,
    player: &AnimationPlayer,
    anims: &ModelAnimations,
) -> Option<(u16, bool, RepeatAnimation)> {
    let (clip, active) = anims
        .clips
        .iter()
        .filter_map(|c| player.animation(c.node).map(|a| (c, a)))
        .min_by(|a, b| a.1.seek_time().total_cmp(&b.1.seek_time()))?;
    Some((
        clip.anim_id,
        go.transient.is_some_and(|t| t.id == clip.anim_id),
        active.repeat_mode(),
    ))
}

/// Which GameObject types get the state-driven **animated** instance (skinned lid/door + state
/// sequences) — **read off the binary's type dispatch**, not a guess.
///
/// `CGGameObject::LoadBaseObject` dispatches on the wire TYPE_ID through the 31-entry jump table at
/// `0x5f76cc` (`cmp ecx,0x1e` + *unsigned* `ja`, so 0..30) and allocates a per-type strategy handler
/// into `[GO+0x210]`. The handlers fall into exactly two families, and the split is legible in the
/// **allocation size**: every type whose handler is `0x1c` bytes gets a 36-slot vtable carrying the
/// real `0x5f3c30`/`0x5f3b50` arm; every other size gets a 34-slot vtable whose corresponding slots
/// are the abstract base's do-nothing bodies — and whose `+0x88` doesn't even exist (reading it walks
/// into the next vtable, which is what makes a naive census of `+0x88` report plausible nonsense).
///
/// So the machine is **20 of the 31 types**, not the three we had:
///
/// | on the machine | off it |
/// |----------------|--------|
/// | 0 DOOR · 1 BUTTON · 2 QUESTGIVER · 3 CHEST · 6 TRAP · 8 SPELL_FOCUS · 9 TEXT · 10 GOOBER · 12 AREADAMAGE · 16 DUELFLAG · 17 FISHINGNODE · 18 SUMMONING_RITUAL · 19 MAILBOX · 23 MEETINGSTONE · 24 FLAGSTAND · 26 FLAGDROP · 27 MINI_GAME · 28 LOTTERY_KIOSK · 29 CAPTURE_POINT · 30 | 4 BINDER · 5 GENERIC · 7 CHAIR · 11 TRANSPORT · 13 CAMERA · 14 MAP_OBJECT · 15 MO_TRANSPORT · 20 AUCTIONHOUSE · 21 GUARDPOST · 22 SPELLCASTER · 25 FISHINGHOLE |
///
/// Type **30** is past vmangos's enum but has a real family-A arm in the table, so it is included for
/// completeness; type **21 GUARDPOST** has no `case` in the source at all and shares the out-of-range
/// default, which points `[GO+0x210]` at a static placeholder and logs `"BADBASEGAMEOBJECT|%d"`.
///
/// The old `0 | 1 | 3` was wrong in both directions — far too narrow, and its stated reason for
/// excluding GOOBER(10) ("also fires custom-anim + spells, so its path is unverified") was not a
/// reason the binary recognises: the custom-anim opcode drives a *disjoint* substate family (8..11),
/// never the lid. Measured against live world data, the narrowing left **1427 spawns** — books,
/// traps, goobers, questgivers — rendering the loader seed's pose instead of their state's.
///
/// A model that doesn't author a skeleton/sequences still falls back to a static mesh (the attach
/// gate also checks the joints/animations exist), so a lid-less chest simply stays static.
pub(crate) fn go_animates(type_id: i32) -> bool {
    matches!(
        type_id,
        0 | 1 | 2 | 3 | 6 | 8 | 9 | 10 | 12 | 16 | 17 | 18 | 19 | 23 | 24 | 26 | 27 | 28 | 29 | 30
    )
}

/// `GAMEOBJECT_TYPE_DOOR` — type id `0`, the one type a ghost's mover trace drops
/// ([`publish_ghost_door_exclusions`]). Also the wire default: vmangos omits zero-valued fields, so
/// an absent `GAMEOBJECT_TYPE_ID` means DOOR.
const GO_TYPE_DOOR: i32 = 0;

/// Which types drop their collision when open: **DOOR(0) / BUTTON(1)** only. A door's
/// static hull can't swing with the mesh, so an open door is made walkable by disabling the collider —
/// keyed off the server's wire state, which *is* the door's real passability. A **CHEST(3)** keeps its
/// collider in every state (you don't walk through an "open" chest), so it is deliberately excluded even
/// though it now animates.
fn collision_follows_state(type_id: i32) -> bool {
    matches!(type_id, 0 | 1)
}

/// Whether a door/button's collider is solid, from its **wire** `GAMEOBJECT_STATE`.
///
/// The load-bearing case is `None`. An absent field is not "unknown" — it is the wire default `0`
/// = `GO_STATE_ACTIVE` = **open**, because vmangos omits zero-valued fields from the create block
/// (the same law `go_templates` already documents for an absent `GAMEOBJECT_TYPE_ID` meaning
/// DOOR). Treating it as unknown and bailing left the collider enabled on **every door that spawns
/// open**, which is the whole of "GameObjects that block passage while visually open": Zul'Gurub's
/// Forcefield (180497) and Stratholme's `Doodad_SmallPortcullis03/04/08/09` are all `type 0`,
/// `startOpen = 1`, spawned at `state = 0`. It also explains the GM workaround the reporter found
/// — toggling the object twice un-sticks it — because the first toggle sends a *non-zero* state,
/// so the field finally exists on the wire, and the trip back to `0` then reads as a real value.
fn collider_is_solid(wire_state: Option<u32>) -> bool {
    wire_state.unwrap_or(GO_STATE_ACTIVE) == GO_STATE_READY
}

/// A cast launched at a GameObject (`SMSG_SPELL_GO` carrying a `TARGET_FLAG_GAMEOBJECT`), bridged from the
/// net apply layer to this module. [`open_go_lid`] opens the target's lid/door iff the
/// spell carries an open-lock effect and the GO is an animated type — the client's `Spell_C` open path.
#[derive(Message, Clone, Copy)]
pub(crate) struct GoLidOpen {
    pub(crate) go_guid: u64,
    pub(crate) spell_id: u32,
}

/// A GameObject's one-shot Custom animation (`SMSG_GAMEOBJECT_CUSTOM_ANIM`), bridged
/// from the net apply layer. The wire's `anim_id` is the Custom index (0..3 → AnimationData ids
/// 153..156); the reference rejects `anim_id >= 4` in the handler and this side keeps that gate.
/// The load-bearing sender is the fishing bobber's bite (`anim_id 0` — the splash, arriving beside
/// the forced state flip and the server's own `SMSG_PLAY_OBJECT_SOUND` splash kit).
#[derive(Message, Clone, Copy)]
pub(crate) struct GoCustomAnim {
    pub(crate) go_guid: u64,
    pub(crate) anim_id: u32,
}

/// What to play for the current `GAMEOBJECT_STATE`: a held **rest** pose (first sight, or a state with no
/// transition), or a one-shot transition **motion** (the swing) that lands on the new rest pose.
#[derive(Clone, Copy, Debug)]
enum Play {
    /// Snap to a held pose — no swing (first sight / stream-in).
    Rest(u16),
    /// Play a transition motion once, holding its end frame (= the destination rest pose).
    Motion(u16),
}

impl Play {
    fn anim_id(self) -> u16 {
        match self {
            Play::Rest(id) | Play::Motion(id) => id,
        }
    }
}

/// The **missing-sequence remap** — what the arm actually requests when the model doesn't author
/// the state's animation id. Read off `0x5f3972`'s jump table `0x5f3b40`, it is a four-way table
/// keyed on the id, consulted only after the ownership test
/// [`ModelAnimations::owns`] (the reference's `0x711960`) says no:
///
/// | missing    | condition         | requests instead           |
/// |------------|-------------------|----------------------------|
/// | 146 Close  | owns Open         | 146 (op4 resolves onward)  |
/// |            | else              | 147 Closed                 |
/// | 147 Closed | owns Close        | 147 (op4 resolves onward)  |
/// |            | else owns Open    | **148 Open, frozen**       |
/// |            | else              | 0 Stand                    |
/// | 148 Open   | owns Close        | 148 (op4 resolves onward)  |
/// |            | else owns Destroy | 150 Destroy                |
/// |            | else              | 149 Opened                 |
/// | 149 Opened | owns Open         | 149 (op4 resolves onward)  |
/// |            | else owns Close   | **146 Close, frozen**      |
/// |            | else              | 151 Destroyed              |
///
/// The returned flag marks the two **frozen** legs, where the reference stands a *motion* clip in for
/// a missing *rest* pose by arming it at playback rate `0`. That reads correctly because a motion's
/// frame 0 IS the pose it departs from: Open's first frame is closed, Close's first frame is open —
/// confirmed in the asset, `G_BookOpenMediumBrown`'s bone-0 rotation being identity at Open's first
/// key and the full open angle at Close's. Ids outside the door group get no remap at all
/// (`lea eax,[esi-0x92]; cmp eax,3; ja`).
///
/// Before this existed an unowned id simply played **nothing**: the `AnimationPlayer` was never armed
/// and the skinned mesh sat at bind pose. `AHN_QIRAJ_DOORROOTS` (the Ahn'Qiraj gate's roots, GO type
/// 1 BUTTON) authors only Stand and Open — no Closed — so it took exactly that path.
fn remap_missing(anims: &ModelAnimations, id: u16) -> (u16, bool) {
    if anims.owns(id) {
        return (id, false);
    }
    match id {
        146 if anims.owns(148) => (146, false),
        146 => (147, false),
        147 if anims.owns(146) => (147, false),
        147 if anims.owns(148) => (148, true),
        147 => (0, false),
        148 if anims.owns(146) => (148, false),
        148 if anims.owns(150) => (150, false),
        148 => (149, false),
        149 if anims.owns(148) => (149, false),
        149 if anims.owns(146) => (146, true),
        149 => (151, false),
        other => (other, false),
    }
}

/// The held rest-pose animation-id for a wire `GAMEOBJECT_STATE` (`0x5f3c30`). `None` for an
/// unmapped state.
fn rest_anim(state: u32) -> Option<u16> {
    match state {
        0 => Some(0x95), // ACTIVE  → Opened (held open)
        1 => Some(0x93), // READY   → Closed (held closed)
        2 => Some(0x97), // ALT     → Destroyed
        _ => None,
    }
}

/// The transition-motion animation-id for a `prev → cur` state change, i.e. the swing. `None`
/// when the pair has no distinct motion (falls back to snapping the rest pose).
///
/// The reference (`0x5f3cb0`) dispatches on the
/// **NEW** state and lets OLD pick transient-vs-rest: NEW 0 takes the Open motion from OLD 1 (else
/// rests Opened), NEW 1 takes Close from OLD 0 / Rebuild from OLD 2 (else rests Closed), NEW 2 takes
/// Destroy from OLD 1 (else rests Destroyed). Our `(_, 2)` wildcard got that last row wrong: an
/// **open** object going destructible (`0 → 2`) rests straight on Destroyed rather than playing the
/// Destroy swing, because the reference's condition is `OLD == 1`, not "any OLD".
///
/// (The reference's conditions are each `OLD == x` **or** `ANIMPROGRESS < 100`. benilla does not read
/// ANIMPROGRESS yet — see the module doc's deferral — so this is the settled column only, which is
/// what every state-change we can observe today resolves to.)
fn motion_anim(prev: u32, cur: u32) -> Option<u16> {
    match (prev, cur) {
        (1, 0) => Some(0x94), // closed → open      : Open motion  (else NEW 0 rests Opened)
        (0, 1) => Some(0x92), // open   → closed    : Close motion
        (2, 1) => Some(0x98), // rebuilt → closed   : Rebuild
        (1, 2) => Some(0x96), // closed → destroyed : Destroy      (else NEW 2 rests Destroyed)
        _ => None,
    }
}

/// Resolve the play for a state observation: first sight (`prev` None) snaps the rest pose; a change plays
/// the transition motion if one exists, else snaps the new rest pose.
fn resolve(prev: Option<u32>, cur: u32) -> Option<Play> {
    match prev {
        None => rest_anim(cur).map(Play::Rest),
        Some(p) => motion_anim(p, cur)
            .map(Play::Motion)
            .or_else(|| rest_anim(cur).map(Play::Rest)),
    }
}

/// Caller 1 (the wire, `0x5f89e0`): track each animated GO's `GAMEOBJECT_STATE` from the wire,
/// acting only on a
/// *genuine* wire change. This is the door/button driver (the server flips their state over the wire) and
/// the first-sight rest-pose seed for every animated GO (a chest streams in closed). Two inputs, both
/// exact: the seed (`Added<GoAnim>`, when attach tags the entity — read off the store),
/// and the field EDGE on `GAMEOBJECT_STATE` for any later delta. An unrelated field change (position,
/// dyn-flags) is not an edge on this dword, so a chest whose wire state is constant is never re-closed by
/// one — its lid is owned by the loot callers below. That used to need a `last_wire` shadow on the
/// component; the edge carries the old value itself.
#[allow(clippy::type_complexity)]
fn sync_wire_go_state(
    mut edges: MessageReader<crate::net::FieldChanged>,
    // The seed leg and the edge leg both write `GoAnim`; one `ParamSet`, two disjoint views.
    mut gos: ParamSet<(
        Query<(&ObjectStore, &mut GoAnim), Added<GoAnim>>,
        Query<&mut GoAnim>,
    )>,
) {
    for (store, mut anim) in &mut gos.p0() {
        // Absent ⇒ the wire default `0` = ACTIVE — vmangos omits zero fields, so a
        // door that spawns OPEN sends none. Reading that as "unknown" left `state` at `None`, and
        // the object rested at its loader pose instead of Opened.
        anim.state = Some(store.0.gameobject_state().unwrap_or(GO_STATE_ACTIVE));
    }
    let mut live = gos.p1();
    for e in edges.read() {
        if e.kind != benilla_protocol::messages::ObjectType::GameObject
            || e.index != benilla_protocol::field::FIELD_GAMEOBJECT_STATE
        {
            continue;
        }
        if let Ok(mut anim) = live.get_mut(e.entity) {
            anim.state = Some(e.new);
        }
    }
}

/// Caller 2 (the open-lock spell-go): open a chest lid / locked door when a cast with an open-lock effect
/// launches at it (`SMSG_SPELL_GO` → [`GoLidOpen`]). Gated on the spell's open-lock effect (the client's
/// `[spell+0xf4] ∈ {OPEN_LOCK, OPEN_LOCK_ITEM}` test) so a plain unit spell that merely names a GO can't
/// open it; observer-safe (another player's cast opens the chest you can see, since it resolves the GO
/// guid from the packet, not our own pending cast). Sets the client state to ACTIVE(0).
fn open_go_lid(
    mut opens: MessageReader<GoLidOpen>,
    spells: Option<Res<crate::ui_action::Spells>>,
    index: Res<GuidIndex>,
    mut gos: Query<&mut GoAnim>,
) {
    for GoLidOpen { go_guid, spell_id } in opens.read().copied() {
        let is_open_lock = spells
            .as_deref()
            .and_then(|s| s.catalog.get(spell_id))
            .is_some_and(|d| d.open_lock.is_some());
        if !is_open_lock {
            continue;
        }
        let Some(&e) = index.0.get(&go_guid) else {
            continue;
        };
        if let Ok(mut anim) = gos.get_mut(e) {
            anim.state = Some(GO_STATE_ACTIVE);
        }
    }
}

/// Caller 4 (the custom-anim opcode): queue the one-shot Custom play. This is the
/// **disjoint** arm channel (`0x5f8c50`) — it never touches
/// [`GoAnim::state`] (the lid family), rejects `anim_id >= 4` exactly as the reference handler
/// does, and maps the index to its AnimationData id (`153 + n`, Custom0..3). Ownership is judged
/// at play time by [`drive_go_anim`] (the model components live there); a guid with no [`GoAnim`]
/// — a non-animated GO type, or one still streaming in — drops the play, matching
/// [`open_go_lid`]'s posture.
fn queue_custom_anim(
    mut plays: MessageReader<GoCustomAnim>,
    index: Res<GuidIndex>,
    mut gos: Query<&mut GoAnim>,
) {
    for GoCustomAnim { go_guid, anim_id } in plays.read().copied() {
        let Some(id) = custom_anim_id(anim_id) else {
            continue; // the reference handler's own reject (`0x5f8971`)
        };
        let Some(&e) = index.0.get(&go_guid) else {
            continue;
        };
        if let Ok(mut anim) = gos.get_mut(e) {
            anim.one_shot = Some(id);
        }
    }
}

/// The wire Custom index → its AnimationData id (`153 + n`, Custom0..3), or `None` for the
/// reference handler's reject (`anim_id >= 4` — `0x5f8971`: opcode `0xb3` byte `b`, reject
/// `b >= 4`, substate `8+b`).
fn custom_anim_id(anim_id: u32) -> Option<u16> {
    (anim_id < 4).then(|| 153 + anim_id as u16)
}

/// The server announced this object's **despawn animation** (`SMSG_GAMEOBJECT_DESPAWN_ANIM`,
/// opcode `0x215`) — the one-shot channel's code 6, [`ANIM_DESPAWN`]. Inserted by the net apply
/// layer at packet time so it is on the entity *before* the `SMSG_DESTROY_OBJECT` that vmangos
/// sends in the same server tick is processed: that ordering is the whole mechanism, and it is
/// why this is a component rather than a message (a message is not read until later in the frame,
/// by which time the destroy has already freed the entity).
///
/// It is **not** GameObject-only — `WorldObject::SendObjectDeSpawnAnim` also fires for a totem's
/// death and a DynamicObject's expiry, and two vmangos boss scripts send it for objects that keep
/// living. So this marker asserts nothing on its own: it arms [`GoAnim::one_shot`] if there is a
/// [`GoAnim`] to arm, and it only defers a destroy that actually arrives ([`PendingDestroy`]).
#[derive(Component)]
pub(crate) struct DespawnAnimAnnounced;

/// The reference's **pending-destroy mark** — `[GO+0xe4]` bit `0x10`, set by the object-manager
/// destroy `0x464920` when it finds the object still pinned.
/// The arm takes the pin whenever the armed
/// substate is not a resting one (`0x5f3b27 call 0x4683e0` → refcount `[obj+0xe8]`), slot 14's
/// footer releases it at the window end (`0x468410`), and only then does the deferred destroy run
/// (`0x46844a call 0x464920`) — which is how an object gets to finish its own despawn animation
/// after the server has already told the client it is gone.
///
/// benilla takes the pin on the ONE substate that produces an observable — 12 / [`ANIM_DESPAWN`],
/// where the whole play happens after the destroy — and [`release_despawn_pin`] is the release.
/// A GO destroyed mid-transition (a door frozen half-open by a despawn) is the reference's other
/// pinned case and still pops instantly here; named, not built.
///
/// One deliberate divergence: the reference's `0x464920` returns *before* removing the object from
/// the manager, so a pinned object stays addressable by guid for the length of its play. benilla
/// drops the guid from the index at destroy time and keeps only the entity, because a respawn that
/// reused the guid would otherwise refresh an object already condemned to despawn. Nothing
/// observable rides on it — the object still draws, and the server has stopped answering for that
/// guid anyway.
#[derive(Component)]
pub(crate) struct PendingDestroy;

/// Caller 5 (the despawn-anim opcode): arm the one-shot [`ANIM_DESPAWN`] play on an object the
/// server has announced as despawning — one arm per announcement, exactly as the reference's
/// single `0x5f8c50(GO, 6)` call is one arm per packet.
///
/// `Changed`, not `Added`, and the difference is not pedantry: re-inserting a component that is
/// already present marks it changed but **not** added, so an object announced a second time
/// without having died in between (`SendObjectDeSpawnAnim` is a `WorldObject` method — two vmangos
/// boss scripts call it on objects that go on living) would silently never arm again.
///
/// A marked entity with no [`GoAnim`] (a totem, a DynamicObject, a GO type off the machine) simply
/// isn't matched, and [`release_despawn_pin`] then pops it on its first pass — the ordinary instant
/// destroy. Model ownership is judged at play time by [`drive_go_anim`], as on the Custom channel.
fn arm_despawn_anim(mut gos: Query<&mut GoAnim, Changed<DespawnAnimAnnounced>>) {
    for mut go in &mut gos {
        go.one_shot = Some(ANIM_DESPAWN);
    }
}

/// Release the pin: an object whose destroy was deferred ([`PendingDestroy`]) goes away as soon as
/// it is no longer *playing* its despawn animation — the reference's `0x468410` decrement reaching
/// zero with the pending-destroy bit set, which runs the deferred `0x464920`.
///
/// Runs AFTER [`drive_go_anim`], and that ordering is load-bearing in both directions. On the
/// arming frame the drive has just set [`GoAnim::transient`], so the object is held; on the frame
/// [`retire_transient_anim`] clears it (the window ended) nothing re-arms 157, so the release runs.
/// An entity that never armed anything — no [`GoAnim`], a model that doesn't author 157 — fails the
/// test on its very first pass and is released the same frame, which is the case for everything
/// that isn't an egg.
///
/// **"Released" is a fade, not a pop**. What the deferred `0x464920` runs into is
/// the base OnDeactivate `0x6145e0`, which hands the object's model to the `SWModelFadeout`
/// scheduler `0x672df0` on its way out — so the object stops existing while its model keeps
/// drawing and ramps to zero. A looted chest whose static model authors no `Despawn` sequence is
/// exactly this path with no animation in front of it, and popping it was the report that found
/// the missing hop. [`DespawnFade`](benilla_world::model_fade::DespawnFade) is that hand-off; it
/// arms once and drives itself, so the pin comes off with it and this query stops matching. The
/// hand-off is [`crate::net::tear_down`], the same one both wire routes take, so the object's
/// identity ends here too — a pinned object is still an object while it plays, and stops being
/// one when the pin drops.
fn release_despawn_pin(
    mut commands: Commands,
    pinned: Query<(Entity, Option<&GoAnim>), With<PendingDestroy>>,
) {
    for (e, go) in &pinned {
        if go.is_some_and(|g| g.transient.is_some_and(|t| t.id == ANIM_DESPAWN)) {
            continue;
        }
        commands
            .entity(e)
            .try_remove::<PendingDestroy>()
            .queue_silenced(crate::net::tear_down);
    }
}

/// Caller 3 (the loot-release): close a chest lid when its loot window closes. The client sends
/// `CMSG_LOOT_RELEASE` and immediately drops the state to READY(1) — no server round-trip. We watch the
/// open loot source guid change (any close path: the player's close, or the server's release when the last
/// item is looted) and close the lid iff the guid that just closed is an animated GO — a looted corpse or
/// creature resolves to an entity without [`GoAnim`], so `get_mut` misses it and nothing happens.
fn close_go_lid(
    loot: Res<crate::ui_loot::LootState>,
    index: Res<GuidIndex>,
    mut gos: Query<&mut GoAnim>,
    mut last_source: Local<Option<u64>>,
) {
    let current = loot.source();
    if *last_source == current {
        return;
    }
    if let Some(closed) = *last_source {
        if let Some(&e) = index.0.get(&closed) {
            if let Ok(mut anim) = gos.get_mut(e) {
                anim.state = Some(GO_STATE_READY);
            }
        }
    }
    *last_source = current;
}

/// The **completion-driven retire** of the current TRANSIENT substate — the substate advance.
/// The reference registers a per-model completion callback at GO model attach
/// (`0x5f7d43` → `[M2+0x70]`) which the driver `0x719370` fires ONCE when the armed sequence
/// reaches its baked window end — span × replay-count, the **loop bit ignored** — and slot 14
/// `0x5f4120` dispatches on the current substate:
///
/// | completed substate | advances to | i.e. |
/// |--------------------|-------------|------|
/// | 2 Open             | 3 Opened    | the swing settles open |
/// | 4 Close            | 1 Closed    | the lid settles shut |
/// | 5 Destroy          | 6 Destroyed | |
/// | 7 Rebuild          | 1 Closed    | |
/// | 0 Spawn, 8..11 Custom0-3 | re-run at the current state (`0x5f4190`) | |
/// | 1/3/6 (a rest pose) | **itself** (`0x5f4167`) | the held pose re-arms, with a fresh variation roll |
///
/// **This — not the animation kernel — is what turns a transition clip into a resting state**, and
/// it is why the kernel's loop bit is not the transition's duration: `G_Crate01`'s Close is
/// `flags` bit 0 clear, so the *pose* would wrap for ever, and only this advance ends it. For the
/// bobber's Custom0 the re-run resolves 149 → 151 → the lookup's all-fallback rows → **Stand**,
/// landing one frame after the 1333 ms window — before the looping kernel's second `$GC0` crossing
/// at 1533 ms. Net law: one splash per 0xB3, one swing per state change, then the state pose.
///
/// **The last row is not "nothing", and 1151 read it as such.** `0x5f4167` is byte-for-byte slot
/// 34's own selection and calls `0x5f3930` **directly**, bypassing slot 34's change guard on an
/// unchanged `[handler+0x10]` — so a resting substate re-issues its own op4, and with it a fresh
/// `variationIdx = -1` roll, every `span × R` ms for ever. The completion that drives it ignores
/// the loop bit (`0x719503` tests it only after the notify block), so a bit-0-CLEAR pose completes
/// too. That is why Onyxia's lava traps spurt *continuously* rather than once at stream-in, and it
/// is the same law units and placed doodads follow.
///
/// Benilla therefore arms EVERY clip `Never`-repeat (one window, the same endpoint), so "the
/// window ended" is the player's finished flag; the retire then clears `shown`, which makes the
/// state arm of [`drive_go_anim`] re-resolve the current state as a fresh rest pose — our
/// state-machine re-run, and (for a motion) exactly the table above, since `rest_anim(state)` is
/// the destination row of whichever motion that state's change armed. Runs before
/// [`drive_go_anim`] in the chain so the re-arm lands the same frame. Reads never deref-mut, so a
/// quiet GO stays out of the Changed stream.
fn retire_transient_anim(
    mut gos: Query<(Entity, &mut GoAnim, &AnimationPlayer)>,
    mut dispatch: MessageWriter<GoStateDispatch>,
) {
    for (entity, mut go, player) in &mut gos {
        // The ARMED node, not `find(id)`'s head variation — see [`Transient::node`].
        let done = |n| {
            player
                .animation(n)
                .is_none_or(bevy::animation::ActiveAnimation::is_finished)
        };
        if let Some(t) = go.transient {
            if done(t.node) {
                go.transient = None;
                // The completion's state-machine re-run: forget the shown pose so the state arm
                // re-resolves it (a silent rest snap — `resolve(None, state)`), exactly the
                // reference's re-arm over the finished transient block.
                go.shown = None;
                // A Spawn/Custom completion re-enters `0x5f3cb0` with `old == new`, and that
                // dispatch drops the object's display-sound loop — which is what ends the
                // GnomeMachine's Custom0 hum at the end of its clip rather than leaving it
                // droning. A transition motion's completion takes a different slot-14 row and
                // does not.
                if completion_redispatches(t.id) {
                    dispatch.write(GoStateDispatch(entity));
                }
            }
            continue;
        }
        // A REST pose completes too, and its completion re-arms *itself* — see [`drive_go_anim`]'s
        // rest arm. Same device: forget the shown pose and the state arm re-resolves it, rolling a
        // fresh variation. `rest_window` is cleared first so a refused re-arm settles instead of
        // spinning.
        if let Some(node) = go.rest_window {
            if done(node) {
                go.rest_window = None;
                go.shown = None;
            }
        }
    }
}

/// **The GameObject arm's instrument** (`WOW_MOVE_TRACE_TAGS=goa`) — one line per arm, carrying
/// everything the roll decided: the requested `AnimationData.dbc` id, what the remap turned it
/// into, the `_rand` draw itself, and the **file sequence slot** the weighted walk landed on.
///
/// That last column is the one this exists for. A GameObject's variation chain is not cosmetic:
/// the per-sequence material lanes (2295) key their UV and tint loops by `seq_index`, so *which
/// take is armed* is what decides whether a Blood of Heroes pool bubbles at all (slot 0 authors no
/// texture transform, slot 1 does) and whether an Onyxia lava trap spurts. From outside, an arm
/// that re-rolls every window and one that armed once and stalled look identical — both are "a
/// still pool" for as long as you happen to watch — and the three legs that produce the difference
/// (`0x5f39fa`'s already-armed skip, the collapse-to-0 leg that jumps past it, the completion that
/// re-enters through slot 14) are invisible in a screenshot. One line per arm makes the cycle,
/// its period and its distribution readable directly.
fn trace_arm(
    entity: Entity,
    lane: &str,
    requested: u16,
    want: u16,
    frozen: bool,
    roll: u16,
    clip: &benilla_assets::AnimClip,
) {
    if !benilla_assets::trace::enabled_for("goa") {
        return;
    }
    benilla_assets::trace::line(
        "goa",
        &format!(
            "{lane} e={entity} id={requested}->{want}{} roll={roll} seq={} freq={} dur={:.3}",
            if frozen { " frozen" } else { "" },
            clip.seq_index,
            clip.frequency,
            clip.duration,
        ),
    );
}

/// Play the sequence for a change of the client-side [`GoAnim::state`] (written by any of the three
/// callers). Mirrors the state-transition detection of [`crate::sound::gameobject`] (first sight silent),
/// but points it at the model instead of the mixer — one system owns the visual, the other the audio.
fn drive_go_anim(
    mut gos: Query<
        (
            Entity,
            &mut GoAnim,
            &mut AnimationPlayer,
            &mut AnimationTransitions,
            &ModelAnimations,
        ),
        Changed<GoAnim>,
    >,
    mut dispatch: MessageWriter<GoStateDispatch>,
    // The client's single `_rand` stream (`0x7400e5`), as `creature_anim` keeps
    // one: op4's variation roll draws from it on every GameObject arm below.
    mut rng: ResMut<benilla_assets::AnimRng>,
) {
    for (entity, mut go, mut player, mut tr, anims) in &mut gos {
        // ── The state arm ──────────────────────────────────────────────────────────────────────
        if let Some(state) = go.state {
            if go.shown != Some(state) {
                let prev = go.shown;
                go.shown = Some(state);
                // `0x5f3cb0` proper — announced before anything below can refuse the arm, because
                // the reference releases the display-sound loop at the head of the dispatch and
                // *then* picks a substate. A re-resolve driven by [`retire_transient_anim`]
                // leaves `dispatched` alone and so announces nothing (see [`GoAnim::dispatched`]).
                if go.dispatched != Some(state) {
                    go.dispatched = Some(state);
                    dispatch.write(GoStateDispatch(entity));
                }
                // A fresh substate replaces whatever transient one was live — the reference keeps
                // exactly ONE (`[handler+0x10]`), so an old motion's completion can never fire
                // over the pose that superseded it.
                go.transient = None;
                go.rest_window = None;
                if let Some(play) = resolve(prev, state) {
                    // Resolve the id to this model's clip (keyed by AnimationData.dbc id, as
                    // `creature_anim` does), through the remap for a model that doesn't author
                    // it — that is what keeps a lidless model on a real pose instead of bind.
                    let (want, frozen) = remap_missing(anims, play.anim_id());
                    // **`variationIdx = -1` — the arm ROLLS a variation** (`0x5f3aee: push -1`),
                    // unlike the loader seed
                    // beneath it, which passes an explicit `0` (`0x710189`). Where a model authors
                    // one sequence per id the two are the same clip and nothing changes; where it
                    // authors a chain they are not, and taking the head is a whole authored
                    // behaviour that never plays. Onyxia's lava traps are the case that named it:
                    // `ONYZIASLAIRLAVATRAP.M2` authors Stand twice — seq 0 (`freq` 29491, looping,
                    // silent) and seq 1 (`freq` 3276, one-shot) — and it is seq **1** that carries
                    // the 300/s `LAVALUMP2` burst, the lava spurting out of the floor. Every one of
                    // the 208 trap GameObjects in the lair took the head, so the chamber's whole
                    // ember field was missing.
                    // **The already-playing skip, and it decides settle-vs-cycle.** Three
                    // edges reach the arm (`0x5f3a0b`): the model OWNS the table id
                    // (`0x5f396c jne`), the model is not live, and the 147-absent collapse to
                    // Stand (`0x5f3a52 xor esi,esi; 0x5f3a54 jmp 0x5f3a0b` — a backwards jump
                    // PAST the check). Only the other remap legs fall through `0x5f39fa`'s
                    // `cmp esi,eax; je 0x5f3b32`, and there a re-arm requesting what is already
                    // armed is refused — so a prop the remap sent to a *non-zero* substitute
                    // settles on it (the fishing bobber), while one that owns its id or collapsed
                    // to Stand re-arms, and re-rolls, every window (the lava trap).
                    let skip =
                        want != 0 && !anims.owns(play.anim_id()) && go.armed_id == Some(want);
                    if skip {
                        continue;
                    }
                    let roll = rng.draw();
                    if let Some(clip) = anims.pick_variation(want, roll) {
                        trace_arm(entity, "state", play.anim_id(), want, frozen, roll, clip);
                        go.armed_id = Some(want);
                        // Snap a rest pose (blend 0 — a stream-in must not swing); ease a motion
                        // over its authored blend.
                        let blend = match play {
                            Play::Rest(_) => 0.0,
                            Play::Motion(_) => clip.blend_time.max(0.0),
                        };
                        let node = clip.node;
                        let active = tr.play(&mut player, node, Duration::from_secs_f32(blend));
                        if frozen {
                            // A motion standing in for a missing rest pose: the reference arms it
                            // at playback rate 0, so it holds frame 0 forever — the pose that
                            // motion departs from.
                            active.seek_to(0.0);
                            active.set_speed(0.0);
                            active.set_repeat(RepeatAnimation::Never);
                        } else {
                            // Explicit, not defaulted: `AnimationTransitions::play` →
                            // `AnimationPlayer::start` only `replay()`s the node, which rewinds the
                            // clock but keeps `speed` — so re-arming a node a frozen leg previously
                            // parked at rate 0 (the same Open clip serves both) would stay stuck.
                            active.set_speed(1.0);
                            // **ONE baked window, rest pose included** (decision 1151 for the
                            // motions; correcting 1151's "nothing advances off
                            // a held pose"). The completion notification `0x719370` fires at
                            // `span × R` **without consulting the loop bit** (`0x719503` tests it
                            // only afterwards), so a bit-0-CLEAR rest clip completes too — and
                            // slot 14's row for a resting substate, `0x5f4167`, calls `0x5f3930`
                            // *directly*, past slot 34's change guard, on an unchanged substate.
                            // The arm is therefore re-issued every ~`span` ms with a fresh
                            // `variationIdx = -1` roll, for ever. For a single-variation id that
                            // is the loop it always was, restarted at its own band start; for a
                            // chain it is the whole point — Onyxia's traps re-roll every 867 ms at
                            // a 10 % weight, and with a 5 s ember life that leaves **47 % of them
                            // holding a live plume at any instant** (measured, `WOW_PARTICLE_CENSUS`
                            // in the lair: 49 of 104), instead of one flurry at stream-in. The
                            // period (`0x7126d8`), the re-roll and `R = 1` from a `(0,0)` replay
                            // pair (`0x7126c3`) are all settled in the binary; what is *not*
                            // settled is which per-frame advance list a GameObject's model rides.
                            active.set_repeat(RepeatAnimation::Never);
                            match play {
                                Play::Motion(_) => {
                                    go.transient = Some(Transient { id: want, node });
                                }
                                Play::Rest(_) => go.rest_window = Some(node),
                            }
                        }
                    }
                    // else: nothing playable even after the remap — hold whatever the loader
                    // seed armed.
                }
            }
        }
        // ── The one-shot Custom channel (`0x5f8c50`) — AFTER the state arm, so the
        // bobber's same-frame pair (forced READY→ACTIVE flip + splash) lands splash-on-top.
        // Gated on the model OWNING the id; no remap on this channel — an unowned Custom
        // plays nothing. Armed for ONE window regardless of the sequence's loop flag (decision
        // 1099, correcting 1090's loop-forever): the kernel does loop a bit0-clear clip and
        // re-fires its events per pass, but the reference's COMPLETION callback fires at window
        // end (span × replay-count; the bobber's replay pair is 0..0 ⇒ one 1333 ms pass) and
        // re-runs the state machine, overwriting the Custom block before a second pass begins.
        // `RepeatAnimation::Never` + [`retire_transient_anim`] reproduce that endpoint exactly: one
        // pass, one `$GC0` splash, then the state pose — never a churning loop.
        // (`is_some` pre-check: an unconditional `take()` mut-derefs the `Mut` and re-marks the
        // component Changed every frame, keeping this Changed-filtered query hot forever.)
        if go.one_shot.is_some() {
            let id = go.one_shot.take().expect("checked is_some");
            if anims.owns(id) {
                // Slot 15 funnels through the same slot-34 → `0x5f3930` arm as the state channel,
                // so it rolls a variation for the same reason — a Custom0 with two authored
                // takes alternates them.
                let roll = rng.draw();
                if let Some(clip) = anims.pick_variation(id, roll) {
                    trace_arm(entity, "custom", id, id, false, roll, clip);
                    go.armed_id = Some(id);
                    go.rest_window = None;
                    let node = clip.node;
                    let active = tr.play(
                        &mut player,
                        node,
                        Duration::from_secs_f32(clip.blend_time.max(0.0)),
                    );
                    active.set_speed(1.0);
                    active.set_repeat(RepeatAnimation::Never);
                    go.transient = Some(Transient { id, node });
                }
            }
        }
    }
}

/// Gate a door/button's collision on its state: solid when **closed** (`GO_STATE_READY`), passable when
/// **open**. The model's collision is a single static hull (not bone-bound — it *can't*
/// swing with the mesh), so an open door is made walkable by disabling the collider, not by moving it —
/// the client's only option, and the faithful one. Keyed off the **wire** state (a door's real
/// passability is the server's, and it holds for an animation-less door too — no [`GoAnim`] required), and
/// scoped to the door/button types ([`collision_follows_state`]); a chest keeps its collider whatever its
/// state.
///
/// **Reconciles on a `GAMEOBJECT_STATE` edge OR on the collider's arrival** (idempotent either way).
/// The `Added<Collider>` half is load-bearing, not belt-and-braces: a streamed
/// GameObject's descriptor lands with the create block, but its `Collider` is baked from the M2
/// hull and inserted by `entities::attach` only when the **asset finishes loading**, frames later.
/// Watching the descriptor alone meant the two conditions were never true in the same frame
/// for a freshly streamed object — the descriptor changed while there was no collider, the collider
/// arrived while the descriptor was quiet — so the reconcile never ran at all, and every door kept
/// the enabled collider it was born with. A *closed* door is solid by default and looked correct;
/// an **open** one stayed solid, which is the entire reported symptom. It also explains the
/// reporter's workaround exactly: toggling the object from the GM panel changes the descriptor
/// *after* the collider exists, so the reconcile finally fires.
///
/// The descriptor half is the field edge on the one dword this reads — the same
/// seed-plus-edge pair [`sync_wire_go_state`] uses.
fn drive_go_collision(
    mut commands: Commands,
    gos: Query<(&ObjectStore, Has<ColliderDisabled>), With<Collider>>,
    arrived: Query<Entity, Added<Collider>>,
    mut edges: MessageReader<crate::net::FieldChanged>,
) {
    let state_edges = edges.read().filter(|e| {
        e.kind == benilla_protocol::messages::ObjectType::GameObject
            && e.index == benilla_protocol::field::FIELD_GAMEOBJECT_STATE
    });
    let mut due: bevy::ecs::entity::EntityHashSet = arrived.iter().collect();
    due.extend(state_edges.map(|e| e.entity));
    for entity in due {
        let Ok((store, disabled)) = gos.get(entity) else {
            continue;
        };
        if !collision_follows_state(store.0.gameobject_type_id()) {
            continue;
        }
        let solid = collider_is_solid(store.0.gameobject_state());
        if solid && disabled {
            commands.entity(entity).remove::<ColliderDisabled>();
        } else if !solid && !disabled {
            commands.entity(entity).insert(ColliderDisabled);
        }
    }
}

/// **A ghost walks through closed doors** — publish the door set the local mover's trace must
/// drop, which is [`benilla_world::collision::MoverTraceExclusions`]' only producer.
///
/// The reference builds a per-trace collision mask, and the local mover's mask gains bit `0x8000`
/// when the body it drives is a player carrying `PLAYER_FLAGS` bit `0x10` (GHOST) — `0x631658`,
/// off `OBJECT_FIELD_TYPE & TYPEMASK_PLAYER` plus the flag, pinned against the `UnitIsGhost`
/// registrar pair. The consumer is a single instruction, `0x5f85f6 test ah,ah` inside `0x5f85f0` —
/// `CGGameObject_C`'s collision-candidacy virtual at primary-vtable slot `+0x50` — which reads the
/// bit through the sign flag and answers *no candidate* for a GameObject whose
/// `GAMEOBJECT_TYPE_ID` is `0`, DOOR. Nothing else about a ghost's collision differs.
///
/// **Off the MOVER's descriptor, not ours.** The producer tests the object being traced, so a
/// possessed creature is not a player and never sets the bit; `player_is_ghost` reading a field a
/// creature has no block for answers false on its own, which is the same guard the byte order
/// gives.
///
/// **DOOR only** — not BUTTON, which shares the state-driven collider lane in
/// [`drive_go_collision`] but is a different type id, and the predicate is an equality against `0`.
/// Note that an absent `GAMEOBJECT_TYPE_ID` *is* DOOR (vmangos omits zero-valued fields), which is
/// why the scan is scoped by [`EntityKind::GameObject`]: read across every `ObjectStore` the same
/// absence would make a DOOR of every unit in the world.
fn publish_ghost_door_exclusions(
    mut exclusions: ResMut<benilla_world::collision::MoverTraceExclusions>,
    mover: Query<&ObjectStore, With<crate::net::Embodied>>,
    doors: Query<(Entity, &ObjectStore, &crate::net::NetEntity), With<Collider>>,
) {
    let ghost = mover.single().is_ok_and(|s| s.0.player_is_ghost());
    if !ghost {
        // The overwhelmingly common frame. Clearing only when non-empty keeps this off the
        // change-detection wire for the whole of a living session.
        if !exclusions.0.is_empty() {
            exclusions.0.clear();
        }
        return;
    }
    exclusions.0.clear();
    exclusions.0.extend(
        doors
            .iter()
            .filter(|(_, store, net)| ghost_passable(net.kind, store.0.gameobject_type_id()))
            .map(|(entity, _, _)| entity),
    );
}

/// Is this collider one a **ghost's** mover trace drops — a GameObject of type DOOR?
///
/// The kind test is not redundant with the type test, and that is the whole reason this is a named
/// function. An absent `GAMEOBJECT_TYPE_ID` reads as `0` = DOOR (vmangos omits zero-valued fields,
/// the law `go_templates` already documents), and a **unit** has no GameObject block at all — so a
/// scan that asked only about the type id would answer DOOR for every creature and player in the
/// world and quietly delete them from the mover's trace.
fn ghost_passable(kind: benilla_protocol::EntityKind, type_id: i32) -> bool {
    kind == benilla_protocol::EntityKind::GameObject && type_id == GO_TYPE_DOOR
}

/// What the GameObject event scanner reads per object: its clock, its park state, and the frame
/// its fired keys resolve in ([`crate::creature_anim::EventFrame`]).
type ScannedGo = (
    Entity,
    &'static ModelAnimations,
    &'static AnimationPlayer,
    Has<benilla_world::rig_anim::AnimParked>,
    &'static GlobalTransform,
    Option<&'static benilla_world::rig_anim::RigPose>,
);

/// Fire the event keyframes an animated GameObject's playing clip crossed this frame — the GO
/// half of the M2 event-kernel surface: the reference registers an event callback
/// per **family-A** GO at create
/// (`0x5f7d1f` → vtable `+0x30` → dispatcher `0x5f3e20`), which is exactly the [`GoAnim`]
/// population — a loader-idle family-B GO has no dispatcher and stays silent. The events flow
/// into the same [`AnimSoundEvent`] stream the creature scanner feeds: the generic `$SND`/`$DSO`/
/// `$DSL` audio arms apply as-is, and the GO-only display-slot family (`$GO0..5`/`$GC0..3` →
/// `GameObjectDisplayInfo.Sound[0..9]`) is routed by [`crate::sound::gameobject`]. The
/// load-bearing tenant: the fishing bobber's Custom0 clip authors `$GC0` at t≈3.87 s — the
/// splash sound the real client plays from the *animation*, beside the server's explicit
/// object-sound packet (the audible double, faithful to the reference on vmangos).
///
/// The playing clip is found as the creature scanner finds a variation track: of the clips with
/// a live [`AnimationPlayer`] play, the smallest seek is the newest arm (a cross-fade's fading
/// source is older by construction). The shared [`advance_track`]/[`scan_events`] helpers give
/// the same arming rules — an arm frame fires nothing, the frame after it fires the clip's `t = 0`
/// head, a loop wrap fires tail-then-head — and a frozen rate-0 leg never advances, so it never
/// fires.
fn fire_go_anim_events(
    gos: Query<ScannedGo, With<GoAnim>>,
    globals: Query<&GlobalTransform>,
    mut last: Local<crate::creature_anim::TrackMemory>,
    mut out: MessageWriter<AnimSoundEvent>,
) {
    for (entity, anims, player, parked, world, pose) in &gos {
        // The election's tick half, GO twin: a parked GO's event track is not
        // scanned — and there is no `MORE_AUDIBLE` exception here, because the flag lives on
        // CREATURE templates only (the reference's re-link arm reads the creature query cache).
        // Memory dropped so a wake re-arms instead of scanning the parked gap as one crossing.
        if parked {
            last.remove(&entity);
            continue;
        }
        let playing = anims
            .clips
            .iter()
            .filter_map(|c| player.animation(c.node).map(|a| (c, a.seek_time())))
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((clip, cur)) = playing {
            if let Some(prev) = advance_track(&mut last, entity, clip.node, cur) {
                let frame = crate::creature_anim::EventFrame {
                    world,
                    rig: pose.and_then(|p| Some((p, globals.get(p.joints_root).ok()?))),
                };
                scan_events(clip, entity, prev, cur, &frame, &mut out);
            }
        }
    }
}

/// Registration hook, mirrored on [`crate::sound::gameobject`]'s: the three state callers write
/// [`GoAnim::state`], then the animation + collision consumers act on it, after the Net drain wrote this
/// frame's descriptor deltas + queued the open-lock [`GoLidOpen`]. The event scanner runs last —
/// it reads the clip/seek state the drive just settled.
pub(crate) fn plugin(app: &mut App) {
    app.add_message::<GoLidOpen>()
        .add_message::<GoCustomAnim>()
        .add_message::<GoStateDispatch>()
        .add_systems(
            Update,
            (
                (
                    sync_wire_go_state,
                    open_go_lid,
                    close_go_lid,
                    queue_custom_anim,
                    arm_despawn_anim,
                    // The completion retire reads last frame's finished flags and must clear
                    // `shown` BEFORE the drive, so its state re-arm lands this frame — the
                    // reference's own one-frame-after-window-end timing.
                    retire_transient_anim,
                ),
                (drive_go_anim, drive_go_collision),
                // The pin release reads what the drive just armed — see its doc for why AFTER.
                release_despawn_pin,
                fire_go_anim_events,
            )
                .chain()
                .in_set(WorldStage::Present),
        );
    // Ahead of the mover, not with the GO drive above: the set it publishes is read by
    // `WorldCollision::cast_mover` inside the player controller, which runs in `Input`.
    // Publishing in `Present` would hand the mover the previous frame's doors — imperceptible,
    // but there is no reason to owe it.
    app.add_systems(
        Update,
        publish_ghost_door_exclusions.in_set(WorldStage::Input),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_world::model_fade::DespawnFade;

    /// **A ghost's mover drops DOOR GameObjects — and nothing else**.
    ///
    /// The third case is the one this test exists for. `GAMEOBJECT_TYPE_ID` is absent on a unit,
    /// and an absent field reads `0`, which *is* DOOR — so the type id alone says "door" for every
    /// creature and player in the world. Excluding those from the mover's trace would mean walking
    /// through units while dead, from a change that never mentions them.
    #[test]
    fn a_ghosts_trace_drops_doors_and_only_doors() {
        use benilla_protocol::EntityKind;

        assert!(
            ghost_passable(EntityKind::GameObject, 0),
            "a DOOR GameObject is what the reference's `0x5f85f0` refuses to make a candidate"
        );
        assert!(
            !ghost_passable(EntityKind::GameObject, 1),
            "a BUTTON is not — it shares the state-driven collider lane, not this one; the \
             reference's predicate is an equality against 0"
        );
        assert!(
            !ghost_passable(EntityKind::Unit, 0),
            "and a UNIT reading type 0 is the absent field, not a door: units keep colliding"
        );
        assert!(
            !ghost_passable(EntityKind::Player, 0),
            "same for a player — the absent-field trap is not unit-specific"
        );
    }

    /// The wire default that let open doors block. `None` must read ACTIVE/open,
    /// never "unknown" — vmangos omits the zero-valued field, so `None` IS the open state for
    /// every door that spawns open (ZG's Forcefield, Stratholme's small portcullises).
    #[test]
    fn an_absent_wire_state_is_open_not_unknown() {
        assert!(!collider_is_solid(None), "absent == ACTIVE(0) == passable");
        assert!(!collider_is_solid(Some(GO_STATE_ACTIVE)));
        assert!(collider_is_solid(Some(GO_STATE_READY)));
        // ALTERNATIVE (destroyed) is passable too — only READY is solid.
        assert!(!collider_is_solid(Some(2)));
    }

    #[test]
    fn rest_poses_match_the_verified_table() {
        assert_eq!(rest_anim(1), Some(0x93)); // READY  → Closed
        assert_eq!(rest_anim(0), Some(0x95)); // ACTIVE → Opened (not the 0x94 Open *motion*)
        assert_eq!(rest_anim(2), Some(0x97)); // ALT    → Destroyed
        assert_eq!(rest_anim(7), None);
    }

    #[test]
    fn transitions_play_the_motion_then_settle() {
        // closed → open swings the Open motion (0x94), open → closed the Close motion (0x92).
        assert!(matches!(resolve(Some(1), 0), Some(Play::Motion(0x94))));
        assert!(matches!(resolve(Some(0), 1), Some(Play::Motion(0x92))));
        // First sight snaps the rest pose, never a motion (a door streamed in open must not swing).
        assert!(matches!(resolve(None, 0), Some(Play::Rest(0x95))));
        assert!(matches!(resolve(None, 1), Some(Play::Rest(0x93))));
        // A change with no distinct motion snaps the destination rest pose.
        assert!(matches!(resolve(Some(2), 0), Some(Play::Rest(0x95))));
        // The destructible row dispatches on OLD too: only a CLOSED object swings Destroy; an open
        // one rests straight on Destroyed (`0x5f3cb0`'s condition is `OLD == 1`, not any OLD).
        assert!(matches!(resolve(Some(1), 2), Some(Play::Motion(0x96))));
        assert!(matches!(resolve(Some(0), 2), Some(Play::Rest(0x97))));
        // …and the rebuild leg stays the motion it always was.
        assert!(matches!(resolve(Some(2), 1), Some(Play::Motion(0x98))));
    }

    /// The custom-anim channel's wire mapping (`0x5f8930`): indices 0..3 arm
    /// Custom0..3 (AnimationData 153..156); anything else is the reference handler's reject.
    /// The fishing bobber's bite is index 0 → 153 — exactly the second sequence
    /// `G_FishingBobber.m2` authors.
    #[test]
    fn custom_anim_maps_the_wire_index_and_rejects_past_3() {
        assert_eq!(custom_anim_id(0), Some(153)); // Custom0 — the bobber splash
        assert_eq!(custom_anim_id(1), Some(154));
        assert_eq!(custom_anim_id(2), Some(155));
        assert_eq!(custom_anim_id(3), Some(156));
        assert_eq!(custom_anim_id(4), None);
        assert_eq!(custom_anim_id(u32::MAX), None);
    }

    /// The delta [`step_clock`] uses for the next frame — the same device as the creature
    /// driver's harness (`creature_anim/driver/tests.rs`) and for the same reason: with
    /// `TimePlugin` live, `Time`'s delta is the REAL gap between two `app.update()` calls, so a
    /// stalled frame on a loaded machine runs a whole clip out in one step and the assertions
    /// below become a coin flip. Here the clock is a number the test writes.
    #[derive(Resource, Default)]
    struct NextStep(Option<Duration>);

    fn step_clock(mut time: ResMut<Time>, mut next: ResMut<NextStep>) {
        time.advance_by(next.0.take().unwrap_or(Duration::from_millis(1)));
    }

    /// Run one frame whose delta is exactly `ms`.
    fn advance(app: &mut App, ms: u64) {
        app.world_mut().resource_mut::<NextStep>().0 = Some(Duration::from_millis(ms));
        app.update();
    }

    /// `G_Crate01`'s door family, as the bytes have it (pinned against the shipped asset by
    /// `benilla-formats/tests/m2_go_crate_lid.rs`): Open/Opened/Close/Closed, **all four `looping`**
    /// — `flags` bit 0 clear — with an empty replay range. Blend times are zeroed so the arm is a
    /// cut and the assertions read the armed clip, not a fade.
    const CRATE_FAMILY: [(u16, f32, u16); 4] = [
        (0x94, 0.666, 0),     // 148 Open   — the lid sweeps 0° → 75°
        (0x95, 0.100, 0),     // 149 Opened — holds 75°
        (0x92, 0.667, 0),     // 146 Close  — sweeps 75° → 0°
        (0x93, 0.167, 29491), // 147 Closed — holds 0° (the chain HEAD, weighted like the traps')
    ];

    /// An app running the two systems this file's law lives in, plus one GameObject wearing a
    /// crate's animation set: REAL `AnimationClip` assets and a real graph, so Bevy's own
    /// `advance_animations` ticks the completions [`retire_transient_anim`] watches.
    fn crate_app(extra_ids: &[(u16, f32, u16)]) -> (App, Entity) {
        use bevy::animation::graph::{AnimationGraph, AnimationGraphHandle};
        use bevy::animation::AnimationClip;

        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins.build().disable::<bevy::time::TimePlugin>(),
            AssetPlugin::default(),
            bevy::animation::AnimationPlugin,
        ));
        app.init_resource::<Time>();
        app.init_resource::<NextStep>();
        // The client's ONE `rand()` stream (2301) — the arm rolls a variation off it.
        app.init_resource::<benilla_assets::AnimRng>();
        // The state dispatch's audio consumer lives in `sound::gameobject`; here it is recorded
        // instead, so a test can assert *which* edges re-enter `0x5f3cb0`.
        app.add_message::<GoStateDispatch>();
        app.init_resource::<Dispatched>();
        app.add_systems(bevy::app::First, step_clock);
        app.add_systems(
            Update,
            (
                (arm_despawn_anim, retire_transient_anim),
                drive_go_anim,
                release_despawn_pin,
                record_dispatches,
            )
                .chain(),
        );

        let authored: Vec<(u16, f32, u16)> =
            CRATE_FAMILY.iter().chain(extra_ids).copied().collect();
        let handles: Vec<_> = authored
            .iter()
            .map(|&(_, dur, _)| {
                let mut c = AnimationClip::default();
                c.set_duration(dur);
                app.world_mut()
                    .resource_mut::<Assets<AnimationClip>>()
                    .add(c)
            })
            .collect();
        let (graph, nodes) = AnimationGraph::from_clips(handles);
        let graph = app
            .world_mut()
            .resource_mut::<Assets<AnimationGraph>>()
            .add(graph);

        let mut lookup = vec![0xffffu16; 160];
        for (slot, &(id, _, _)) in authored.iter().enumerate() {
            // A variation chain shares one id, and `animationLookup` holds its HEAD — so only the
            // first slot claiming an id writes here, exactly as the exporter bakes it.
            if lookup[id as usize] == 0xffff {
                lookup[id as usize] = slot as u16;
            }
        }
        let anims = ModelAnimations {
            graph: graph.clone(),
            clips: authored
                .iter()
                .zip(&nodes)
                .map(|(&(id, dur, frequency), &node)| benilla_assets::AnimClip {
                    anim_id: id,
                    seq_index: 0,
                    node,
                    // The asset fact this whole record turns on: the crate's transitions are
                    // bit-0-clear bands, i.e. the kernel loops them.
                    looping: true,
                    duration: dur,
                    move_speed: 0.0,
                    blend_time: 0.0,
                    bounds_center: Vec3::ZERO,
                    bounds_radius: 0.0,
                    bounds_min: Vec3::ZERO,
                    bounds_max: Vec3::ZERO,
                    events: Vec::new().into(),
                    arm_nodes: None,
                    upper_node: None,
                    frequency,
                    replay: (0, 0),
                    poses_bones: true,
                })
                .collect(),
            hand_close: [None, None],
            playable_animation_lookup: Vec::new(),
            animation_lookup: lookup,
            global_bones: Vec::new(),
            first_seq: None,
            pose: Default::default(),
        };
        // First sight is a closed chest, exactly as it streams in.
        let go = app
            .world_mut()
            .spawn((
                anims,
                AnimationPlayer::default(),
                AnimationTransitions::new(),
                AnimationGraphHandle(graph),
                GoAnim {
                    state: Some(GO_STATE_READY),
                    ..Default::default()
                },
            ))
            .id();
        (app, go)
    }

    /// Every [`GoStateDispatch`] the run has produced, in order — the audio consumer's input,
    /// stood in for.
    #[derive(Resource, Default)]
    struct Dispatched(Vec<Entity>);

    fn record_dispatches(mut msgs: MessageReader<GoStateDispatch>, mut out: ResMut<Dispatched>) {
        out.0.extend(msgs.read().map(|d| d.0));
    }

    /// What the object is actually playing: the armed `AnimationData` id and its repeat mode.
    fn armed(app: &App, go: Entity) -> Option<(u16, RepeatAnimation)> {
        let e = app.world().entity(go);
        let node = e.get::<AnimationTransitions>()?.get_main_animation()?;
        let id = e
            .get::<ModelAnimations>()?
            .clips
            .iter()
            .find(|c| c.node == node)?
            .anim_id;
        Some((
            id,
            e.get::<AnimationPlayer>()?.animation(node)?.repeat_mode(),
        ))
    }

    /// **The report**: loot the crate, close the loot window, and the lid must
    /// settle SHUT — not spring open and slam once a window, for ever.
    ///
    /// The whole door family is `flags` bit 0 clear, so a driver that arms a transition by the
    /// clip's loop bit arms Close on `Forever`: the lid jumps back to 75° every 667 ms. What ends
    /// a transition in the reference is the object layer's advance (slot 14 `0x5f4120`:
    /// substate 4 Close → 1 Closed), driven by the completion callback at the arm's baked window
    /// — the loop bit ignored. This runs the whole click-to-settle cycle through the real systems
    /// and real clips, and then keeps running: two seconds past the swing, three Close windows
    /// wide, the crate must still be holding Closed.
    #[test]
    fn the_looted_crate_settles_shut_instead_of_flapping_for_ever() {
        let (mut app, go) = crate_app(&[]);
        let state = |app: &mut App, s: u32| {
            app.world_mut()
                .entity_mut(go)
                .get_mut::<GoAnim>()
                .unwrap()
                .state = Some(s);
        };

        // Streamed in closed: the rest pose, snapped. It is ONE baked window like everything else
        // — `0x5f4167` re-arms a resting substate through `0x5f3930` at its own completion, so the
        // pose repeats by re-arming (and re-rolling its variation), not by the kernel's loop bit
        // (this corrects 1151's "nothing advances off a held
        // pose"). What must hold across those windows is the armed ID, asserted below.
        app.update();
        assert_eq!(armed(&app, go), Some((0x93, RepeatAnimation::Never)));

        // The open-lock cast lands: the lid swings open, ONE window.
        state(&mut app, GO_STATE_ACTIVE);
        app.update();
        assert_eq!(
            armed(&app, go),
            Some((0x94, RepeatAnimation::Never)),
            "the Open motion is a transient substate — one window, whatever its loop bit says"
        );

        // …and at its window end the machine advances 2 Open → 3 Opened on its own.
        advance(&mut app, 700);
        app.update();
        assert_eq!(
            armed(&app, go),
            Some((0x95, RepeatAnimation::Never)),
            "the completion advance settles the swing onto the Opened rest pose"
        );

        // The loot window closes (`CMSG_LOOT_RELEASE`): the lid swings shut, ONE window.
        state(&mut app, GO_STATE_READY);
        app.update();
        assert_eq!(armed(&app, go), Some((0x92, RepeatAnimation::Never)));

        // 4 Close → 1 Closed, and then it STAYS there — through many completion re-arms of the
        // Closed pose itself. Pre-1151 the Close *motion* was armed `Forever`, so this is the
        // assertion the director's crate report failed at, and the re-arm cycle must not
        // resurrect it.
        advance(&mut app, 700);
        app.update();
        assert_eq!(
            armed(&app, go),
            Some((0x93, RepeatAnimation::Never)),
            "the lid must settle on Closed"
        );
        for _ in 0..20 {
            advance(&mut app, 100);
            assert_eq!(
                armed(&app, go),
                Some((0x93, RepeatAnimation::Never)),
                "two seconds on — three Close windows — the crate is still shut"
            );
        }
    }

    /// **The arm rolls a VARIATION** — `variationIdx = -1` (`0x5f3aee: push -1`), not the id's
    /// head. The loader seed under it takes an explicit
    /// variation 0 (`0x710189`), so a model's second and later takes are reachable ONLY through
    /// this arm; resolving the id with `find()` made them unreachable everywhere.
    ///
    /// The subject is Onyxia's lava traps in miniature: a Closed chain authored 29491 : 3276, where
    /// the 10 %-weighted second take is the one that does something (there, a 300/s `LAVALUMP2`
    /// burst — the lava spurting out of the lair floor; 208 trap GameObjects all played the silent
    /// head). Spawning many objects makes the roll observable without a statistical test on one:
    /// the count must be neither 0 (the bug) nor all (a broken walk), and near the authored tenth.
    #[test]
    fn the_arm_rolls_a_weighted_variation_not_the_head() {
        const N: usize = 400;
        let (mut app, go) = crate_app(&[(0x93, 0.866, 3276)]);
        let (anims, player, tr, graph) = {
            let e = app.world().entity(go);
            (
                e.get::<ModelAnimations>().unwrap().clone(),
                AnimationPlayer::default(),
                AnimationTransitions::new(),
                e.get::<bevy::animation::graph::AnimationGraphHandle>()
                    .unwrap()
                    .clone(),
            )
        };
        let second = anims
            .clips
            .iter()
            .filter(|c| c.anim_id == 0x93)
            .nth(1)
            .expect("the chain's second take")
            .node;
        let mut gos = vec![go];
        for _ in 1..N {
            gos.push(
                app.world_mut()
                    .spawn((
                        anims.clone(),
                        player.clone(),
                        tr.clone(),
                        graph.clone(),
                        GoAnim {
                            state: Some(GO_STATE_READY),
                            ..Default::default()
                        },
                    ))
                    .id(),
            );
        }
        app.update();
        let rolled = gos
            .iter()
            .filter(|&&e| {
                app.world()
                    .entity(e)
                    .get::<AnimationPlayer>()
                    .unwrap()
                    .animation(second)
                    .is_some()
            })
            .count();
        assert!(
            rolled > 0,
            "not one of {N} arms reached the second take — the arm is taking the head, which is \
             the whole Onyxia lava-trap bug"
        );
        assert!(
            (N / 40..N / 4).contains(&rolled),
            "{rolled}/{N} on a 3276/32768 chain — the weighted walk is not weighting"
        );
    }

    /// **A rest pose is ONE window, and its completion re-arms it — with a FRESH roll, for ever**
    /// (correcting 1151's "nothing advances off a held
    /// pose"). `0x719370` fires the completion at `span × R` without consulting the loop bit
    /// (`0x719503` tests it only afterwards), and slot 14's resting row `0x5f4167` calls
    /// `0x5f3930` directly — past slot 34's change guard — on an unchanged substate.
    ///
    /// This is the half that makes Onyxia's Lair a *continuous* ember field rather than one flurry
    /// at stream-in: ~200 trap GameObjects each re-roll a 10 %-weighted Stand chain every 867 ms,
    /// against a 5 s particle lifespan. One object, many windows, is the same law in the small.
    #[test]
    fn a_rest_pose_re_arms_every_window_and_re_rolls_its_variation() {
        let (mut app, go) = crate_app(&[(0x93, 0.866, 3276)]);
        let nodes: Vec<_> = {
            let e = app.world().entity(go);
            e.get::<ModelAnimations>()
                .unwrap()
                .clips
                .iter()
                .filter(|c| c.anim_id == 0x93)
                .map(|c| c.node)
                .collect()
        };
        assert_eq!(nodes.len(), 2, "the chain is two takes");
        let mut seen = [0u32; 2];
        app.update();
        for _ in 0..300 {
            let player = app.world().entity(go).get::<AnimationPlayer>().unwrap();
            for (i, n) in nodes.iter().enumerate() {
                if player.animation(*n).is_some() {
                    seen[i] += 1;
                }
            }
            // Longer than either take's band, so whichever is armed completes and re-arms.
            advance(&mut app, 900);
        }
        assert!(
            seen[0] > 0 && seen[1] > 0,
            "over 300 windows the pose showed takes {seen:?} — a rest pose that never re-armed \
             would show exactly one, and Onyxia's floor would spurt once and go quiet"
        );
        // The pose itself never wanders off Closed: re-arming is not the substate advance.
        assert!(matches!(armed(&app, go), Some((0x93, _))));
    }

    /// **The state dispatch is an EDGE, and a rest pose re-arming itself is not one** (slot 14
    /// sends a resting substate to `0x5f4167`, which
    /// calls `0x5f3930` directly and never re-enters `0x5f3cb0`). That is the whole reason a
    /// brazier's `TorchLoop` — a looping display-slot kit registered by a `$GO2` on the Opened
    /// rest pose — hums continuously instead of being released and restarted every band pass,
    /// because the dispatch's first act is `0x5f3cc8 call 0x5f40c0`.
    ///
    /// What *does* dispatch: a genuine `SetGoState`, and a Custom substate's completion (slot
    /// 14's `0x5f4190` row, `old == new`) — which is what ends a Gnome machine's Custom0 hum.
    #[test]
    fn only_a_state_change_and_a_custom_completion_re_enter_the_dispatch() {
        let (mut app, go) = crate_app(&[(153, 0.667, 0)]); // Custom0 — the crate authors one
        app.update();
        let seen = |app: &App| app.world().resource::<Dispatched>().0.len();
        assert_eq!(seen(&app), 1, "the spawn's own SetGoState is a dispatch");

        // Fifty rest windows. The pose re-arms every one of them (the Onyxia law) and not one of
        // those re-arms may announce a dispatch.
        for _ in 0..50 {
            advance(&mut app, 200);
        }
        assert!(matches!(armed(&app, go), Some((0x93, _))), "still Closed");
        assert_eq!(seen(&app), 1, "a rest pose's own re-arm is not a dispatch");

        // A genuine SetGoState — the lid opens.
        app.world_mut()
            .entity_mut(go)
            .get_mut::<GoAnim>()
            .unwrap()
            .state = Some(GO_STATE_ACTIVE);
        app.update();
        assert_eq!(seen(&app), 2);
        // …and the Open motion settling onto Opened is slot 14's `0x5f413d` row, not the dispatch.
        // (Two ticks: the first runs the clip past its window, the next is where the retire sees
        // the finished flag — the shape every completion test in this file uses.)
        advance(&mut app, 700);
        app.update();
        assert_eq!(
            seen(&app),
            2,
            "a transition motion advancing to its rest pose does not re-enter the dispatch"
        );

        // A Custom0 block, whose completion does.
        app.world_mut()
            .entity_mut(go)
            .get_mut::<GoAnim>()
            .unwrap()
            .one_shot = Some(153);
        app.update();
        assert_eq!(seen(&app), 2, "arming a Custom is not itself a dispatch");
        advance(&mut app, 700);
        app.update();
        assert_eq!(
            seen(&app),
            3,
            "its completion re-runs the machine through `0x5f3cb0`"
        );
        assert!(
            app.world()
                .resource::<Dispatched>()
                .0
                .iter()
                .all(|&e| e == go),
            "every dispatch names the object it ran on"
        );
    }

    /// The Custom channel shares the ONE transient slot with the motions (the reference's
    /// `[handler+0x10]`), so 1100's bobber law has to keep holding through it: a Custom0 arms over
    /// the state pose, runs exactly one window, and the completion re-runs the machine back onto
    /// the state's own pose.
    #[test]
    fn a_custom_block_still_runs_one_window_and_hands_back_to_the_state() {
        let (mut app, go) = crate_app(&[(153, 0.667, 0)]); // Custom0 — the crate authors one
        app.update();
        assert_eq!(armed(&app, go), Some((0x93, RepeatAnimation::Never)));

        app.world_mut()
            .entity_mut(go)
            .get_mut::<GoAnim>()
            .unwrap()
            .one_shot = Some(153);
        app.update();
        assert_eq!(armed(&app, go), Some((153, RepeatAnimation::Never)));

        advance(&mut app, 700);
        app.update();
        assert_eq!(
            armed(&app, go),
            Some((0x93, RepeatAnimation::Never)),
            "one Custom window, then the state pose — never a churning loop"
        );
    }

    /// **B140**: UBRS's Rookery Eggs. Walk into a `TRAP`'s radius, the server
    /// spends its last charge and sends `SMSG_GAMEOBJECT_DESPAWN_ANIM` immediately followed by
    /// `SMSG_DESTROY_OBJECT` in the same tick — and the egg must **hatch before it pops**: 157
    /// Despawn, one window (2.667 s on `G_DragonEggFreeze`, whose bone 8 swells 4.76×), with the
    /// object held alive for exactly that long by the pin.
    ///
    /// The failing shape this pins is the whole report: the destroy freed the entity on arrival,
    /// so 271 eggs blinked out with no animation at all.
    #[test]
    fn an_announced_despawn_plays_its_window_before_the_object_pops() {
        let (mut app, go) = crate_app(&[(157, 2.667, 0)]);
        app.update();
        assert_eq!(armed(&app, go), Some((0x93, RepeatAnimation::Never)));

        // The wire pair, in the order vmangos sends it and Commands apply it.
        app.world_mut().entity_mut(go).insert((
            crate::net::Guid(0xF110_0000_0000_0E66),
            DespawnAnimAnnounced,
            PendingDestroy,
        ));
        app.update();
        assert_eq!(
            armed(&app, go),
            Some((157, RepeatAnimation::Never)),
            "the announcement arms 157 Despawn for ONE window"
        );
        assert!(
            app.world().get::<crate::net::Guid>(go).is_some(),
            "the pin holds the OBJECT alive across its own destroy, not just its model"
        );

        advance(&mut app, 2700);
        app.update();
        assert!(
            app.world().get::<DespawnFade>(go).is_some(),
            "the window ended — the pin drops and the deferred destroy hands the model to the fade"
        );
        assert!(
            app.world().get::<crate::net::Guid>(go).is_none(),
            "…and the deferred destroy is the teardown: the object ends as the fade begins"
        );
        assert!(
            app.world().get::<PendingDestroy>(go).is_none(),
            "and the pin comes off with it, so the release stops re-arming the fade"
        );
    }

    /// The other half of the same rule: a model that doesn't author 157 (or an object with no
    /// animation machine at all — a totem, a DynamicObject, both of which `SendObjectDeSpawnAnim`
    /// also fires for) has nothing to play, so the pin never forms and the release runs on its
    /// first pass — straight to the fade, with no animation in front of it. **This is the looted
    /// chest**: `DeadmineCargoBoxes.m2` is a fully static model, so the announced
    /// despawn resolves to nothing and the teardown fade is the whole observable.
    #[test]
    fn an_unownable_despawn_anim_goes_straight_to_the_fade() {
        let (mut app, go) = crate_app(&[]); // the crate family only — no 157
        app.update();

        app.world_mut()
            .entity_mut(go)
            .insert((DespawnAnimAnnounced, PendingDestroy));
        app.update();
        assert!(
            app.world().get::<DespawnFade>(go).is_some(),
            "nothing to play ⇒ the teardown fade, same frame — never a pop"
        );
    }

    /// The announcement **alone** kills nothing. `SendObjectDeSpawnAnim` lives on `WorldObject`,
    /// and two vmangos boss scripts send it for objects that go on living (Sapphiron sends it for
    /// *himself*); only a destroy that actually arrives marks [`PendingDestroy`]. So an announced
    /// object with no destroy plays its window and returns to its state pose, still there.
    #[test]
    fn an_announcement_without_a_destroy_never_despawns_anything() {
        let (mut app, go) = crate_app(&[(157, 2.667, 0)]);
        app.update();

        app.world_mut().entity_mut(go).insert(DespawnAnimAnnounced);
        app.update();
        assert_eq!(armed(&app, go), Some((157, RepeatAnimation::Never)));

        advance(&mut app, 2700);
        app.update();
        assert!(app.world().get_entity(go).is_ok(), "no destroy, no despawn");
        assert_eq!(
            armed(&app, go),
            Some((0x93, RepeatAnimation::Never)),
            "and the window hands back to the state pose, like any other one-shot"
        );
    }

    /// A `ModelAnimations` that owns exactly `ids` — only the lookup table matters to the remap.
    fn owning(ids: &[u16]) -> ModelAnimations {
        let hi = ids.iter().copied().max().unwrap_or(0) as usize;
        let mut lookup = vec![0xffffu16; hi + 1];
        for (slot, &id) in ids.iter().enumerate() {
            lookup[id as usize] = slot as u16;
        }
        ModelAnimations {
            graph: Handle::default(),
            clips: Vec::new(),
            hand_close: [None, None],
            playable_animation_lookup: Vec::new(),
            animation_lookup: lookup,
            global_bones: Vec::new(),
            first_seq: None,
            pose: Default::default(),
        }
    }

    #[test]
    fn an_owned_id_is_never_remapped() {
        let full = owning(&[146, 147, 148, 149]);
        for id in [146, 147, 148, 149] {
            assert_eq!(remap_missing(&full, id), (id, false));
        }
    }

    #[test]
    fn a_missing_rest_pose_freezes_the_neighbouring_motion() {
        // The Ahn'Qiraj gate roots: Stand + Open only, no Closed. The reference stands the Open
        // motion in at rate 0 — its frame 0 IS the closed pose. Before this, benilla played nothing
        // and the 42-bone root tangle rendered at bind pose.
        let roots = owning(&[0, 148]);
        assert_eq!(remap_missing(&roots, 147), (148, true));
        // The mirror leg: no Opened, no Open, but a Close motion → freeze Close at frame 0 (open).
        let no_open = owning(&[146]);
        assert_eq!(remap_missing(&no_open, 149), (146, true));
    }

    #[test]
    fn the_remap_falls_through_the_door_family_in_the_verified_order() {
        // 146 Close missing: kept when Open exists (op4 resolves onward), else Closed.
        assert_eq!(remap_missing(&owning(&[148]), 146), (146, false));
        assert_eq!(remap_missing(&owning(&[147]), 146), (147, false));
        // 147 Closed missing: kept when Close exists; else the frozen leg; else Stand.
        assert_eq!(remap_missing(&owning(&[146]), 147), (147, false));
        assert_eq!(remap_missing(&owning(&[0]), 147), (0, false));
        // 148 Open missing: kept when Close exists; else Destroy if present; else Opened.
        assert_eq!(remap_missing(&owning(&[146]), 148), (148, false));
        assert_eq!(remap_missing(&owning(&[150]), 148), (150, false));
        assert_eq!(remap_missing(&owning(&[0]), 148), (149, false));
        // 149 Opened missing with neither motion: Destroyed.
        assert_eq!(remap_missing(&owning(&[0]), 149), (151, false));
    }

    /// Slot 14's dispatch table (`0x5f41e4`): rows 0 and 8..11 land at `0x5f4190`, which re-runs
    /// the state machine **through `0x5f3cb0`** and so drops the object's display-sound loop; the
    /// motion rows (2/4/5/7) advance to their rest pose and the rest rows (1/3/6) re-arm
    /// themselves, neither of which touches it. Read as `AnimationData.dbc` ids, the key this
    /// file uses everywhere.
    #[test]
    fn only_spawn_and_the_custom_block_re_enter_the_state_dispatch() {
        for id in [145, 153, 154, 155, 156] {
            assert!(completion_redispatches(id), "substate id {id}");
        }
        // The four transition motions, the three rest poses — and Despawn (157), whose substate
        // 12 is past slot 14's own `cmp 0xb` bound and dispatches nowhere at all.
        for id in [146, 147, 148, 149, 150, 151, 152, 157] {
            assert!(!completion_redispatches(id), "substate id {id}");
        }
    }

    #[test]
    fn ids_outside_the_door_group_get_no_remap() {
        // `lea eax,[esi-0x92]; cmp eax,3; ja` — only 146..149 index the jump table.
        let none = owning(&[0]);
        for id in [145, 150, 151, 152, 157] {
            assert_eq!(remap_missing(&none, id), (id, false));
        }
    }

    #[test]
    fn ownership_reads_the_lookup_table_not_the_clip_list() {
        // The table is short — a model's `nAnimationLookup` stops just past its highest id, so an id
        // beyond the end is the out-of-bounds sentinel. `clips` is deliberately empty here: a clip
        // scan would answer "owns nothing" and take every remap's last leg.
        let book = owning(&[146, 147, 148, 149]);
        assert!(book.owns(147) && !book.owns(0) && !book.owns(150));
    }

    #[test]
    fn chest_animates_but_keeps_its_collider() {
        // A chest (3) is on the animation machine (2271) but off the collision gate: you see the
        // lid move, but you never walk through an open chest.
        assert!(go_animates(3));
        assert!(!collision_follows_state(3));
        // Doors/buttons are on both.
        assert!(go_animates(0) && collision_follows_state(0));
        assert!(go_animates(1) && collision_follows_state(1));
    }

    #[test]
    fn the_type_gate_is_the_verified_census() {
        // The 20 family-A types (0x1c-byte handler, 36-slot vtable with the real arm).
        for t in [
            0, 1, 2, 3, 6, 8, 9, 10, 12, 16, 17, 18, 19, 23, 24, 26, 27, 28, 29, 30,
        ] {
            assert!(go_animates(t), "type {t} runs the machine");
        }
        // The 11 family-B types, whose vtable slots are the abstract base's do-nothing bodies.
        for t in [4, 5, 7, 11, 13, 14, 15, 20, 21, 22, 25] {
            assert!(!go_animates(t), "type {t} keeps the loader seed");
        }
        // Past the table: the unsigned `ja` default, no handler.
        assert!(!go_animates(31) && !go_animates(99));
        // The three that used to be the whole gate are still on it; the ones it wrongly excluded —
        // TEXT (every book) and GOOBER — are the reported regressions this widening closes.
        assert!(go_animates(9) && go_animates(10));
        // Type 0 is also what an ABSENT type id reads as, so the default animates.
        assert!(go_animates(0));
    }
}
