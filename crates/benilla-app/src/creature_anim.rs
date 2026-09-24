//! Movement-driven character & creature animation (decision 0019, Milestone C; the locomotion controller
//! of decisions 0047 + 0049). Each animated unit carries a [`ModelAnimations`] component (its clips, by
//! `AnimationData.dbc` id), an [`AnimationPlayer`], and an
//! [`AnimationTransitions`][bevy::animation::transition::AnimationTransitions]; this module is a
//! small per-unit state machine that picks the animation from the unit's movement state, **cross-fades**
//! into it over the clip's blend-in time, and scales a locomotion clip's playback rate to the unit's
//! speed.
//!
//! Regimes (decisions 0049 → 0083/0087):
//! - **Gait** — the directly-cross-faded ground/swim gaits + idle: Stand / Walk / Run / Fast-run by speed,
//!   WalkBackwards when moving back, the swim ids when swimming (the client's selector `0x5fd8b0`).
//!   A change cross-fades to the new clip over its `blend_time` instead of snapping.
//! - **Special** — the stand-state poses (sit/sleep/kneel: down → loop → up) as one-shot-bracketed
//!   loops, **preemptible** (decision 0055), plus the jump's enter/hang. The jump's *landing* is NOT a
//!   bracket: [`select::Mode::Land`] is a plain pick from the input at touchdown, freely overwritten the
//!   instant a movement flag changes (decisions 0083/0087 — land-then-press runs immediately).
//! - **One-shots (swings, emotes)** — routed **per play from live state** (decision 0087, the
//!   byte-verified `0x5fe2f0` route): a standing-idle unit plays them full-body on the base track
//!   ([`select::Mode::Swing`]); a moving / seated / combat-airborne unit plays them on the SpineLow
//!   torso-masked overlay ([`AnimClip::upper_node`]) while the base keeps driving the legs underneath —
//!   never dropped, never bracket-gated.
//!
//! The playback **rate** of a locomotion clip is `speed / sequence.moveSpeed` (`0x5fe2f0`), so
//! a unit moving faster than the clip's design speed cycles its legs proportionally faster (the fix for a
//! backpedal that otherwise drags). Non-locomotion clips (idle, the jump/sit transitions — `moveSpeed 0`)
//! play at 1×. Death (decoded health 0) overrides everything.
//!
//! The movement view is unified per unit ([`select::unify`]): the self-avatar's [`MovementState`] is
//! filled by the player controller from input; a **remote player**'s comes from its relayed
//! `MSG_MOVE_*` stream (live CMovement flags + extrapolation speed, via [`crate::net::RemoteMotion`]),
//! so its walk/run/ strafe/backpedal/turn animate from the real flags; a **creature**'s is derived from
//! its server [`crate::net::Spline`] (forward + speed). Stand-state (sit/sleep/kneel/chairs) comes from
//! the unit's `UNIT_FIELD_BYTES_1` byte for streamed units, and from the controller (echo + in-flight
//! request) for our own avatar (decision 0080c).

use std::time::Instant;

use benilla_assets::{AnimClip, ModelAnimations};
use benilla_formats::AnimDataCatalog;
use bevy::prelude::*;

use benilla_assets::AssetSet;
use benilla_world::schedule::WorldStage;

/// The pure animation-selection logic (the `0x5fd100`/0073 tables, movement/Special state,
/// gait/swing/ready picks, playback-rate math) — kept in its own file as it carries the bulk of the
/// unit-tested selector logic, separate from the Bevy driver systems in [`driver`].
pub(crate) mod select;
pub(crate) use select::{
    ease_strafe_yaw, move_flags, strafe_body_offset, swim_body_rotation, MovementState,
};
use select::{Mode, Special};

/// The display-facing counter-twist (the strafe body pose): the [`BodyTwist`] component + the
/// post-animation system that composes the SpineLow/Head counter-rotation onto the frame's pose.
mod twist;
pub(crate) use twist::{wrap_pi, BodyTwist};

/// The unit animation-LOD gate (decision 0448): park an off-frustum rig's per-bone pose
/// evaluation — the clocks, the driver state machine, and the event tracks keep running, so
/// off-screen combat stays audible (0075) and a re-appearing unit snaps to the absolute-clock
/// pose. Shipped as a modernization, it is the faithful direction instead: the reference does not
/// tick an off-frustum unit (`0x683dd0`'s second walk) — decision 1473
/// records that, and what still diverges (parked rigs keep drawing; the reference keeps only
/// `MORE_AUDIBLE` creatures audible off-screen).
mod lod;

/// The unit's wielded weapon classes — `(item class, item subclass)` per hand, `None` for an empty
/// (or non-item) hand. Written by the held-item resolution ([`crate::entities`], decision 0072) from
/// the same descriptor data that places the weapon models; read by the swing/ready animation
/// selectors (decision 0073 — the byte-verified `GetWeapon` byte pair `0x605e30`).
///
/// **The two readings of a hand** (decision 1863). The reference's accessor takes a `visFlag`:
/// `GetWeapon(slot, 1)` is what is *worn* there, `GetWeapon(slot, 0)` is what the unit can
/// *fight* with — and the two differ by exactly one live descriptor bit, [`UNIT_FLAG_DISARMED`].
/// The fields below are the `visFlag == 1` reading — what the paperdoll and the dress-up model
/// show, the two surfaces that keep the weapon while disarmed because they read the flag nowhere.
/// Everything that decides how the unit *behaves* — swing, Ready idle, parry, the sheath machine,
/// the swing sound, the attached model — goes through [`Wielded::armed_main`] /
/// [`Wielded::armed_off`], which apply the ladder.
#[derive(Component, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Wielded {
    pub(crate) main: Option<(u8, u8)>,
    pub(crate) off: Option<(u8, u8)>,
    /// The **ranged-slot** item's `(class, subclass)` — the local auto-repeat idle's selector
    /// ([`select::ranged_load_anim`], the client's `0x5fd530` LUT; decision 0099 phase 5).
    pub(crate) ranged: Option<(u8, u8)>,
    /// The mainhand/offhand items' sheath *types* (1 back-2H · 2 back-staff · 3 hip · 4 shield ·
    /// 0 none) — each picks its arm's draw/stow one-shot independently (`0x611930`:
    /// `(1 << (rec[+4] & 0x1f)) & 0x88` → HipSheath 90 for types {3, 7},
    /// Sheath 89 otherwise; the slot's own byte is the *only* input).
    pub(crate) main_sheath: u8,
    pub(crate) off_sheath: u8,
    /// The ranged item's sheath type, same pick — the ranged **draw** clip reads it
    /// (`0x6118a0`/`0x611960`/`0x611a20` all run `(1 << (rng[+4] & 0x1f)) & 0x88` on the ranged
    /// record). Its **stow** clip does not: `0x611b60`'s PREV==2 legs push a literal `0x59`
    /// (`0x611c8c`/`0x611cd3`), so a bow is always put away with Sheath 89.
    pub(crate) ranged_sheath: u8,
    /// The ranged item's `InventoryType` — the byte-verified **arm** pick: `0x1a`
    /// (RANGEDRIGHT: gun/crossbow/wand) and `0x19` (THROWN) go to the right arm, everything else
    /// (`0x0f`, a bow) to the left (`0x611b60` @ `0x611c74`, and the same compare in both
    /// drawers). `0` when nothing is in the ranged slot.
    pub(crate) ranged_inv: u32,
    /// Each slot's `Material` id — the **only** key the draw/stow sound pick uses
    /// (`SheatheSoundLookups`, decision 0882). Not derivable from the weapon class: real 5875 data
    /// has maces (subclass 4) in both metal and wood, and puts every bow, crossbow and wand in
    /// wood while guns and thrown are metal. It rides the wire — `SMSG_ITEM_QUERY_SINGLE_RESPONSE`
    /// for players, the `UNIT_VIRTUAL_ITEM_INFO` byte triple for creatures — so nothing here is a
    /// guess. Indexed by held slot: 0 mainhand · 1 offhand · 2 ranged.
    pub(crate) materials: [u8; 3],
    /// `UNIT_FIELD_FLAGS & `[`UNIT_FLAG_DISARMED`] — a rogue's Disarm, up on the unit's own
    /// descriptor and therefore replicated for **every** unit, not just ours (CGUnit's ladder is
    /// byte-identical to CGPlayer's, so a disarmed creature behaves exactly like a disarmed
    /// player). Resolved beside the hands because that is where the reference reads it: inside
    /// `GetWeapon` itself (`[[unit+0x110]+0xa0] & 0x200000`), off the same descriptor these pairs
    /// come from.
    pub(crate) disarmed: bool,
}

/// `UNIT_FIELD_FLAGS` bit 21 — **`UNIT_FLAG_DISARMED`** (vmangos `UnitDefines.h`), read by the
/// reference inside `GetWeapon` at `0x5ec2b8` (CGPlayer) and `0x605e58`/`0x605e98` (CGUnit):
/// `test dword ptr [ecx+0xa0], 0x200000`. `0xa0 = 4 × 0x28` pins it to field `UNIT_FIELD_FLAGS`
/// off the descriptor base `[unit+0x110]`.
pub(crate) const UNIT_FLAG_DISARMED: u32 = 0x0020_0000;

/// `ItemClass` 2 — **WEAPON**. The class byte the reference's disarm ladder tests, and the whole
/// of its test: no subclass, no inventory type, nothing else. Disarm takes weapons, so a shield or
/// a held-in-off-hand trinket is never the hand it hides.
pub(crate) const ITEM_CLASS_WEAPON: u8 = 2;

/// **The disarm ladder** — which single held slot `UNIT_FLAG_DISARMED` hides, given what each
/// melee hand holds (decision 1863; both class bodies identical).
///
/// The reference does not "empty a disarmed unit's hands". `GetWeapon(slot, visFlag = 0)` runs a
/// **two-probe ladder with main-hand precedence**, and the off-hand probe's polarity is the
/// opposite of the main hand's:
///
/// ```text
/// ; slot 0 (MAIN HAND) — one probe, 0x5ec2ae
/// 5ec2b8: test dword ptr [ecx+0xa0],0x200000  ; disarmed?
/// 5ec2cc: call [edx+0x98]                     ; GetWeapon(0, 1) — vis=1 skips the gate
/// 5ec2d6: cmp byte ptr [eax],2 ; je 0x5ec346  ; class 2 -> RETURN NULL
///
/// ; slot 1 (OFF HAND) — two probes, 0x5ec262, MAIN HAND asked FIRST
/// 5ec280: call [edx+0x98]                     ; GetWeapon(0, 1)   <<< slot 0, not 1
/// 5ec28a: cmp byte ptr [eax],2 ; je 0x5ec2aa  ; main hand IS a weapon -> PROCEED (keep the off hand)
/// 5ec297: call [eax+0x98]                     ; GetWeapon(1, 1)
/// 5ec2a1: cmp byte ptr [eax],2 ; je 0x5ec346  ; off hand IS a weapon -> RETURN NULL
/// ```
///
/// So **exactly one weapon is hidden — the first one found scanning main hand → off hand** — and
/// a disarmed dual-wielder keeps swinging their off-hand weapon, model and all. vmangos agrees
/// from the other side: `Unit::CanUseEquippedWeapon` returns `!DISARMED` for `BASE_ATTACK` and an
/// unconditional `true` for `OFF_ATTACK` and `RANGED_ATTACK`. The **ranged slot is never gated**
/// (`0x5ec25e` pushes equip index `0x11` and jumps past both tests).
///
/// Stated over classes rather than over [`Wielded`] because two layers need the same law from two
/// different data sources — the animation layer off the resolved hands, the action bar's
/// equipped-item test off the raw inventory (`0x5ea5d0`'s slot 0/1/2 loop).
pub(crate) fn disarmed_hand(main_class: Option<u8>, off_class: Option<u8>) -> Option<usize> {
    if main_class == Some(ITEM_CLASS_WEAPON) {
        return Some(0);
    }
    if off_class == Some(ITEM_CLASS_WEAPON) {
        return Some(1);
    }
    None
}

