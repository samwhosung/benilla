//! Route M2 animation event tags (`crate::creature_anim::AnimSoundEvent`) to audio — the
//! anim-driven trigger surface (decision 0070 slice 3).
//!
//! Routed here: `$SND`/`$DSO` (one-shot kit `data` at the model), `$DSL`/`$DSE` (an ambient loop
//! — **registered into, and released from, the emitter pool** [`super::emitter_pool`], which owns
//! everything about how it sounds; the two dispatchers' arms differ and both are modelled below),
//! `$CSD` — the **character emote clips' embedded voice** (HumanMale EmoteLaugh 70 carries
//! `$CSD 6923` = the SoundEntries kit literally named `HumanMaleEmoteLaugh`; Cry 77 → 6921,
//! Chicken 78 → 6919, Applaud 80 → 4× `ClapSounds` 6576 — probe-verified on the real 5875 M2 +
//! SoundEntries; the client's `$CSD` handler `0x623c10` → `0x459230` plays the event payload as
//! a literal SoundEntries id, byte-confirming the routing) — and the **gathering/work pair**
//! (decision 0562):
//!
//! - **`$TRD`** (`0x62faa0`): the in-flight spell's `SpellVisual` **field-14 strike sound**,
//!   positioned — **the mining pick clang** (visual 93 → 1143 "Mining Impact") and the crafting
//!   hammer (the smithing visuals carry the same field), fired at the work anims' 0.666 s
//!   impact keyframe. Fully client-side: the in-flight spell is the unit's cast hold (the
//!   client caches it from the local GO interaction, `0x6ec220` → `[CGUnit+0xc8c]`), so no
//!   server state is involved.
//! - **`$ESD`** (`0x6239f0`): the unit's `UNIT_NPC_EMOTESTATE` → `Emotes.dbc` `EventSoundID`,
//!   gated on `EmoteSpecProc == 2`, positioned at the unit — wire-driven work-state sounds
//!   (a chopwood camp worker's state 234 → 3202; state 233 carries a second mining kit 3782,
//!   which no vmangos path ever sets for a player — verified at its source).
//!
//! `$CST`/`$CSL`/`$CSR` are still NOT routed, but **the reason we gave was wrong** and is
//! corrected here. The handler `0x60c940` does only
//! 3D-**reposition** the already-playing cast handle — that part held — but it repositions it to
//! the **event's own point** (`0x600143 mov edx,[ebx+0x10]` → `0x60c960`/`0x60c990` → `0x61ceb0`),
//! i.e. to the casting hand, not to the caster. benilla's kit player tracks a looping cast sound
//! to the caster's origin, so the role is *not* covered: we are a body-width out for the whole
//! cast. Left unbuilt rather than half-built, because what the reference does when the
//! GUID-tracked follow and this reposition disagree is not settled here — decision 1915's open.
//!
//! `$FD1..$FD9`/`$FDX` have no sound route at all yet (they are the CreatureSoundData fidget
//! family). When one lands it wants the **unit origin + 2.0 z** (`0x6232c0` → `0x6230a0`), not
//! the fired key's point — the same shape `$TRD` takes below.
//!
//! The footstep family and the CreatureSoundData-driven tags (`$FD*` fidgets, `$AH*` attacks,
//! `$CSS` swings) are routed by their own consumers as those land (slice-3 tasks); unrecognized
//! tags are trace-logged so the stream is observable without spam.

use bevy::prelude::*;

use crate::creature_anim::{held_strike_sound, AnimSoundEvent, CastHold, SpellVisuals};
use crate::net::ObjectStore;
use benilla_assets::WorldAssets;
use benilla_world::schedule::WorldStage;

use super::emitter_pool::AmbientEmitterPool;
use super::emote::EmoteSounds;
use super::kit::{play_kit, KitRef, SoundCategory, SoundKits};
use super::{AudioListener, SoundConfig, SoundOutput};

/// `$TRD`'s own z-bias — `0x62fb3f fadd [0x7ff9d8]`, the constant `1.0`. Distinct from the
/// footstep foley's `2.0` and from the per-attachment table, and it is added to the unit's own
/// position because this arm is handed no point at all.
const TRD_HEIGHT: f32 = 1.0;