impl Wielded {
    /// The held slot this unit's disarm hides right now — [`disarmed_hand`] over its own hands,
    /// `None` while the flag is down. The equipment layer reads it to know which weapon the
    /// reference's flag reflex takes off the hand ([`crate::entities`]).
    pub(crate) fn disarmed_hand(&self) -> Option<usize> {
        self.disarmed
            .then(|| disarmed_hand(self.main.map(|(c, _)| c), self.off.map(|(c, _)| c)))
            .flatten()
    }

    /// The **mainhand** as `GetWeapon(0, 0)` sees it — the reading the swing (`0x6246a0`), the
    /// Ready idle (`0x5fcdc0`), the parry LUT, the sheath machine and the swing sound all take.
    pub(crate) fn armed_main(&self) -> Option<(u8, u8)> {
        self.main.filter(|_| self.disarmed_hand() != Some(0))
    }

    /// The **offhand** as `GetWeapon(1, 0)` sees it. Nulled only when the main hand did *not*
    /// claim the disarm — see [`disarmed_hand`].
    pub(crate) fn armed_off(&self) -> Option<(u8, u8)> {
        self.off.filter(|_| self.disarmed_hand() != Some(1))
    }
}

/// The unit is engaged in melee auto-attack (`SMSG_ATTACKSTART` .. `ATTACKSTOP`, decision 0073):
/// standing still it plays the weapon-class Ready idle — the client's `0x5fd360` arm gates on the
/// auto-attack-target GUID being set, i.e. engagement, **not** sheath state.
///
/// **It carries that GUID**, because the reference's `[+0xc48]` is the target and not a flag, and
/// a second reader wants the unit and not just the fact: `0x6e3480`'s melee arm resolves
/// `[caster+0xc48]` and uses THAT unit's combat reach, which is what the spell tooltip's range
/// cell prints while you are auto-attacking. Every animation reader still asks only
/// `With`/`Has`, which is unchanged by the payload.
#[derive(Component)]
pub(crate) struct Engaged(pub(crate) u64);

/// The local player has fired an auto-repeat spell (Auto Shot / wand Shoot) — the client's
/// `[+0xd58] & 0x200`, whose **only writer binary-wide** is the local cast-send tail (`0x6e593b`,
/// gated `AttributesEx2 & 0x20`). While it and the
/// ranged **sheath** gate (`CUR == 2`) both hold, a standing unit's base idle is the ranged
/// weapon's Load clip ([`select::ranged_load_anim`]) — the client's resolver `0x5fd460`,
/// which gates on exactly this bit; local-player-only by construction (remote units never run
/// the local cast-send). A REMOTE shooter has no drawn idle at all: it plays LoadBow **once** at
/// the volley's single `SMSG_SPELL_START` through the PrecastKit ([`CastHold`], `0x6e7901`), then
/// the fire clip per GO over its ordinary idle. Cleared
/// only by [`cancel_auto_repeat_local`]'s callers — the client's cancel `0x6ea080` (its `0x200`
/// clear `0x6ea113`), reached from the cast-result fail of the cached spell, the button
/// re-press toggle, melee attack-start, target death, the wand-only new-cast handoff, and
/// `SMSG_CANCEL_AUTO_REPEAT` — which vmangos DOES send (corrected 2026-08-05; see
/// `crate::spell::net`'s `cancel_auto_repeat`), so against a live server that packet, not
/// a local death watcher, is what ends a volley whose target dies.
#[derive(Component)]
pub(crate) struct AutoRepeatArmed;

/// The weapon-visual hold — the client's `[+0xd58] & 0x400`, set for **ANY caster** (self,
/// remote players, NPCs) whose **ranged** spell visual plays (`0x60d020`, sole caller `0x6ec2cf`
/// inside `PlaySpellVisual`).
/// Cleared like the client's id-matched `0x60d040` + its unconditional sibling `0x60fc50`: a
/// **non-ranged** visual play by the same caster (the stale-visual cleanup `0x6ec39e`), the
/// local cancel (`0x6ea12b`), melee attack-start, and the spline-move apply (`0x6020e8`).
/// Deliberately NOT cleared on sheath change or volley end — the
/// client's bit persists latent.
///
/// **It gates nothing here — but no longer for the reason this doc used to give.** `0x400` is
/// tested in exactly one place image-wide: `0x5fc3f0`'s Hold self-loop gates (`test ah,0x6` for
/// HoldBow 109, `test ah,0x4` for 110/111/112). 0994 recorded that the dispatcher is never
/// reached for a bow id and concluded the bit is dead. **That absence proof is wrong**
/// (decision 1544): the dispatcher has a second, deferred fire site (`0x7075af`), so those gates
/// DO execute — they are the Hold's own per-completion re-arm.
///
/// We still gate nothing on it, and that is now a *modelling* choice with a stated equivalence
/// rather than a claim about the binary. Our Hold is armed once and loops until a real recompute
/// (`select::ranged_hold_anim`), where the reference re-arms it at each completion while the gate
/// holds. The two diverge only where `0x400` holds *without* `0x200` — a remote shooter's latent
/// hold — and a remote shooter never enters the drawn idle at all (`0x5fd460` claims on `0x200`,
/// which only the local cast-send sets), so the case cannot arise. Reading the bit as a second
/// *entry* into the drawn idle remains wrong: that is what left a shooter aiming forever after
/// one Serpent Sting (decision 0991).
#[derive(Component)]
pub(crate) struct RangedHold;

/// A unit's displayed **nocked ammo** — the client's `[+0xd28]`/`[+0xd2c]` ammo-model pair,
/// written only by `CGUnit::UpdateAmmoDisplay 0x60ba30`: the ammo's `ItemDisplayInfo` id, rendered
/// as a model on the unit's **body** at HandArrow (35) — the ONE bone attach, bow-only (`0x60bb19`;
/// `0x479f40`'s 0x18/0x19 pick a model directory, not Special2/Special3). Set per shot from the
/// `SMSG_SPELL_START` `CAST_FLAG_AMMO` tail (any caster — the only source for a remote/NPC
/// shooter). This is the display-id CACHE; whether the arrow is visibly in the hand is the
/// [`NockLatch`] (the `$BWP`/`$BWR` cycle). Removed by the un-nock `0x60f530` paths: the local
/// cancel, leaving the ranged sheath stance (`0x60fc72`), or a weapon change.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NockedAmmo {
    /// The ammo's `ItemDisplayInfo` id (never 0 — an id-0 refresh removes the component instead).
    pub(crate) display_id: u32,
}

/// The client's nock latch `[+0xd58] & 0x4000` (set `0x624b70`, cleared `0x60016c`): SET at
/// the body clip's **`$BWP`** BowPull keyframe (LoadBow 105 carries it at ~0.6 s — the visible
/// pull-from-quiver moment, which also (re)attaches the in-hand arrow), CLEARED at **`$BWR`**
/// BowRelease (AttackBow 46 carries it at ~0.067 s — the arrow leaves the hand as the missile
/// flies) and by the un-nock paths. The equipment attach leg shows the [`NockedAmmo`] model only
/// while this holds, and the bowstring's middle vertex tracks the draw hand on the same gate.
#[derive(Component)]
pub(crate) struct NockLatch;

/// Drive [`NockLatch`] from the anim-event stream ([`AnimSoundEvent`] — the same `$`-tag
/// keyframes the sound/impact consumers ride): `$BWP` latches (only a unit with a [`NockedAmmo`]
/// display cached — the client's handler gates on the cached ranged spell), `$BWR` clears.
/// Ordered after [`events::fire_anim_events`] so a tag lands the frame it's crossed.
pub(crate) fn drive_nock_latch(
    mut events: MessageReader<AnimSoundEvent>,
    nocked: Query<Has<NockedAmmo>>,
    mut commands: Commands,
) {
    for ev in events.read() {
        match &ev.ident {
            b"$BWP" if nocked.get(ev.entity) == Ok(true) => {
                // `try_insert`, matching the `$BWR` arm's guard below: the tag names a unit that
                // may be despawned by the time these commands apply (the fade lane is unordered
                // against this chain — B130's second window, in the same defect class).
                commands.entity(ev.entity).try_insert(NockLatch);
            }
            b"$BWR" => {
                if let Ok(mut e) = commands.get_entity(ev.entity) {
                    e.remove::<NockLatch>();
                }
            }
            _ => {}
        }
    }
}

/// The one local auto-repeat cancel — benilla's `0x6ea080`: clear the live key (`[0xceac30]`),
/// drop the Load/Hold idle gates
/// (the `0x200`/`0x400` bits — [`AutoRepeatArmed`] + [`RangedHold`]), un-nock the ammo
/// (`0x60f530` — [`NockedAmmo`]), and ack the server (`CMSG_CANCEL_AUTO_REPEAT_SPELL`, the
/// client's sole send site `0x6ea0c6`) — **sheath untouched**: the weapon stays drawn, the idle
/// falls back to the ordinary stand. Every cancel trigger funnels here, like the client's 8
/// live callers.
pub(crate) fn cancel_auto_repeat_local(
    entity: Option<Entity>,
    auto_repeat: &mut crate::spell::AutoRepeatActive,
    commands: &mut Commands,
    net: &crate::net::NetCommands,
) {
    if auto_repeat.0.is_none() {
        // The client's `[0xceac30] == 0` early-out — nothing running, nothing sent.
        return;
    }
    auto_repeat.0 = None;
    if let Some(e) = entity {
        commands
            .entity(e)
            .remove::<(AutoRepeatArmed, RangedHold, NockedAmmo, NockLatch)>();
    }
    let _ = net.0.send(crate::net::ClientCommand::CancelAutoRepeat);
}

/// The one local **StopAttack** — benilla's `0x5ecac0`: player-only, a no-op unless an attack
/// lock is held, then `CMSG_ATTACKSTOP` (its builder `0x624370` pushes opcode `0x142`) followed by
/// the tail jump `0x5ecb63 → CancelQueuedCast 0x6e6f30` — which takes the **queued on-next-swing
/// spell** down with the attack: `0x6e6f30`'s first leg tests the inflight id `[0xceca88]` for
/// `Attributes & 0x404` and hands it to `CancelCast 0x6e4940(dl=1, reason 0x1c)`, whose casting arm
/// sends `CMSG_CANCEL_CAST 0x12f` naming it and then pops the slot through `0x6e4ad0`.
///
/// This is the un-queue [`crate::spell::QueuedMeleeSpell`] names as its real clear path, and it
/// is why a Raptor Strike ring goes dark the moment Auto Shot starts (the auto-repeat arm inside
/// the cast commit calls straight into here, `0x6e5976`, guarded by nothing but "the caster is the
/// active player").
///
/// The lock is our server-echoed [`Engaged`] — the ref's `[+0xc48]`, which it sets *locally* at
/// attack-start and which **StopAttack does not clear** (only the `SMSG_ATTACKSTOP` echo does,
/// `0x624e40`), so the Attack button's own ring survives this call in both clients. Two ref fields
/// are deliberately unmodelled, neither with a benilla reader: `[+0xc50]` ("locally initiated, not
/// server-confirmed") and `[+0xc54]` ("stop sent, awaiting the echo") — the latter's sole reader
/// image-wide is `0x5eccda`, inside `Attack 0x5ecb70`, which no benilla cast path enters while
/// engaged (see [`crate::spell::cast_send`]'s tail). The two ratio legs `0x5ecac0` runs before
/// the stop are `TriggerTutorial(0xa)` / `(0xb)` — the low-health / low-mana popups, not audio and
/// not part of this seam.
pub(crate) fn stop_attack_local(
    engaged: bool,
    queued_melee: &mut crate::spell::QueuedMeleeSpell,
    net: &crate::net::NetCommands,
) {
    if !engaged {
        // The ref's `!IsAttacking(0x60ecb0) && [+0xc50] == 0` early-out: nothing running,
        // nothing sent — and the queued strike, which cannot exist without a swing to fire it,
        // survives untouched.
        return;
    }
    let _ = net.0.send(crate::net::ClientCommand::AttackStop);
    if let Some(spell_id) = queued_melee.current() {
        let _ = net
            .0
            .send(crate::net::ClientCommand::CancelCast { spell_id });
        queued_melee.clear_if(spell_id);
    }
}

/// The one local **StartAttack** — benilla's `0x5ecb70`, [`stop_attack_local`]'s twin. Two halves,
/// and keeping them apart is the whole point:
///
/// - **the send** (`0x5eccfd`) is gated by the already-attacking test at `0x5eccda` — skipped when
///   the attack lock is held, *unless* a stop is in flight (`[+0xc54]`), which is why
///   stop → select → re-swing re-points at a new target instead of going quiet;
/// - **the tail** (`0x5ecd78`–`0x5ecd95`) — melee sheath SNAP then the local auto-repeat cancel
///   `0x6ea080` — runs either way, because the skip's `jmp` lands *inside* it. That is what makes
///   "start swinging" and "keep auto-shooting" mutually exclusive.
///
/// `stop_in_flight` is a **parameter, not stored state**: the ref's `[+0xc54]` is set by
/// [`stop_attack_local`] and consumed here, and the only caller that spans both is the target
/// switch, which does them back to back in one call chain. Decision 1036 removed the persistent
/// latch for exactly this reason — outside that chain it has no reader.
///
/// The caller owes the reference's own two entry conditions, which differ per site and are not
/// re-derived here: a cast press must not call this at all while engaged (`TryCast`'s `6e51cb`/
/// `6e51d2` gate), and the Attack button forks to [`stop_attack_local`] instead (`0x6131a0` @
/// `0x6131d9`). `0x5ecb70`'s own validator legs — the target's alive-or-feign + `CanAttack
/// 0x606980` walk, and the `[0xb4b3e4]` world gate — stay with the callers that already compute
/// them (`target::scan`'s `new_attackable`, the drain's `attack_actor_refusal`).
pub(crate) fn start_attack_local(
    entity: Entity,
    target: u64,
    engaged: bool,
    stop_in_flight: bool,
    auto_repeat: &mut crate::spell::AutoRepeatActive,
    sheath: &mut MessageWriter<SheathRequest>,
    commands: &mut Commands,
    net: &crate::net::NetCommands,
) {
    // **Starting an attack stands you up** — `0x5ecc9b call 0x5ed430(0)`, inside this function and
    // ahead of everything else it does (decision 1768).
    //
    // Its position is the content. It sits **after** the attackability cluster
    // (`0x5ecc2e`/`0x5ecc35`, whose not-attackable leg diverts to `0x5ecc37` and never reaches the
    // stand) — which is why a right-click on a friendly NPC does not stand you — and **before any
    // range logic**, `CMSG_ATTACKSWING` being all the way down at `0x5eccfd`. So an attackable
    // target out of reach *does* stand you, and so does re-issuing an attack you are already
    // making: that skip lands at `0x5ecd78`, 73 bytes past the stand. Hence unconditional here,
    // above the swing's own `engaged` fork, rather than inside it.
    //
    // benilla's callers own the attackability walk (see this function's header), so by the time we
    // are called the diverting leg has already been taken elsewhere.
    commands.write_message(crate::player::StandStateRequest { state: 0 });
    if !engaged || stop_in_flight {
        let _ = net
            .0
            .send(crate::net::ClientCommand::AttackSwing { guid: target });
    }
    // The tail. The sheath setter is idempotent — the client's own "no-op if already melee" — so
    // re-running it on a re-point costs nothing and matches `0x5ecd80`'s unconditional call.
    sheath.write(SheathRequest {
        entity,
        state: 1,
        ceremony: false,
    });
    cancel_auto_repeat_local(Some(entity), auto_repeat, commands, net);
}

/// The **Attack toggle** — benilla's `0x6131a0`, the third of the trio and the one the base Attack
/// pseudo-spell runs (`TryCast`'s effect-`0x4e` short-circuit at `0x6e4c7a` dispatches straight to
/// it, ahead of every cast gate). It forks on the attack lock and nothing else: already attacking →
/// `0x6131d9 call 0x5ecac0` [`stop_attack_local`], otherwise `0x6131ee call 0x5ecb70`
/// [`start_attack_local`].
///
/// The fork is the whole content, and it is worth a name because both halves used to be wrong here:
/// the button never toggled melee OFF, and it cancelled a running auto-repeat on *every* press —
/// but only the start arm reaches `0x5ecd8c`, so in the reference toggling melee off leaves Auto
/// Shot running.
///
/// `[0xb4b3e4]` — the reference's second condition on the stop arm, a world/session global also
/// tested at `0x5ecbc0` — is unmodelled; nothing in benilla can be false there while a press is
/// being drained.
pub(crate) fn toggle_attack_local(
    entity: Entity,
    target: u64,
    engaged: bool,
    queued_melee: &mut crate::spell::QueuedMeleeSpell,
    auto_repeat: &mut crate::spell::AutoRepeatActive,
    sheath: &mut MessageWriter<SheathRequest>,
    commands: &mut Commands,
    net: &crate::net::NetCommands,
) {
    if engaged {
        stop_attack_local(engaged, queued_melee, net);
    } else {
        start_attack_local(
            entity,
            target,
            engaged,
            false,
            auto_repeat,
            sheath,
            commands,
            net,
        );
    }
}

/// **The local attack seams' whole write set, as ONE [`SystemParam`]** — so a caller threading
/// [`stop_attack_local`]/[`start_attack_local`] through a call chain takes one param instead of
/// six, and so a new sink in either seam lands in every caller at once.
///
/// The targeting side needs exactly this: `target::scan`'s `commit` runs the reference's
/// stop → select → re-swing law (`SetSelection 0x493540`'s own `0x493a08 call 0x5ecac0` and
/// `0x4938c8 call 0x5ecb70`), so every selection writer that can fire it has to carry both seams'
/// inputs. [`crate::spell::cast_send::CastLadder`] already carries them field by field for the
/// cast path and calls the free functions directly.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct AttackSeam<'w, 's> {
    pub(crate) net: Res<'w, crate::net::NetCommands>,
    pub(crate) queued_melee: ResMut<'w, crate::spell::QueuedMeleeSpell>,
    pub(crate) auto_repeat: ResMut<'w, crate::spell::AutoRepeatActive>,
    pub(crate) sheath: MessageWriter<'w, SheathRequest>,
    pub(crate) ecs: Commands<'w, 's>,
    /// Our own entity — the sheath snap's and the auto-repeat cancel's subject.
    pub(crate) me: Query<'w, 's, Entity, With<crate::net::SelfPlayer>>,
}

impl AttackSeam<'_, '_> {
    /// `0x5ecac0` — see [`stop_attack_local`].
    pub(crate) fn stop(&mut self, engaged: bool) {
        stop_attack_local(engaged, &mut self.queued_melee, &self.net);
    }

    /// `0x5ecb70` — see [`start_attack_local`]. A no-op before our own entity streams in.
    pub(crate) fn start(&mut self, target: u64, engaged: bool, stop_in_flight: bool) {
        let Ok(e) = self.me.single() else { return };
        start_attack_local(
            e,
            target,
            engaged,
            stop_in_flight,
            &mut self.auto_repeat,
            &mut self.sheath,
            &mut self.ecs,
            &self.net,
        );
    }
}

/// A unit is mid-cast (`SMSG_SPELL_START` .. `GO`/`FAILED_OTHER`, decision 0099 phase 1) — the
/// wire-truth state seam. Inserted on `SpellStart` only when it carries a nonzero cast time (an
/// instant cast's `SpellGo` follows with nothing to interrupt, so it never gets one); removed on
/// the matching-spell-id `SpellGo`/`SpellFailedOther` (the client's reap `0x614150(spellId, 0)` is
/// keyed — a triggered proc's GO mid-cast never clears a different spell's precast, decision 0107).
/// The *animation* side rides [`CastEvent`]/[`CastHold`] (the resolved layer,
/// [`spell_visual::route_cast_visuals`]), not this component. `until` is `Option` rather than a
/// bare deadline so a later channel (an open-ended cast, no fixed end) can land here without a
/// sentinel.
#[derive(Component, Clone, Copy)]
pub(crate) struct Casting {
    pub(crate) spell_id: u32,
    #[allow(dead_code)]
    pub(crate) until: Option<Instant>,
}

/// The client's PlayAnimation **call order**, reconstructed. The real client has no pecking
/// order between a swing and a spell kit's anim landing in the same frame — its handlers run
/// synchronously in packet order and the LATER `PlayAnimation` call simply overwrites bone 0
/// (`0x5fe2f0`). benilla's swing and cast pipelines are
/// separate message streams, so that order must ride the messages: every emitter of an
/// animation-bearing request ([`SwingMessage`], [`CastEvent`], [`EmoteAnim`], [`KitPush`])
/// stamps `next()` at emission — the wire drain stamps in packet order; scene-time emitters
/// (a missile arriving, an interact emote) stamp when they fire, which is after that frame's
/// packets, exactly where the client's scene-update calls sit. [`driver::drive_animations`]
/// resolves a same-frame collision by the highest stamp.
#[derive(Resource, Default)]
pub(crate) struct PlaySeq(u64);