/// The attachment the emote voice is born at — `0x623c3a push 0x11`.
const CSD_ATTACH: u16 = 17;

pub(super) fn route_anim_events(
    mut events: MessageReader<AnimSoundEvent>,
    // GlobalTransform: `$SND` tags can fire from parented visuals (a mount child's model),
    // whose local Transform is not a world position (0441 fold-back).
    transforms: Query<&GlobalTransform, Without<Camera3d>>,
    units: Query<(Option<&ObjectStore>, Option<&CastHold>)>,
    // Which dispatcher this entity's events arrive on. A family-A GameObject carries
    // [`crate::go_anim::GoAnim`] — the same population the reference registers `0x5f3e20` for —
    // and that dispatcher's DS-family arms differ from the placed-M2 handler `0x6951e0`'s.
    go_lane: Query<(), With<crate::go_anim::GoAnim>>,
    // The attachment reads `$CSD` needs (`0x623b90`) — the same pure position read the overhead
    // anchor makes, spawning nothing.
    attach: crate::entities::AttachPoints,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    emotes: Option<Res<EmoteSounds>>,
    spells: Option<Res<crate::ui_action::Spells>>,
    visuals: Option<Res<SpellVisuals>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
    // Kit ids already complained about. A failing kit here is a PER-EVENT failure on a stream that
    // fires at doodad rates, so warning every time is a log flood, not a diagnostic: one live run
    // past Darnassus produced 420 identical lines for `NightElfLantern01`'s `$DSL(33764)`, an id
    // that is simply not in 5875's `SoundEntries` (32401 is the corpus's only other one). The
    // reference does nothing audible for an id it cannot resolve, so this is data, not an error —
    // but it is still worth saying once, because a kit that goes missing for any OTHER reason is a
    // real bug and silence would hide it.
    mut complained: Local<std::collections::HashSet<u32>>,
    // The ambient emitter pool — `$DSL`/`$DSE`'s whole destination.
    mut pool: ResMut<AmbientEmitterPool>,
) {
    if events.is_empty() {
        return;
    }
    let (Some(mut kits), Some(assets)) = (kits, assets) else {
        return;
    };
    let listener = listener.pos;
    let ring = |kits: &mut SoundKits,
                out: &mut SoundOutput,
                kit: u32,
                ev: &AnimSoundEvent,
                complained: &mut std::collections::HashSet<u32>| {
        // The fired key's own point where the arm is byte-proven to pass it (below), else the
        // model root — which is what every arm here used unconditionally before 1904.
        let pos = ev
            .pos
            .or_else(|| transforms.get(ev.entity).map(|t| t.translation()).ok());
        if let Err(e) = play_kit(
            kits,
            &assets,
            out,
            &config,
            listener,
            KitRef::Id(kit),
            pos,
            SoundCategory::Sfx,
        ) {
            if complained.insert(kit) {
                warn!("anim event kit {kit}: {e:#} (further reports for this kit suppressed)");
            }
        }
    };
    for ev in events.read() {
        match &ev.ident {
            // `$DSL` — the DOODAD SOUND **LOOP** (`0x69521d`). A persistent handle with a
            // lifecycle, one per doodad (`[CMapDoodadDef+0x168]`): crossing the marker again
            // **repositions** the existing registration (`0x462000`) and never restarts it; a
            // DIFFERENT id releases the old one (`0x461f80`) and registers the new (`0x461d80`). So
            // there is no wrap retrigger at all — which the shipped audio already implied, since
            // `NightElfStreetLampLoop` is 4.000 s on a 3.333 s sequence and `CampFireSmallLoop` is
            // 2.967 s on the same, mismatched in both directions.
            //
            // **It always loops, and consults NO flag.** Looping is an entry-point constant on the
            // reference's registration pool: `0x7a54d0` builds mode `0x1002` (`HW3D|LOOP_NORMAL`)
            // and calls `SetLoopCount(stream, -1)`, against `0x7a5490`'s `0x1000` for the one-shot
            // path. This corrects the interim that shipped with the first half of B345: the kit's
            // 0x200 bit has exactly ONE reader image-wide (`0x458840`), whose two callers are the
            // GameObject display-slot lane select and the spell-visual lane — it reaches neither
            // mode word and is a LANE SELECT, not a loop flag. (0x400, which correlated perfectly
            // with the four non-sustaining `$DSL` kits in the shipped data, is `random pitch`; the
            // correlation was authoring practice — you do not detune a sustained hum.) `force_loop`
            // here is therefore the faithful shape, not a workaround: 25 of the 60 kits a `$DSL`
            // names omit 0x200 and every one of them loops in the real client.
            // `$DSL` — the DOODAD SOUND **LOOP** (`0x69521d`). It does not start a sound. It
            // **registers this doodad's position** as one emitter of its SoundEntries id in the
            // pool at `0xb06dd8` (`0x461d80`), and re-crossing the marker only *repositions* that
            // registration (`0x462000`) — which is why there is no wrap retrigger, and why
            // `NightElfStreetLampLoop` (4.000 s of sample on a 3.333 s sequence) is not chopped
            // every cycle. Whether anything is audible, from where, and how many at once are the
            // pool pump's questions, not this scanner's: see [`super::emitter_pool`].
            b"$DSL" if ev.data != 0 => {
                // **The emitter's point is the marker's, not the model's** (decision 1904). Both
                // handlers take the kernel's `eventWorldPos` as an argument and pass it straight
                // into the pool: the placed-M2 lane `0x6951e0` is `fn(fourcc, data, &worldPos, …)`
                // with `[ebp+0x10]` the `C3Vector*` it hands to `0x461d80`/`0x462000`, and the
                // GameObject lane `0x5f3fe5` does the same with its own `p3`. It is not a detail
                // here: 149 of the 244 shipped `$DSL` records sit off their model's origin, out to
                // **67.6 yd** on `Maraudon_Waterfall01.m2` — a waterfall whose roar was landing at
                // the model's pivot instead of the water.
                if let Some(at) = ev
                    .pos
                    .or_else(|| transforms.get(ev.entity).ok().map(|t| t.translation()))
                {
                    // **The two lanes' `$DSL` arms differ, and only here**. The placed-M2 handler
                    // `0x6951e0` compares the id and swaps; the GameObject dispatcher's arm
                    // `0x5f3fe5` does **not compare it at all** — a live handle is only
                    // repositioned, whatever id the marker names. Onyxia's lava trap is the case
                    // that makes it observable: `ONYZIASLAIRLAVATRAP.M2` (208 spawns in the lair)
                    // authors `$DSL(8681)` on its chained Stand variation and `$DSL(8682)` on
                    // Custom0, so on the GameObject lane the Custom0 hum never displaces the
                    // Stand one.
                    if go_lane.contains(ev.entity) {
                        super::emitter_pool::register_keeping_first(
                            &mut pool, ev.entity, ev.data, at, listener,
                        );
                    } else {
                        super::emitter_pool::register(&mut pool, ev.entity, ev.data, at, listener);
                    }
                }
            }
            // `$DSE` — the doodad sound **STOP** token (`0x45534424`): it releases the doodad's
            // registration (`0x461f80`), and its `data` is 0 on all 16
            // shipped models. Without it a `$DSL` started at a keyframe never ends — which is
            // exactly the elevator and machinery family (`GnomereganElevatorLoop`, `SubwayLoop`,
            // the Undercity and Thunder Bluff lifts, the zeppelin), where the loop is authored to
            // run for one leg of the animation and stop. Releasing a record is not stopping a
            // sound: the id keeps sounding while any *other* doodad still names it.
            // …and `$DSE` has **no arm on the GameObject dispatcher at all** — the token falls to
            // `0x5f4004 ret`. Seven shipped GO display models author one anyway (the Maraudon
            // corrupted plants on their Destroy clip, the Blackrock door mechanism on Closed, the
            // Maraudon teleporter on Open); on that lane what actually drops the registration is
            // the state-machine dispatch those very clips are armed BY
            // ([`crate::go_anim::GoStateDispatch`]), so honouring the authored intent and
            // honouring the bytes agree — but only one of them is the mechanism.
            b"$DSE" if !go_lane.contains(ev.entity) => {
                super::emitter_pool::release(&mut pool, ev.entity);
            }
            // `$SND`/`$DSO` positioned at the fired key: both proven lanes hand the arm the
            // kernel's point and it reaches `0x458870(id, pos, -1, 1.0f)` unchanged — the placed-M2
            // handler at `0x695205`, the GameObject one at `0x5f3fe0 → 0x5f3f60`.
            //
            // **`$CSD` is deliberately still at the model root.** It has no arm on either of those
            // dispatchers: it is the CGUnit lane's (`0x623c10` → `0x459230`), and whether *that*
            // dispatcher's arms take the event point or the unit's own is the one piece of this
            // mechanism not yet read from the binary — open, not assumed. It matters: every player
            // model authors six `$CSD` records, all on the head.
            b"$SND" | b"$DSO" if ev.data != 0 => {
                ring(&mut kits, &mut out, ev.data, ev, &mut complained);
            }
            // **`$CSD` is at ATTACHMENT 17, never the fired key's point.** The dispatcher hands
            // `0x623c10` the *data* and no position (`0x5ffeed`); it asks `0x623b90` for
            // attachment `0x11`, falling back to `GetPosition + zBias[17] = 2.0`. The corpus makes
            // this the sharpest correction in the table: every player model authors **six** `$CSD`
            // records, one per emote clip, all on the head — and not one of them decides where the
            // voice plays. (The reference then GUID-binds the handle so it follows the unit,
            // `0x7a57e0`/`[unit+0xb28]`; ours is a one-shot at the onset point, which is the same
            // sound for a body that is not walking away mid-laugh — noted in 1915.)
            b"$CSD" if ev.data != 0 => {
                let root = transforms
                    .get(ev.entity)
                    .map_or(Vec3::ZERO, |t| t.translation());
                let at = attach.point(ev.entity, CSD_ATTACH, root);
                let voiced = AnimSoundEvent {
                    pos: Some(at),
                    ..*ev
                };
                ring(&mut kits, &mut out, ev.data, &voiced, &mut complained);
            }
            b"$ESD" => {
                let Some(emotes) = emotes.as_deref() else {
                    continue;
                };
                let state = units
                    .get(ev.entity)
                    .ok()
                    .and_then(|(store, _)| store)
                    .map_or(0, |s| s.0.unit_emote_state());
                if let Some(kit) = (state != 0)
                    .then(|| emotes.state_event_sound(state))
                    .flatten()
                {
                    // **EVENT POINT** — `0x5fff5a` pushes the dispatcher's point and `0x623a1e`
                    // hands it straight to `0x458870` (that `edx` is the point, not the payload).
                    ring(&mut kits, &mut out, kit, ev, &mut complained);
                }
            }
            b"$TRD" => {
                let (Some(spells), Some(visuals)) = (spells.as_deref(), visuals.as_deref()) else {
                    continue;
                };
                let hold = units.get(ev.entity).ok().and_then(|(_, h)| h);
                let kit = hold.and_then(|h| held_strike_sound(spells, &visuals.0, h.spell_id));
                if let Some(kit) = kit {
                    // **UNIT ORIGIN + 1.0 z** — the dispatcher hands `0x62faa0` nothing at all
                    // (`0x5ffedb mov ecx,esi; call`), and it re-derives: `0x62fb39 call [edx+0x14]`
                    // GetPosition, `0x62fb3f fadd [0x7ff9d8]` = +1.0, then `0x458870`. The mining
                    // pick's clang comes from the miner, raised, not from the pick's own keyframe.
                    let at = transforms
                        .get(ev.entity)
                        .map(|t| t.translation() + Vec3::Y * TRD_HEIGHT)
                        .ok();
                    let unit_root = AnimSoundEvent { pos: at, ..*ev };
                    ring(&mut kits, &mut out, kit, &unit_root, &mut complained);
                }
            }
            other => {
                trace!(
                    "anim event {} (data {}) — no route yet",
                    String::from_utf8_lossy(other),
                    ev.data
                );
            }
        }
    }
}

/// Registration hook for [`super::SoundPlugin`].
pub(super) fn plugin(app: &mut App) {
    app.add_systems(Update, route_anim_events.in_set(WorldStage::Present));
}