impl PlaySeq {
    pub(crate) fn next(&mut self) -> u64 {
        self.0 += 1;
        self.0
    }
}

/// A cast lifecycle edge on a streamed unit, straight off the wire (written by the net bridge,
/// consumed by [`spell_visual::route_cast_visuals`], which resolves spell → `SpellVisual` → kit →
/// animation/sound — the DBC knowledge stays in the anim layer, decision 0107).
#[derive(Message, Clone, Copy)]
pub(crate) struct CastEvent {
    pub(crate) entity: Entity,
    pub(crate) spell_id: u32,
    pub(crate) kind: CastEventKind,
    /// [`PlaySeq`] stamp at emission — carried through to the kit's [`EmoteAnim`].
    pub(crate) seq: u64,
}

/// Which cast edge fired (decision 0107 verdict 2's stage wiring).
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum CastEventKind {
    /// `SMSG_SPELL_START` — the precast kit persists from here (subliminal for an instant, whose
    /// GO follows at once).
    Start,
    /// `SMSG_SPELL_GO` — reaps the precast, plays the cast/release kit.
    Go,
    /// The cast died without a release (`SMSG_SPELL_FAILED_OTHER`, or our own failed
    /// `SMSG_CAST_RESULT`) — reap only, nothing plays.
    Fail,
    /// The spell **landed on this unit** (`entity` = the struck target, not the caster — decision
    /// 0099 phase 4): a missile arrival, written back by `crate::entities::missile`. Plays the
    /// impact kit (stage 1) + the state kit (stage 2) on the target — the client's unit-impact
    /// hand-off `0x61dc50`. A speed-0 spell's impacts never come through here: the router plays
    /// them inline at the GO (same resolve, no round-trip). `weapon_visual` = the caster's
    /// ranged-weapon substitute `SpellVisual` id, resolved at GO time and carried through the
    /// flight (the caster may despawn mid-flight; the client's missile carries its kit context)
    /// — the `0x60d450` fallback a basic shot's impact kit resolves through (decision 0370).
    Impact { weapon_visual: Option<u32> },
    /// A projectile arrived at a **ground point** instead of on a unit — the client's per-tick
    /// missile dispatch taking `0x61e1d0` → `0x61d870`, which is the **no-live-target** arm, not
    /// a ground arm as such (a target that despawned mid-flight lands there too). Ours is reached
    /// only by the single
    /// missile a dest-targeted GO with an empty hit list launches
    /// ([`spell_visual::MissileSpawn::ground_aim`]). `entity` is the **caster** — `0x61d870`
    /// plays the kit on it, not on anything at the point — and `pos` is the arrival position,
    /// the client's `extra` override (`0x61d8e7`: `lea edx,[esi+0x20]`). Plays `SpellVisual`
    /// **field 13 at stage 3**.
    GroundImpact { pos: Vec3 },
}

/// The resolved casting **hold** — the animation a unit sustains while a cast or channel is in
/// flight (`SpellVisualKit` field 2 of the precast/channel kit, decision 0107). Managed entirely
/// by [`spell_visual::route_cast_visuals`]; the driver renders it per frame: **standing → the
/// gait slot, full-body** (the client's stationary-cast pin — the settled `[CGUnit+0xb4]` gate) —
/// **moving → a looping masked overlay** (the ordinary torso-masked route a moving caster falls
/// through to). A Special (jump/pose) outranks it — the server interrupts such casts anyway.
/// `spell_id` keys the reap, like everything in this pipeline (the client's `0x614150(spellId, …)`):
/// an unrelated spell's GO/fail never drops another cast's hold.
#[derive(Component, Clone, Copy)]
pub(crate) struct CastHold {
    pub(crate) anim_id: u16,
    pub(crate) spell_id: u32,
    /// A ranged-slot shot's wind-up (`Attributes & 0x2` — Throw's ReadyThrown, the bow's
    /// LoadBow): the client brackets every ranged kit play with ranged snaps (`0x60f34c`
    /// before, the outer `0x6e5930`/`0x6e78f3` after), so while this hold is live the sheath
    /// reconcile's force-stow never survives the frame ([`driver`]'s bracket override).
    pub(crate) ranged: bool,
}

/// Per-hand weapon-grip state: whether a weapon is held in the right/left hand, so that hand's fingers
/// curl into a grip. The real client arms `HandsClosed` (AnimationData 15) on a hand's finger key-bones
/// **purely by that hand's attach point being occupied by a weapon** — not combat, not sheath state; a
/// forearm-mounted shield or an empty hand stays open (`0x60b590` /
/// paperdoll `0x5059a0`). Written by the held-item resolver ([`crate::entities`], decision 0072) from the
/// weapon's hand attach point; read by [`driver::drive_hand_grip`] to hold/release the finger overlay.
#[derive(Component, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HandGrip {
    /// A weapon occupies the right-hand attach point (mainhand, id 1).
    pub(crate) right: bool,
    /// A weapon occupies the left-hand attach point (a non-shield offhand, id 2).
    pub(crate) left: bool,
}

/// The weight of the per-hand weapon-grip finger overlay ([`benilla_assets::ModelAnimations::hand_close`])
/// over the base gait on the finger key-bones both drive. The base clip animates the whole skeleton
/// (fingers at Stand's flat 0°) and isn't masked out of the finger subtree, so grip and base blend there —
/// this makes the grip dominate ≈ 8:1 (the fingers read closed, not a half-open wash). Same rationale/value
/// as the sheath ceremony and the upper-body one-shot overlays. Shared by the live driver
/// ([`driver::drive_hand_grip`]) and the glue-booth spawn ([`crate::portrait`]) so both blend the grip
/// identically.
pub(crate) const HAND_GRIP_WEIGHT: f32 = 8.0;

/// One completed melee swing for `attacker` (`SMSG_ATTACKERSTATEUPDATE` → the net bridge; decision
/// 0073: one packet = one swing animation, no client-side timer). `hit_info` bit `0x4` = offhand
/// swing; bit `0x10000` suppresses the animation. The outcome fields (`victim`, `victim_state`,
/// `damage`) ride along for the combat-audio consumer (`sound::combat`), which plays the swing's
/// sounds when the animation's `$CSS`/`$CAH`/`$HIT` events fire. A fully-absorbed swing's
/// "Absorb" word keys `hit_info & 0x20`, not a magnitude (`0x6243e0` never reads one — decision
/// 0279), so no absorb field rides here; the full-block synthesis likewise rewrites
/// `victim_state` in the net bridge before this message is built.
#[derive(Message, Clone, Copy)]
pub(crate) struct SwingMessage {
    pub(crate) attacker: Entity,
    /// The swing's target, when it's streamed to us.
    pub(crate) victim: Option<Entity>,
    pub(crate) hit_info: u32,
    /// vmangos `VictimState`: 1 hit · 2 dodge · 3 parry · 4 interrupt · 5 blocks · 6 evades ·
    /// 7 immune · 8 deflects (decision 0279's byte-verified consequence table keys off it).
    pub(crate) victim_state: u32,
    pub(crate) damage: u32,
    /// `0x625e40`'s verdict, carried from the packet because **the floating number is gated by it
    /// too** ([`benilla_protocol::messages::AttackerState::displayed`]): `0x62440d` is the first
    /// thing the worldtext builder `0x6243e0` does. Everything else this message drives — the
    /// animation, the flinch, the blood, the sounds, the timers — runs regardless, which is why
    /// the verdict rides along instead of suppressing the message.
    pub(crate) displayed: bool,
    /// [`PlaySeq`] stamp at emission (the wire drain, in packet order).
    pub(crate) seq: u64,
}

/// Play a one-shot **anim-emote** on a unit: the given `AnimationData.dbc` id, played over the
/// gait and returning to it when it finishes (the [`select::Mode::Swing`] one-shot pattern). The
/// gossip/vendor interact route ([`crate::target`], decision 0081) uses it to play EmoteTalk
/// (id 60) on the self-player: the per-animation sheath reconcile then reads that clip's
/// WeaponFlags (`0x10`) and stows the drawn weapon — a committed state change that *persists*
/// after the emote (nothing restores it; weapons stay stowed until re-triggered — decision 0080).
/// The stow is the emote's flags, not a sheath wire. Also fed by the `SMSG_EMOTE` receive path
/// ([`emote_anim::emote_to_anim`]), which maps a bridged anim emote's `Emotes.dbc` id to its
/// `AnimID` — `/wave`, `/bow`, `/laugh`, `/cheer`, … all play through this same one-shot.
#[derive(Message, Clone, Copy)]
pub(crate) struct EmoteAnim {
    pub(crate) entity: Entity,
    pub(crate) anim_id: u16,
    /// [`PlaySeq`] stamp — a kit anim inherits its [`CastEvent`]'s; scene-time emitters stamp
    /// fresh at fire time. The driver's same-frame swing collision resolves by the higher stamp.
    pub(crate) seq: u64,
}

/// A **state kit's animation id, which is a comparison and never a play** — `PlaySpellVisualKit`'s
/// stage-2 leg (decision 2085).
///
/// `0x60edf0`'s tail has exactly one site that hands a kit's `+0x8` to the play primitive
/// (`0x60f3c5 call 0x5fe2f0`), and **stage 2 is diverted around it**: `0x60f387 jne` takes the
/// stage-2 leg, which compares the unit's currently-armed id (`0x5fdb50` — upper-body key bone
/// preferred, bone 0 as the fallback) against `kit.AnimID` and either does nothing (`je 0x60f3ca`,
/// already playing it) or runs `0x5fd9e0(unit, -1)` — a **base-animation recompute** — before
/// leaving the block for good. Both `SpellVisual` field-4 consumers hardcode stage 2 (the aura
/// watcher `0x5ff4c6` and the impact hand-off `0x61dced`), so no path in the image ever plays a
/// state kit's anim.
///
/// It is therefore an animation-**cutting** mechanism, and the cut is not incidental: Charge
/// (22911) plays `Knockdown`(121) from its impact kit and then cuts it with its own state kit's
/// `Stun`(14), because 121 ≠ 14 forces the recompute. That is also why no stun ever shows the
/// `Stun` pose despite 23 kits naming it — the selector `0x5fd8b0` cannot produce 14 (14 is not
/// among its literal outputs), so the recompute lands on `Stand`.
///
/// The driver holds the comparison because the armed id lives there. An aura **refresh** in place
/// emits nothing: the watcher `0x604d00` fires neither arm when a slot is rewritten with the same
/// id and a non-zero flags nibble, which our per-unit armed-id diff reproduces by construction.
#[derive(Message, Clone, Copy)]
pub(crate) struct BaseAnimRecompute {
    pub(crate) entity: Entity,
    /// The state kit's `AnimID` — the right-hand side of `0x60f390`'s compare, never played.
    pub(crate) anim_id: u16,
}

/// Rear up a rider's mount (decision 0441 P2): resolved to a [`EmoteAnim`] one-shot of
/// MountSpecial(94) on the unit's MOUNT CHILD entity by [`flourish_to_anim`] — `0x608d50`'s
/// routing (the flourish plays on the mount model; the rider holds Mount(91) throughout).
/// Written by the net drain (`SMSG_MOUNTSPECIAL_ANIM` — observed riders only; our own echo
/// is dropped there) and by the self space-bar gate (`crate::player`, locally at send time).
/// A unit with no mount child (a race with dismount) drops silently — nothing to rear.
#[derive(Message, Clone, Copy)]
pub(crate) struct MountFlourish {
    pub(crate) unit: Entity,
}

/// [`MountFlourish`] → [`EmoteAnim`]: hop unit → its mount child and fire the one-shot there.
/// The mount child runs the untouched creature machinery (it is not itself "mounted"), so the
/// general one-shot player takes it from here — full-body 94 over the mount's gait, returning
/// to it when done.
fn flourish_to_anim(
    mut msgs: MessageReader<MountFlourish>,
    mut out: MessageWriter<EmoteAnim>,
    mut play_seq: ResMut<PlaySeq>,
    mounts: Query<&crate::entities::mount::MountChild>,
) {
    for m in msgs.read() {
        let Ok(child) = mounts.get(m.unit) else {
            debug!("flourish: {:?} has no mount child — dropped", m.unit);
            continue; // dismounted before the flourish landed — nothing to rear
        };
        debug!(
            "flourish: MountSpecial(94) one-shot on {:?} (mount of {:?})",
            child.0, m.unit
        );
        out.write(EmoteAnim {
            entity: child.0,
            anim_id: select::MOUNT_SPECIAL,
            seq: play_seq.next(),
        });
    }
}

/// A **spell-side wound flinch** on `entity` — the client's `0x60ea70(unit, severity = 0)`
/// reached from three spell paths (decision 2058): the kit player's own branch (`0x60edf0` @
/// `0x60f3ad`: a kit anim in `[8,10]` goes here instead of `PlayAnimation`), the instant-hit
/// impact loop (`0x6e8bf0` @ `0x6e8c89` — after the impact kit, iff the spell targets enemies,
/// [`benilla_formats::SpellDisplay::is_harmful`]), and the missile impact hand-off (`0x61dc50` @
/// `0x61dc74` — before the impact kit, every living target). All three pass **severity 0**, so
/// the id is never the kit's own column: the trigger picks CombatWound(9) / StandWound(8) by the
/// victim's engagement exactly like a non-crit melee hit ([`select::wound_anim`]) and lays it
/// into the SECONDARY-blend slot — a decaying overlay that never interrupts what plays
/// underneath (decision 0111). Never the [`EmoteAnim`] one-shot route: that replaces the base
/// track, the exact routing 0111 falsified. Written by [`spell_visual::route_cast_visuals`];
/// consumed by [`driver::drive_animations`] into the same per-frame wound slot as a melee hit.
#[derive(Message, Clone, Copy)]
pub(crate) struct WoundAnim {
    pub(crate) entity: Entity,
}

/// The target lists off one `SMSG_SPELL_GO` (decision 0099 phase 4) — the payload [`CastEvent`]'s
/// `Copy` shape can't carry. Written by the net bridge alongside the GO's [`CastEvent`]; consumed
/// by [`spell_visual::route_cast_visuals`], which resolves `Spell.dbc` Speed and either plays the
/// impacts inline (speed 0) or launches missiles ([`spell_visual::MissileSpawn`]) — the client's
/// two GO branches (`0x6e8bf0` instant vs `0x6e8a50 → 0x60a3d0` projectile).
#[derive(Message, Clone)]
pub(crate) struct SpellGoTargets {
    pub(crate) caster: Entity,
    pub(crate) spell_id: u32,
    /// Units the spell landed on — each gets the impact hand-off (inline or on missile arrival).
    pub(crate) hits: Vec<Entity>,
    /// Units it missed, with the wire's `SpellMissInfo` code — a projectile still flies at
    /// each, and its **arrival** plays the victim's defense clip for DODGE(3)/BLOCK(5) (the
    /// client's `Missile_C::Update 0x61ceb0` dispatch — Dodge 30 / ShieldBlock 24, never Parry).
    /// No impact kit plays. The deflect
    /// *flight* visual (the projectile glancing off) stays a named approximation — ours ends
    /// the flight at the target.
    pub(crate) misses: Vec<(Entity, u8)>,
    /// The ground point a dest-targeted cast launched at (`TARGET_FLAG_DEST_LOCATION`), in
    /// **bevy** coords (converted at the apply seam like every scene position). A ground AOE's
    /// GO arrives with empty hit/miss lists and only this. No consumer reads it yet — the
    /// dest-anchored launch visual is still an open question (the persistent area
    /// effect anchors to the DynamicObject create, not to this packet).
    pub(crate) dest: Option<Vec3>,
    /// The GO's ammo block (`castFlags & 0x20`, a ranged shot): the `ItemDisplayInfo` id of the
    /// caster's ammo/thrown — the missile's model when the spell's visual chain has none (the
    /// client's `0x479f40` fallback, decision 0099 phase 5).
    pub(crate) ammo_display_id: Option<u32>,
    /// [`PlaySeq`] stamp — same packet as the GO's [`CastEvent`], same stamp: the inline
    /// impacts play from the one handler call.
    pub(crate) seq: u64,
}

/// The sheath **policy layer**'s types + ceremony mechanics (decision 0080) — the one-setter
/// request, the ceremony overlays, the `AnimationData.dbc` policy table. The driver below
/// executes them, so every sheath transition has exactly one author.
mod sheath;
use sheath::{load_anim_data, SheathSwap};
pub(crate) use sheath::{
    toggle_sheath_next, AnimData, SheathRequest, SheathSwapMessage, VisualSheath,
};

/// The `SMSG_EMOTE` → [`EmoteAnim`] consumer (Part 1 of the emote-animation wiring) — kept
/// alongside `sheath` as its own small concern.
mod emote_anim;

/// The client-local gesture producers (decision 1469): the chat talk/question/exclamation/shout/
/// laugh, and the NPC-interact talk. The reference's `0x60bb30`, which shares the one-shot player
/// with `SMSG_EMOTE`.
mod gesture;
use emote_anim::emote_to_anim;
pub(crate) use gesture::{select_gesture, Gesture, GestureQueue};

/// The Bevy driver systems that execute the state machine [`select`] picks (decision 0049 + 0073,
/// decision 0087's one-shot routing, decision 0080's sheath execution) — kept in its own file as
/// it carries the bulk of the per-frame system logic, separate from this module face and from the
/// pure selector logic in [`select`].
mod driver;
pub(crate) use driver::oneshot_is_live;
use driver::{drive_animations, drive_hand_grip};

/// The model event-keyframe scanner (decision 0070 slice 3) — kept in its own file as its own
/// small concern, separate from the driver ([`driver`]) that advances the clips it scans.
mod events;
use events::fire_anim_events;
pub(crate) use events::{
    advance_track, footfall_culls, footfall_side, is_footstep_sound, scan_events, AnimSoundEvent,
    EventFrame, TrackMemory,
};

/// The `$BTH` breath puffs — a unit's visible cold vapour in a snow zone (B233, decision 1149).
mod breath;
use breath::{classify_breath, fire_breath};

mod impact;
use impact::route_swing_impacts;
pub(crate) use impact::{DefenseAnim, PendingImpacts, SwingFlush, SwingImpact, SwingSlowdown};

/// The spell-visual data plane + the cast-edge router (decision 0099 phase 2 / 0107):
/// `SpellVisual.dbc` × `SpellVisualKit.dbc` loaded alongside [`AnimData`], and
/// [`spell_visual::route_cast_visuals`] resolving wire [`CastEvent`]s into [`CastHold`]s, release
/// one-shots ([`EmoteAnim`]) and kit sounds ([`spell_visual::SpellKitSound`]).
mod blood;
mod env_damage;
pub(crate) mod net;
pub(crate) mod spell_visual;
use blood::{blood_spurts, load_blood_tables};
use env_damage::{hard_landing_dust, load_env_damage_table};
pub(crate) use env_damage::{EnvDamageTable, HardLanding, HARD_LANDING_DESCENT};
use spell_visual::{
    arm_aura_state_fx, arm_level_up_fx, arm_loot_fx, arm_morph_latch, arm_mount_poof_fx,
    load_spell_visuals, replay_morph_kit, route_cast_visuals, MorphLatch,
};
// The aura-slot watcher is the shared trigger for BOTH halves of a state kit, so the CharProc half's
// own tests (`crate::aura_visual`) drive it directly rather than re-deriving the slot diff.
#[cfg(test)]
pub(crate) use spell_visual::arm_aura_state_fx as arm_aura_state_fx_for_test;
pub(crate) use spell_visual::{
    held_strike_sound, ChainProcPlay, FxClass, FxStage, KitPush, MissileSpawn, SpellKitFx,
    SpellKitShake, SpellKitSound, SpellVisuals,
};

/// The per-unit animation state machine.
///
/// `Transform` is required, not optional: the driver reads the entity's render scale as the
/// locomotion rate divisor (decision 0903), so a driver on a transform-less entity would silently
/// stop being driven at all. Every real unit is spawned with one — the requirement is here so a
/// hand-assembled entity (a test world, a future harness) can't quietly miss it.
#[derive(Component)]
#[require(Transform)]
pub(crate) struct AnimDriver {
    mode: Mode,
    /// The gait id currently targeted while in [`Mode::Gait`] (so we cross-fade only on a change). `None`
    /// forces a fresh selection (first eval, or after a Special state exits).
    gait: Option<u16>,
    /// The movement flags the **base slot** was last armed for — the reference has no per-one-shot
    /// latch, so "the movement state changed" means *since the base last took a request*, never
    /// "since this one-shot started" (decision 0894). The distinction is invisible until a one-shot
    /// and a movement change land on the SAME frame: Ice Block's root wipes the direction bits in
    /// the very frame the cast one-shot arrives, so an arm-time comparison sees no edge at all and
    /// the cast keeps bone 0 — where the reference's next base request overwrites it and leaves the
    /// character neutral.
    gait_flags: u32,
    /// The **base-animation lock** ([`driver::play::BaseAnimLock`]) — the reference's
    /// `[unit+0xd58] & 0xc0000`. While a `Knockdown`/`LiftOff`/`Land` holds it, every base request
    /// is refused outright, which is how a stunned victim's knockdown survives the root's own
    /// `Stand` recompute (decision 2096).
    base_lock: driver::play::BaseAnimLock,
    /// The unit's **client-side sheath state** — the mirror of the client's committed CUR cache
    /// (`[unit+0xd40]`, decision 0080): what the weapon placement renders (absent a
    /// [`VisualSheath`] ceremony pin) and what the setter/reconcile test against. Seeded from
    /// the descriptor byte at first sight; re-adopted whenever the server byte *changes* (the
    /// `0x604c70` field-apply); overwritten by requests ([`SheathRequest`]) and the
    /// per-animation reconcile. `None` = not yet seen (arms silently).
    sheath_cur: Option<u8>,
    /// The `UNIT_FIELD_BYTES_2` sheath byte last seen, to detect inbound field *changes* (our
    /// own `CMSG_SETSHEATHED` echo arrives already-committed and adopts as a no-op).
    sheath_byte: Option<u8>,
    /// The pending mid-animation weapon swap, while a draw/stow one-shot is in flight.
    sheath_swap: Option<SheathSwap>,
    /// **Did this unit start an animation THIS frame** — of any kind: a one-shot, a masked
    /// overlay, a cast/channel hold, or the mode machine's own base/gait play. Rewritten every
    /// pass by [`driver::drive_animations`], so a reader one system later sees exactly the
    /// frame's edge.
    ///
    /// Its one consumer is the weapon-trail latch ([`crate::weapon_trail`], decision 2076). The
    /// reference consumes `unit+0xd1c`/`+0xd20` inside `CGUnit::PlayAnimation 0x5fe2f0` itself
    /// (`0x5fe48e`, the fields' only reader), and `0x5fe2f0` is the **single** animation entry
    /// point in the image — 40 call sites, locomotion among them (`0x602c60` → `0x5fd9e0` →
    /// `0x5fd8b0` → `0x5fd100`), which is why this is not a one-shot flag: a unit that simply
    /// starts running consumes the arm, and that is what starts Charge's trail.
    /// benilla's driver is one batched system, not
    /// a per-play function, so the arm cannot be read at the call the way the reference reads it.
    started_anim: bool,
    /// A **masked upper-body one-shot** in flight (decision 0087): a swing/emote the live-state route
    /// sent to the SpineLow overlay (moving / seated / airborne-in-combat), playing *beside* [`Mode`]
    /// while the base track keeps driving the legs (run / sit / jump-arc). `None` = no overlay; a
    /// finished overlay **fades** the subtree back to the base ([`Self::overlay_fade`]) rather than
    /// dropping it. The full-body route never uses this — it replaces the base via [`Mode::Swing`]
    /// — until a locomotion request **transplants** that clip up here (decision 0878).
    overlay: Option<Overlay>,
    /// The key-bone slot's **cross-fade in flight** (decision 0878) — the client's per-bone
    /// SECONDARY as an upper-body arm or release seeds it. Two producers, one curve
    /// ([`select::blend_lambda`], λ decaying 1 → 0): the **fade-to-rest** that releases a finished
    /// one-shot over a fixed 150 ms (`0x5fc920` → op4 `param_3 = −1`, `0x7123af`), and a **blended
    /// re-arm** cross-fading a new masked clip in over the incoming clip's own blendTime
    /// (`0x7125f2`). A **transplant** arm carries `blendFlag = 0` and seeds nothing — it resumes
    /// the clip at its live frame with no fade at all.
    overlay_fade: Option<OverlayFade>,
    /// The victim **wound-flinch** in flight (decision 0111) — the client's per-bone **SECONDARY
    /// blend slot**: a decaying cross-fade overlay (`λ` smoothstep `0.75 → 0` over the wound
    /// clip's own span) layered over whatever else plays, then self-releasing. Deliberately a
    /// separate slot from [`Self::overlay`] — in the client they are two independent per-bone
    /// slots (primary arm vs secondary blend), so a masked swing and a wound decay **coexist**;
    /// folding the wound into the one-shot slot is exactly the falsified 0107 routing.
    wound: Option<Wound>,
    /// Whether the current airborne arc launched **upward** — a jump (the JumpStart/Jump bracket)
    /// vs a step-off fall (the gait freeze). Decided from `vertical_speed` on the arc's first
    /// airborne frame — the MSG_MOVE_JUMP-vs-StartFalling distinction the real client carries as
    /// an event, read off the launch velocity in our flag-collapsed machine. Stale on the ground.
    jump_arc: bool,
    /// Vertical speed as of last frame — the **launch** detector behind [`Self::jump_arc`]. A jump
    /// out of a one-frame micro-detachment never toggles the FALLING bit (land and relaunch inside
    /// one frame), so the bit's edge cannot be the only place the arc is classified; a rise past
    /// `JUMP_ARC_MIN_UP` from below is the launch itself, and nothing but a jump produces one.
    last_vertical_speed: f32,
    /// FALLING was set last frame — marks the arc edges: takeoff (decide [`Self::jump_arc`]) and
    /// the landing of a bracket-less step-off fall (which must still run the `0x602c60` land pick
    /// even though no Special drove the arc).
    was_falling: bool,
    /// The **deferred combat one-shot** — the client's `CGUnit+0xd60` cache (decision 0406):
    /// a combat clip requested while another combat
    /// clip plays is NOT armed — the playing clip's rate doubles (op6 2.0f) and the request
    /// parks here, played by the driver the moment no one-shot is live (the client's
    /// base-recompute read). Any normal arm clears it (the client's `0x5fe48e` writes −1 on
    /// every non-fast-path PlayAnimation; the driver clears at its play sites).
    deferred: Option<u16>,
    /// The armed looping arm's **replay window** (decision 0516): the variation node the loop
    /// armed + its rolled budget `R` (the client's
    /// `block+0xbc`; the window is `R` clip-lengths wide, op4 `0x7126d8` — live for LOOPS too,
    /// correcting 0117's "loops ignore it"). The per-frame watchdog (`0x719370`'s transcription
    /// in `drive_animations`) re-arms the id when the node — still the MAIN armed animation —
    /// completes `R` passes: the fresh weighted pick that alternates a gryphon's flap/glide and
    /// walks a multi-part dance. `None` = the main arm is a one-shot (its budget is a `Count`
    /// repeat) or a deliberate freeze (ranged Load / Loot).
    loop_window: Option<(bevy::animation::graph::AnimationNodeIndex, u32)>,
    /// The playback rate last written to the base slot — `speed / (moveSpeed · |modelScale|)`
    /// ([`select::playback_rate`], decision 0903); `1.0` for anything that is not a rate-scaled
    /// locomotion clip. Purely an **instrument**: the hover inspector's anim line reads it, so
    /// "this creature's walk looks too fast" is answerable by hovering the creature instead of by
    /// working the divisor out by hand. Recorded rather than re-derived, so the card can never
    /// disagree with what is actually playing.
    gait_rate: f32,
    /// `UNIT_FIELD_MOUNTDISPLAYID` as of the last pass — the driver's **mount-transition edge**
    /// detector, the local twin of the client's own UpdateField change-watcher (`0x604329`:
    /// TYPEID Unit, descriptor offset `0x1fc`, width 4, thunk `0x604570` → `0x5ffa50`). Any
    /// change is an edge — 0→N (mount), N→N′ (swap) and N→0 (dismount) alike — because the
    /// reference's handler is one watcher whose two legs BOTH arm bone 0 of the body: the build
    /// `0x607b44` op4(bone 0, **91 `Mount`**, cross-fade, PRIMARY), the teardown `0x607ce0` op4
    /// seq **0 `Stand`**. The arm is a plain last-writer-wins play, so it *displaces a full-body
    /// one-shot the transition catches in flight* — which our gait-slot mount pin alone never did
    /// (decision 0927).
    mount_display: u32,
    /// The Special wanted LAST frame — the driver's Special **edge** detector (decision 0864).
    /// An edge is a play in the client (the jump/pose entry, the FALLINGFAR latch's Fall, the
    /// land pick), so it clears the deferred-combat cache like any normal arm (`0x5fe48e`);
    /// the *level* must not (mid-air the airborne-freeze issues no plays, so a fast-path park
    /// survives to its clip's end).
    last_special: Option<Special>,
    /// The **airborne snapshot** node whose clock [`driver::play::leave_special`] stopped
    /// (decision 0503, the swim hop's too-short kick — kept as a symptom fix whose mechanism is
    /// open, because the "blends from a frozen pose" law it was justified by is refuted: the
    /// client's blend source keeps running, decision **1566**. Scoped to the airborne cut and
    /// **not** to be generalised). The per-frame rate write
    /// ([`driver::play::sync_base_rate`](driver::play)) must not restart it, so it is named here
    /// rather than inferred from a zero speed — a rate-scaled clip legitimately reads 0 when the
    /// body is standing still, and skipping *that* would strand it frozen forever. Self-clearing:
    /// the moment anything else is armed the node stops being the main animation and the sync
    /// drops the name (decision 0906).
    frozen: Option<bevy::animation::graph::AnimationNodeIndex>,
}

impl AnimDriver {
    /// The `AnimationData` ids currently driving this unit — `(base, masked overlay)` — the
    /// inspector card's anim readout. The base is whatever occupies the full-body slot (gait /
    /// special / landing / swing), as the *requested* ids before any missing-clip substitution
    /// (the layer the selectors chose — also what the reconcile tests, decision 0125); the
    /// overlay is a masked upper-body one-shot playing beside it, if any. A `None` base = no
    /// gait selected yet (first frame).
    pub(crate) fn playing(&self) -> (Option<u16>, Option<u16>) {
        let base = match self.mode {
            Mode::Gait => self.gait,
            Mode::Entering(s) => Some(s.enter()),
            Mode::Looping(s) => Some(s.loop_id()),
            Mode::Exiting(_, id) => Some(id),
            Mode::Land { id, .. } => Some(id),
            Mode::Swing { id, .. } => Some(id),
        };
        (base, self.overlay.map(|o| o.id))
    }

    /// The playback rate the base slot is running at ([`Self::gait_rate`]) — the inspector card's
    /// `rate` readout. `1.0` for every clip outside the locomotion whitelist.
    pub(crate) fn rate(&self) -> f32 {
        self.gait_rate
    }
}

/// A masked upper-body play over the base ([`AnimDriver::overlay`]): the graph node it drives
/// ([`AnimClip::upper_node`]) and the requested `AnimationData.dbc` id (for the event scan +
/// the sheath reconcile — both must see this play, decision 0087 (d)). `looping` marks the cast
/// hold's sustained variant (decision 0107): a one-shot auto-releases when its clip finishes; a
/// looping hold never finishes and is released only by [`driver::drive_animations`]'s hold logic
/// (the [`CastHold`] gone, or the unit stopping — the hold then moves to the gait slot).
#[derive(Clone, Copy)]
struct Overlay {
    node: bevy::animation::graph::AnimationNodeIndex,
    id: u16,
    looping: bool,
}

/// A key-bone cross-fade in flight ([`AnimDriver::overlay_fade`], decision 0878). `out` is the
/// **retiring** node, holding the outgoing pose the client snapshots into the bone's secondary
/// slot (`0x7123af`: `rep movsd +0x98 → +0xc4`); it is `None` when
/// a fresh clip fades in over the *inherited base* pose, which needs no node of its own — the base
/// is already there. `left`/`total` are the window: the fixed 150 ms of a fade-to-rest, or the
/// incoming clip's own blendTime on a blended re-arm.
#[derive(Clone, Copy)]
struct OverlayFade {
    out: Option<bevy::animation::graph::AnimationNodeIndex>,
    /// Seconds still to run — λ = [`select::blend_lambda`]`(left / total)`, decaying 1 → 0.
    left: f32,
    total: f32,
}

/// A wound-flinch decay in flight ([`AnimDriver::wound`], decision 0111): the graph node the
/// blend drives (the clip's masked [`AnimClip::upper_node`], or its full-body node when the
/// bone-selection forces bone 0 — [`select::wound_full_body`]) and the decay window's inputs.
/// The client seeds `end = clock + span, rate = 1/span, λ₀ = 0.75` (op4 `linkFlag=0`,
/// `0x712647-0x712682`); the driver's per-frame upkeep replays that decay off the clip's own
/// playback clock and stops the node when it expires — the kernel's self-release (`0x7147b9`).
#[derive(Clone, Copy)]
struct Wound {
    node: bevy::animation::graph::AnimationNodeIndex,
    /// The wound clip's full length (seconds) — the decay denominator (the client's `1/span`).
    span: f32,
    /// Masked to the upper-body subtree — where the base *and* a live one-shot overlay both
    /// blend, so the λ-anchoring weight must count them ([`select::wound_weight`]'s `others`).
    masked: bool,
}

impl Default for AnimDriver {
    fn default() -> Self {
        Self {
            mode: Mode::Gait,
            gait: None,
            gait_flags: 0,
            base_lock: driver::play::BaseAnimLock::default(),
            sheath_cur: None,
            sheath_byte: None,
            sheath_swap: None,
            started_anim: false,
            overlay: None,
            overlay_fade: None,
            wound: None,
            jump_arc: false,
            last_vertical_speed: 0.0,
            was_falling: false,
            deferred: None,
            loop_window: None,
            gait_rate: 1.0,
            mount_display: 0,
            last_special: None,
            frozen: None,
        }
    }
}

impl AnimDriver {
    /// The `AnimationData.dbc` id of the clip this unit is currently playing (target gait, a Special's
    /// enter/loop/exit one-shot, or Death — which the death override records as the gait). `None` before
    /// the first selection. The mouse pick reads it for the **current animation's** bounds sphere — the
    /// real client's broad-phase volume tracks the playing sequence (`0x7089c0`).
    pub(crate) fn active_anim(&self) -> Option<u16> {
        match self.mode {
            Mode::Gait => self.gait,
            Mode::Entering(sp) => Some(sp.enter()),
            Mode::Looping(sp) => Some(sp.loop_id()),
            Mode::Exiting(_, exit) => Some(exit),
            Mode::Land { id, .. } => Some(id),
            Mode::Swing { id, .. } => Some(id),
        }
    }

    /// The **resolved** id of the clip actually driving playback right now (decision 0082):
    /// [`Self::active_anim`] (the selector's *requested* semantic id) run through the model's own
    /// fallback resolution. This is what the real client's per-animation sheath reconcile tests
    /// (`0x5fdb50` reads the record of the sequence **actually playing**, not the semantic pick that
    /// was asked for) and what the event-fire path needs (a clip crossed via resolution is the one
    /// whose timeline is actually advancing). [`Self::active_anim`] itself stays the raw requested id
    /// — the state machine's `Mode`/`gait` identity comparisons key on the *selector's* choice, which
    /// resolution must not perturb (two candidates that happen to resolve to the same baked substitute
    /// are still a different selection). `catalog` absent (a brief window before `AnimationData.dbc`
    /// loads) degrades to identity, matching [`find_resolved`]'s own degrade.
    pub(crate) fn resolved_anim(
        &self,
        anims: &ModelAnimations,
        catalog: Option<&AnimDataCatalog>,
    ) -> Option<u16> {
        self.active_anim().map(|id| resolved_id(anims, id, catalog))
    }

    /// The unit's client-side sheath state (0 stowed · 1 melee · 2 ranged) — the committed CUR
    /// cache the placement renders and the Z toggle flips. `None` until first driven.
    pub(crate) fn sheath_state(&self) -> Option<u8> {
        self.sheath_cur
    }

    /// Did this unit start an animation this frame — see [`Self::started_anim`].
    pub(crate) fn started_anim(&self) -> bool {
        self.started_anim
    }

    /// Stand in for the driver's own write, so a consumer's test can raise the edge without
    /// standing up the whole animation pass to produce it.
    #[cfg(test)]
    pub(crate) fn set_started_anim(&mut self, played: bool) {
        self.started_anim = played;
    }

    /// Whether a draw/stow ceremony is in flight — the manual toggle's mid-ceremony debounce
    /// (guards 11–12 of the client's `ToggleSheath` chain, decision 0080d).
    pub(crate) fn sheath_ceremony_active(&self) -> bool {
        self.sheath_swap.is_some()
    }
}

/// Drives unit animation from movement state. Runs after [`WorldStage::Net`] (where the net bridge updates
/// splines and the controller writes our avatar's state), so what it reads is the current frame's.
pub(crate) struct CreatureAnimPlugin;

impl Plugin for CreatureAnimPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        twist::plugin(app);
        lod::plugin(app);
        breath::register(app);
        // The breath classifier is a per-unit environment resolve, not an animation step: it runs
        // off the area authority, at its own 10 s cadence, and only publishes the component
        // `fire_breath` reads inside the chain below.
        app.add_systems(
            Update,
            classify_breath.after(benilla_world::terrain_stream::AreaAuthoritySet),
        );
        app.add_message::<AnimSoundEvent>()
            .add_message::<SwingMessage>()
            .add_message::<SwingImpact>()
            .add_message::<SwingFlush>()
            .add_message::<DefenseAnim>()
            .add_message::<SwingSlowdown>()
            .init_resource::<PendingImpacts>()
            .init_resource::<PlaySeq>()
            .init_resource::<gesture::GestureQueue>()
            .add_message::<EmoteAnim>()
            .add_message::<BaseAnimRecompute>()
            .add_message::<MountFlourish>()
            .add_message::<WoundAnim>()
            .add_message::<SheathSwapMessage>()
            .add_message::<SheathRequest>()
            .add_message::<CastEvent>()
            .add_message::<SpellGoTargets>()
            .add_message::<SpellKitSound>()
            .add_message::<SpellKitShake>()
            .add_message::<SpellKitFx>()
            .add_message::<MissileSpawn>()
            // The kit's beam edge (0955) — `crate::entities` owns what it becomes.
            .add_message::<ChainProcPlay>()
            .add_message::<KitPush>()
            // The aura CharProc edges the slot watcher emits alongside its effect-model ones —
            // drained by `crate::aura_visual`, which owns the body's alpha/tint for an aura's life.
            .add_message::<crate::aura_visual::AuraProc>()
            .add_message::<HardLanding>()
            // The pending-morph latch — the reference's per-unit `[+0xd54]` SpellRec slot.
            .init_resource::<MorphLatch>()
            .add_systems(
                Startup,
                (
                    load_anim_data,
                    load_spell_visuals,
                    load_blood_tables,
                    load_env_damage_table,
                )
                    .after(AssetSet::Open),
            )
            .add_systems(
                Update,
                (
                    route_cast_visuals,
                    // The lootable-corpse sparkle's edge watcher — a SpellKitFx writer like the
                    // router above, consumed by the same entity-visuals chain this set precedes.
                    arm_loot_fx,
                    // Its level-edge sibling: the ding (decision 0305) — same shape, same home.
                    arm_level_up_fx,
                    // …and its mount-edge sibling: the poof (decision 0927). Same hardcoded-effect
                    // spawn shape, one field over.
                    arm_mount_poof_fx,
                    // The aura-slot watcher: state kits persist for the aura's life (the bread).
                    arm_aura_state_fx,
                    // The pending-morph latch (`0x5ff0c0` arms, `0x60abe0` replays): the aura
                    // add/remove edges arm it, and a display swap's rebuild replays the latched
                    // spell's impact kit — the shapeshift cloud, both directions. Arm before
                    // replay: the swap message crosses from last frame's teardown, and its
                    // latch was armed by the same wire burst.
                    (arm_morph_latch, replay_morph_kit).chain(),
                    blood_spurts,
                    // The landing predictor's dust leg (the vocal leg lives in `sound`).
                    hard_landing_dust,
                    emote_to_anim,
                    // The client-local gestures (decision 1469) — the chat talk/shout/laugh and
                    // the NPC-interact talk. The client's second (and only other) producer of a
                    // one-shot emote; same place in the order as the wire one, for the same reason.
                    gesture::drive_gestures,
                    // The flourish hop (unit → mount child) — before the driver so the
                    // one-shot lands the same frame the packet (or space press) arrived.
                    flourish_to_anim,
                    drive_animations,
                    // The weapon-trail latch (decision 2076) — immediately after the driver,
                    // because the edge it consumes is `AnimDriver::played_anim`, which the driver
                    // rewrites every pass. The reference reads the latch *inside*
                    // `PlayAnimation 0x5fe2f0` itself; one system later is as close as a
                    // batched driver gets, and it is still the same frame.
                    crate::weapon_trail::fire_weapon_trails,
                    drive_hand_grip,
                    fire_anim_events,
                    // After the event scan: consume this frame's impact tags; the SwingImpact
                    // consumers (blood, flinch, floating text) read it next frame — noise against
                    // the ~300–600 ms deferral itself.
                    route_swing_impacts,
                    // The `$BWP`/`$BWR` nock latch, same-frame off the same scan — before the
                    // entity-visuals chain so the arrow appears/vanishes the frame the keyframe
                    // lands, not one behind.
                    drive_nock_latch,
                    // …and, off the same two keys, the RANGED PROP's own clip (decision 2281):
                    // the bow's limbs bend on `$BWP`, and on `$BWR` a gun fires the muzzle blast
                    // its BowRelease(161) sequence carries. Beside the latch because the reference
                    // arms both from one handler each, and ahead of the entity-visuals chain so the
                    // prop's pose is this frame's before the rider lane reads it.
                    crate::ranged_flex::flex_ranged_props
                        // The per-sequence material samplers (`SeqHosts`) ask an anim host which
                        // slot it is playing; a prop that armed 161 this frame must be answering
                        // for 161, not for last frame's Stand. The chain's own `.before(
                        // EntityVisualsSet)` does not reach them — they hang off `ModelVisSet`.
                        .before(benilla_world::model_render::ModelVisSet),
                    // …and the un-nock's reset (`0x60f59d`), which the reference reaches from the
                    // SAME `$BWR` it just armed on. It must run AFTER the arm, or its skip-while-
                    // releasing guard has nothing to read and a gun's blast is cancelled on the
                    // frame it starts — the chain gives it that order by construction.
                    crate::ranged_flex::reset_ranged_props_on_unnock
                        // Same edge as the arm above, and for the same reason: this writes the
                        // prop's player too, and the per-sequence material samplers must answer
                        // for the clip it leaves armed.
                        .before(benilla_world::model_render::ModelVisSet),
                    // …and the `$BTH` puff off the same scan: another SpellKitFx writer, so it
                    // belongs ahead of the entity-visuals chain like its `arm_*_fx` siblings.
                    fire_breath,
                )
                    .chain()
                    .after(WorldStage::Net)
                    // After predicate B's recompute (decision 1477): the loot leg's self trigger
                    // is that boolean, and the reference has no gap between arming the latch and
                    // posing — its chest arm force-plays Loot 50 in the same handler. Without
                    // this edge the kneel lands a frame late at every loot window.
                    .after(crate::ui_loot::resolve_loot_kneel)
                    // After Input so a sheath request (the Z toggle) executes the same frame.
                    .after(WorldStage::Input)
                    // Before the entity-visuals chain: [`VisualSheath`] writes must be applied
                    // before `resolve_equipment` reads them, or a sheath transition double-swaps
                    // the weapon placement for a frame (the flash).
                    .before(crate::entities::EntityVisualsSet),
            )
            // The freeze (CharProc 11, decision 0889): **after the driver**, so this frame's arms
            // are already in when the clocks are held — the Ice Block observable is precisely that
            // the cast one-shot gets armed and then never advances a frame. After the drain too, so
            // the node it reads is this frame's.
            .add_systems(
                Update,
                crate::aura_visual::apply_aura_anim_rate
                    .after(drive_animations)
                    .after(crate::aura_visual::drain_aura_procs),
            );
    }
}

/// Resolve `id` to the id this model actually plays (decision 0082, `0x711bf0`):
/// [`ModelAnimations::resolve`] via `catalog` when `AnimationData.dbc` has loaded, else identity — a
/// brief window at startup ([`load_anim_data`](sheath::load_anim_data) runs at `Startup`, so this
/// degrade is only ever live for the first few frames, same shape as `anim_data.map_or(0, ..)`
/// elsewhere in this module).
fn resolved_id(anims: &ModelAnimations, id: u16, catalog: Option<&AnimDataCatalog>) -> u16 {
    catalog.map_or(id, |cat| anims.resolve(id, cat).id)
}

/// The clip for a *requested* id, resolved through the model's own baked fallback first (decision
/// 0082) — so a model lacking `id` exactly still plays its baked substitute (e.g. the chicken's
/// Attack2H request landing on AttackUnarmed) instead of coming back empty.
fn find_resolved<'a>(
    anims: &'a ModelAnimations,
    id: u16,
    catalog: Option<&AnimDataCatalog>,
) -> Option<&'a AnimClip> {
    anims.find(resolved_id(anims, id, catalog))
}

#[cfg(test)]
mod attack_stand_tests {
    use super::*;

    /// **`StartAttack` stands a seated player** — `0x5ecc9b`, and unconditionally within this
    /// function (decision 1768).
    ///
    /// `engaged = true` is the case worth pinning. It takes `0x5ecb70`'s skip, which jumps to the
    /// tail at `0x5ecd78` — **73 bytes past the stand** — so re-issuing an attack you are already
    /// making still stands you, and no `CMSG_ATTACKSWING` goes out. A stand written only on the
    /// swing's own leg would miss exactly this case.
    ///
    /// This is the behaviour that replaces the turn-driven stand 1766 deleted: what stands a
    /// seated player on a right-click is the *action* the click dispatches, not the gesture.
    #[test]
    fn starting_an_attack_stands_you_even_when_the_swing_is_skipped() {
        let mut app = App::new();
        app.add_message::<SheathRequest>()
            .add_message::<crate::player::StandStateRequest>();
        // A dead-letter net channel: the send is `let _ = …` and no packet is under test here.
        let (tx, _rx) = crossbeam_channel::unbounded();
        app.insert_resource(crate::net::NetCommands(tx));
        app.insert_resource(crate::spell::AutoRepeatActive(None));

        let me = app.world_mut().spawn_empty().id();
        app.add_systems(
            Update,
            move |mut commands: Commands,
                  mut auto_repeat: ResMut<crate::spell::AutoRepeatActive>,
                  mut sheath: MessageWriter<SheathRequest>,
                  net: Res<crate::net::NetCommands>| {
                start_attack_local(
                    me,
                    0xdead_beef,
                    true, // already engaged — the skip leg
                    false,
                    &mut auto_repeat,
                    &mut sheath,
                    &mut commands,
                    &net,
                );
            },
        );
        app.update();

        let stands: Vec<u8> = app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<crate::player::StandStateRequest>>()
            .drain()
            .map(|r| r.state)
            .collect();
        assert_eq!(
            stands,
            vec![0],
            "StartAttack asks for stand state 0 exactly once, even on the leg that sends no swing"
        );
    }
}

#[cfg(test)]
mod nock_latch_tests {
    use super::*;

    /// The `$BWP`/`$BWR` nock cycle (set `0x624b70`, cleared `0x60016c`): the pull keyframe
    /// latches (only a unit with a cached ammo display), the release keyframe clears — the arrow
    /// appears as she draws it and leaves the hand with the shot.
    #[test]
    fn bwp_latches_and_bwr_clears_the_nock() {
        let mut app = App::new();
        app.add_message::<AnimSoundEvent>();
        app.add_systems(Update, drive_nock_latch);
        let unit = app.world_mut().spawn(NockedAmmo { display_id: 5996 }).id();
        app.world_mut().write_message(AnimSoundEvent {
            entity: unit,
            ident: *b"$BWP",
            data: 0,
            anim_id: 0,
            pos: None,
        });
        app.update();
        assert!(
            app.world().entity(unit).contains::<NockLatch>(),
            "$BWP latches a unit with a cached ammo display"
        );
        app.world_mut().write_message(AnimSoundEvent {
            entity: unit,
            ident: *b"$BWR",
            data: 0,
            anim_id: 0,
            pos: None,
        });
        app.update();
        assert!(
            !app.world().entity(unit).contains::<NockLatch>(),
            "$BWR clears — the arrow leaves with the shot"
        );
        // No cached ammo display → the pull nocks nothing (the client's handler gates on the
        // cached ranged spell).
        let bare = app.world_mut().spawn_empty().id();
        app.world_mut().write_message(AnimSoundEvent {
            entity: bare,
            ident: *b"$BWP",
            data: 0,
            anim_id: 0,
            pos: None,
        });
        app.update();
        assert!(
            !app.world().entity(bare).contains::<NockLatch>(),
            "no ammo display, no latch"
        );
    }
}

#[cfg(test)]
mod disarm_tests {
    use super::*;

    /// **The ladder, hand by hand** (decision 1863; `0x5ec240`/`0x605e30`). The
    /// reference hides **exactly one weapon**, main hand first — so a disarmed dual-wielder
    /// punches with the main hand and still swings its off-hand weapon. Every selector downstream
    /// reaches its own unarmed leg by its own table; there is no disarm case in any of them.
    #[test]
    fn disarm_hides_one_weapon_main_hand_first() {
        let worn = Wielded {
            main: Some((2, 0x7)),   // 1H sword
            off: Some((2, 0xf)),    // dagger
            ranged: Some((2, 0x2)), // bow
            ..Default::default()
        };
        let disarmed = Wielded {
            disarmed: true,
            ..worn
        };

        // CONTROL — the weapon tables, untouched while the flag is down.
        assert_eq!(worn.disarmed_hand(), None);
        assert_eq!(select::swing_anim_main(worn.armed_main()), 17); // Attack1H
        assert_eq!(select::swing_anim_off(worn.armed_off()), 88); // AttackOffPierce
        assert_eq!(select::ready_anim(worn.armed_main()), 26); // Ready1H

        // Disarmed, both hands full: the MAIN HAND is the one hidden, and its claim CANCELS the
        // off-hand probe (`5ec28d je 0x5ec2aa`) — the dagger keeps swinging as a dagger.
        assert_eq!(disarmed.disarmed_hand(), Some(0));
        assert_eq!(disarmed.armed_main(), None);
        assert_eq!(disarmed.armed_off(), Some((2, 0xf)), "the off hand is KEPT");
        assert_eq!(select::swing_anim_main(disarmed.armed_main()), 16); // AttackUnarmed
        assert_eq!(select::swing_anim_off(disarmed.armed_off()), 88); // still the dagger
        assert_eq!(select::ready_anim(disarmed.armed_main()), 25); // ReadyUnarmed

        // The RANGED slot has no gate at all: `0x5ec25e` pushes equip index `0x11` and jumps
        // straight past both probes, so a disarmed hunter still nocks.
        assert_eq!(disarmed.ranged, Some((2, 0x2)));
        assert_eq!(select::ranged_load_anim(disarmed.ranged), 105); // LoadBow

        // …and the WORN reading is untouched throughout — it is what the paperdoll shows.
        assert_eq!(disarmed.main, worn.main);
    }

    /// The off hand is hidden only when the main hand does **not** claim the disarm — an empty or
    /// non-weapon main hand. That is the one case that reaches AttackUnarmedOff(117), whose
    /// `AnimationData.dbc` row (id 117, fallback 87) is a clip distinct from AttackUnarmed(16).
    #[test]
    fn an_empty_main_hand_hands_the_disarm_to_the_off_hand() {
        let off_only = Wielded {
            main: None,
            off: Some((2, 0xf)),
            disarmed: true,
            ..Default::default()
        };
        assert_eq!(off_only.disarmed_hand(), Some(1));
        assert_eq!(off_only.armed_off(), None);
        assert_eq!(select::swing_anim_off(off_only.armed_off()), 117);

        // A NON-WEAPON main hand does not claim it either — the test is `ItemClass == 2` only.
        let torch = Wielded {
            main: Some((15, 0)), // class 15 = miscellaneous
            off: Some((2, 0xf)),
            disarmed: true,
            ..Default::default()
        };
        assert_eq!(torch.disarmed_hand(), Some(1));
        assert_eq!(
            torch.armed_main(),
            Some((15, 0)),
            "a non-weapon is not taken"
        );
        assert_eq!(torch.armed_off(), None);
    }

    /// Neither hand holds a weapon: the flag hides nothing at all, and a shield stays a shield.
    #[test]
    fn the_ladder_takes_weapons_and_leaves_the_rest() {
        let shielded = Wielded {
            main: None,
            off: Some((4, 6)), // class 4 = ARMOR, subclass 6 = shield
            disarmed: true,
            ..Default::default()
        };
        assert_eq!(shielded.disarmed_hand(), None);
        assert_eq!(shielded.armed_off(), Some((4, 6)));
    }

    /// The pure ladder, over the classes alone — the form the action bar's equipped-item test
    /// (`0x5ea5d0`'s slot 0/1/2 loop) reads off the raw inventory.
    #[test]
    fn the_ladder_over_classes() {
        assert_eq!(disarmed_hand(Some(2), Some(2)), Some(0));
        assert_eq!(disarmed_hand(Some(2), None), Some(0));
        assert_eq!(disarmed_hand(None, Some(2)), Some(1));
        assert_eq!(disarmed_hand(Some(4), Some(2)), Some(1));
        assert_eq!(disarmed_hand(None, None), None);
        assert_eq!(disarmed_hand(Some(4), Some(4)), None);
    }
}
