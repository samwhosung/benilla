//! Creature voice — CreatureSoundData-driven vocals (decision 0070 slice 3): the death cry on a
//! live health→0 transition, and the anim-tag vocals (`$FD1..4` fidgets, `$FDX` stand,
//! `$WNG`/`$WGG` wing flap/glide).
//!
//! The swing-driven vocals (exertion/injury) and the `$AH0..3` custom-attack columns live in
//! [`super::combat`] (decisions 0075 + 0525 — the custom attacks are the swing dispatch's
//! natural-weapon impact, not free-standing anim vocals); the
//! aggro/alert flares land here ([`ai_reaction_vocals`], decision 0280), and so does the ambient
//! body loop ([`creature_body_loops`] — the `loop_sound` column, `0x623800`'s alive-gate). Still
//! untriggered — data in the catalog, triggers INFERRED (0280's §5): stun/jump_start/jump_end
//! (offsets verified, live triggers likely M2 tags — unpinned).

use bevy::ecs::entity::EntityHashMap;
use bevy::prelude::*;

use benilla_formats::CreatureVoiceCatalog;
use benilla_protocol::EntityKind;

use crate::creature_anim::AnimSoundEvent;
use crate::entities::mount::{MountBody, MountChild};
use crate::net::{FieldChanged, NetEntity, ObjectStore, SelfPlayer};
use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_world::schedule::WorldStage;

use super::kit::{
    bark_chance_pass, object_sound_playing, play_kit, play_kit_ext, source_kit_playing,
    stop_source_kit, stop_unit_voice, unit_voice_state, KitRef, Latch, PlayExtras, SoundCategory,
    SoundKits, STAND_CHANCE, STAND_COOLDOWN,
};
use super::{AudioListener, SoundConfig, SoundOutput};

/// The display→voice catalog (CreatureDisplayInfo.SoundID → CreatureSoundData).
#[derive(Resource)]
pub(super) struct CreatureVoices(pub(super) CreatureVoiceCatalog);

fn load_creature_voices(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_creature_voice_catalog(&mut chain)
    };
    match loaded {
        Ok(cat) => {
            info!("sound: {} creature voice rows", cat.len());
            commands.insert_resource(CreatureVoices(cat));
        }
        Err(e) => warn!("sound: creature voices failed to load: {e:#}"),
    }
}

/// `0x623a40`'s bark states, in the order its jump table `0x623afc` lists them — the number IS
/// the priority (higher wins, `0x623a82`). State `3` is a real arm that plays nothing and has no
/// known caller, so it has no constant.
const BARK_AGGRO: u8 = 0;
const BARK_PET_ORDER: u8 = 1;
const BARK_PET_ATTACK: u8 = 2;
const BARK_DEATH: u8 = 4;

/// Step 3 of [`play_bark`] — `0x623a66`-`0x623a88`, the priority comparison alone.
///
/// `latched` is the state of the bark currently sounding on this unit, or `None` for a free slot.
/// The reference's two `je`s skip the comparison entirely when the handle is absent or finished,
/// so a free slot admits **every** state including `0`; a held slot admits only a *strictly*
/// higher one (`jle` aborts on equal, which is what keeps a burst of identical barks to one).
const fn bark_admitted(latched: Option<u8>, state: u8) -> bool {
    match latched {
        None => true,
        Some(held) => state > held,
    }
}

/// Step 5 of [`play_bark`] — the jump table at `0x623afc`, as a lookup. `0` means "this state
/// plays nothing", which covers both the table's own silent arm (state 3, `0x623af0`) and its
/// out-of-range default (`0x623aa0 cmp eax,4; ja`), and is also what an unpopulated column reads
/// as. All three are the same observable in the reference and the same `0` here.
const fn bark_kit(voice: &benilla_formats::CreatureVoice, state: u8) -> u32 {
    match state {
        BARK_AGGRO => voice.aggro,
        BARK_PET_ORDER => voice.pet_order,
        BARK_PET_ATTACK => voice.pet_attack,
        BARK_DEATH => voice.death,
        _ => 0,
    }
}

/// **The creature bark dispatcher, whole** — `0x623a40(ecx = unit, state)`, the one function
/// every one-shot creature vocal below goes through (decision 2039). Four states are live here:
/// `0` the HOSTILE aggro flare, `1` the pet's ORDER bark, `2` the pet's ATTACK bark, `4` the
/// death cry. (State `3` exists in the table and plays nothing — it still stops and latches.)
///
/// Reading it as one function is the point: it was three call sites with three different subsets
/// of its gates before, and the pet barks could not be added correctly to any of them, because
/// what makes a pet bark audible over a pet's near-continuous aggro flares is exactly the
/// priority rule the aggro-only site had no reason to carry.
///
/// The order of operations is the reference's, and it is load-bearing:
///
/// 1. **No voice row, no bark** (`0x623a4a`) — the caller's `for_display` miss.
/// 2. **A server-pushed object sound live on this unit mutes it** (`0x623a59` → `0x4591f0`): a
///    scripted voice line is not talked over. Unlike the class route `0x623490`/`0x62f880`, this
///    one is a direct call, not a virtual — so it has **no player exemption**.
/// 3. **The priority latch** (`0x623a66`–`0x623a88`): while `[unit+0xb20]` holds a live handle, a
///    state `<=` the latched one is dropped outright. A free slot skips the comparison entirely.
/// 4. **Stop the old and latch the new** (`0x623a8d`/`0x623a95`) — *before* the column is read.
/// 5. **Then** resolve the column and play. **A zero column still silences step 4's victim** —
///    the handle is overwritten with the null result (`0x623aee`), so a state whose column is
///    unpopulated cuts short whatever it superseded and sounds nothing in its place. That is the
///    client's behaviour, not an oversight of ours. (It is not reachable through the pet barks on
///    a vmangos server: `SendPetTalk` is gated on `SUMMON_PET`, and only those four voices carry
///    the columns — see [`pet_talk_vocals`].)
fn play_bark(
    kits: &mut SoundKits,
    assets: &WorldAssets,
    out: &mut SoundOutput,
    config: &SoundConfig,
    listener: Vec3,
    unit: Entity,
    pos: Vec3,
    voice: &benilla_formats::CreatureVoice,
    state: u8,
) {
    if object_sound_playing(out, unit) {
        return;
    }
    if !bark_admitted(unit_voice_state(out, unit), state) {
        return;
    }
    stop_unit_voice(out, unit);
    let kit = bark_kit(voice, state);
    if kit == 0 {
        return;
    }
    if let Err(e) = play_kit_ext(
        kits,
        assets,
        out,
        config,
        listener,
        KitRef::Id(kit),
        Some(pos),
        SoundCategory::Sfx,
        PlayExtras {
            source: Some(unit),
            latch: Latch::Voice(state),
            ..default()
        },
    ) {
        warn!("creature bark state {state} (kit {kit}): {e:#}");
    }
}

/// Play the death vocal on a **live** death — the reference's two triggers, each a field edge
/// (decision 2297): the `UNIT_FIELD_HEALTH` watcher's alive→dead arm (`0x6046f0` at `0x6047a3`,
/// `OLD > 0 && NEW ≤ 0` — the real death handler `0x605860`) and the `UNIT_DYNAMIC_FLAGS`
/// watcher's `UNIT_DYNFLAG_DEAD` SET edge (`0x600543`), which fires the very same `0x623a40(4)`
/// — so a feign drops the body with its death cry, exactly like a kill (decision 1022). A unit
/// that streams in already dead cries nothing: the edge stream is create-suppressed, the same
/// distinction the animation driver makes for the settled-corpse pose.
fn death_vocals(
    mut edges: MessageReader<FieldChanged>,
    units: Query<(&NetEntity, &Transform)>,
    voices: Option<Res<CreatureVoices>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
) {
    use benilla_protocol::field::{FIELD_UNIT_DYNAMIC_FLAGS, FIELD_UNIT_HEALTH, UNIT_DYNFLAG_DEAD};
    let (Some(voices), Some(mut kits), Some(assets)) = (voices, kits, assets) else {
        return;
    };
    let listener = listener.pos;
    for e in edges.read() {
        let died = (e.unit_field(FIELD_UNIT_HEALTH) && e.old > 0 && e.new == 0)
            || (e.unit_field(FIELD_UNIT_DYNAMIC_FLAGS)
                && e.old & UNIT_DYNFLAG_DEAD == 0
                && e.new & UNIT_DYNFLAG_DEAD != 0);
        if !died {
            continue;
        }
        let Ok((net, transform)) = units.get(e.entity) else {
            continue;
        };
        let Some(voice) = net.display_id.and_then(|d| voices.0.for_display(d)) else {
            continue;
        };
        play_bark(
            &mut kits,
            &assets,
            &mut out,
            &config,
            listener,
            e.entity,
            transform.translation,
            voice,
            BARK_DEATH,
        );
    }
}

/// The aggro/alert flare vocals (`SMSG_AI_REACTION` → the net bridge; decision 0280): HOSTILE →
/// the aggro bark (CreatureSoundData col 10), ALERT → the alert bark (col 13) — pure audio, like
/// the client (`0x6056e0` plays no animation/UI on either leg).
///
/// **HOSTILE rides the unit's one-shot voice channel, and that is not a nicety** (decision 1399):
/// the bark goes out through `0x623a40(0)`,
/// which stores its channel in `[unit+0xb20]` with category 0 latched in `[unit+0xb24]` —
/// **the lowest category in the table, so it never interrupts a playing bark and a repeat is
/// dropped while one is live** ([`unit_voice_playing`]). The wire does not send this packet
/// sparingly: vmangos fires `SMSG_AI_REACTION` HOSTILE from `Unit::Attack` on every target
/// acquisition *and* from `Unit::SendPetAIReaction` on every pet autocast and self-picked
/// target (`PetAI::DoAttack`, `PetAI::UpdateAI`'s autocast leg), so a hunter's pet in a normal
/// fight draws them in bursts. Played ungated, a level-60 bear pet fired its 3.166 s
/// `mBearAggroA.wav` **63 times in two minutes** — three copies inside a single frame, then a
/// ~10 Hz run — which is up to thirty overlapping copies of one roar, and is what the director
/// heard as a phasing, stuttering, constantly-growling bear. The reference receives the same
/// packets and collapses them at this slot.
///
/// **ALERT is a different route, and is faithfully unconditional** — pinned since, decision 1401.
/// It reaches `vtable+0x88(8,0)` → `0x623490` → col 13, and it *is* rolled, but the class-8
/// threshold is **100** and the compare is inclusive, so `P = 1`. Its one
/// real gate is a mute while a **server-pushed** object sound is live on the unit — the
/// `SMSG_PLAY_OBJECT_SOUND` (opcode `0x278`) registry. ALERT stores no latch and stays off the
/// slot.
///
/// **That gate is now in.** 1401 recorded the opcode as one "benilla does not implement at all",
/// which was wrong when it was written — benilla has parsed, decoded and played
/// `SMSG_PLAY_OBJECT_SOUND` since well before it (`sound::net::play_object_sound` →
/// `ServerSoundKind::ObjectSound` → `sound::zone::server_sounds`, which even resolves the source
/// entity to position the kit at it). What was missing was the **per-GUID registry** the gates
/// consult (`0x4591f0`, tested by `0x623a40`'s gate (i) and by `0x623490`'s gate 3 for classes 0,
/// 1, 2, 3 and 8): the pushed channel played untagged, so nothing could ask "is an object sound
/// live on this unit".
///
/// It could not be tagged with a bare `PlayExtras::source`, because that shared tag also answered
/// the greeting latch — a third meaning in one field. [`kit::Latch`] split those apart (decision
/// 1399), and `Latch::ObjectSound` is the registry: a resolved object sound registers on its unit
/// for as long as it sounds, and [`kit::object_sound_playing`] is `0x4591f0`. ALERT (class 8) and
/// HOSTILE (the priority route) consult it below; the combat vocals (classes 0–3) consult it in
/// `super::combat`. Players are exempt — the CGPlayer twin `0x62f880` omits the gate entirely.
fn ai_reaction_vocals(
    mut reactions: MessageReader<crate::net::AiReactionMessage>,
    units: Query<(&NetEntity, &Transform, Option<&MountChild>)>,
    mounts: Query<&NetEntity, With<MountBody>>,
    voices: Option<Res<CreatureVoices>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
) {
    if reactions.is_empty() {
        return;
    }
    let (Some(voices), Some(mut kits), Some(assets)) = (voices, kits, assets) else {
        return;
    };
    let listener = listener.pos;
    for r in reactions.read() {
        let Ok((net, transform, mount_child)) = units.get(r.unit) else {
            continue;
        };
        // The mounted redirect (`0x60c480` — `+0xb44 ?: +0xb40`; 0441 fold-back): ALERT reads
        // through the mount-preferred
        // getter — a mounted unit alerts with its MOUNT's voice — while HOSTILE (like DEATH)
        // reads the base row directly and stays the rider's own bark.
        let display = if r.hostile {
            net.display_id
        } else {
            mount_child
                .and_then(|mc| mounts.get(mc.0).ok())
                .and_then(|m| m.display_id)
                .or(net.display_id)
        };
        let Some(voice) = display.and_then(|d| voices.0.for_display(d)) else {
            continue;
        };
        // HOSTILE is the priority route — `0x623a40(0)`, [`play_bark`], which owns both the
        // `AISOUNDDESC` mute and the `[unit+0xb20]` latch.
        if r.hostile {
            play_bark(
                &mut kits,
                &assets,
                &mut out,
                &config,
                listener,
                r.unit,
                transform.translation,
                voice,
                BARK_AGGRO,
            );
            continue;
        }
        // ALERT is the *class* route (`vtable+0x88(8,0)` → `0x623490` → col 13) and is a
        // different function: it neither tests nor stores the voice slot, so it stays a plain
        // play. It does consult the same `AISOUNDDESC` pool (`0x4591f0` from `0x6234cb`) — but
        // there through a virtual whose **CGPlayer twin `0x62f880` omits the gate**, which is why
        // the player exemption lives on this leg and not in [`play_bark`].
        if voice.alert == 0 {
            continue;
        }
        if net.kind != EntityKind::Player && object_sound_playing(&out, r.unit) {
            continue;
        }
        if let Err(e) = play_kit(
            &mut kits,
            &assets,
            &mut out,
            &config,
            listener,
            KitRef::Id(voice.alert),
            Some(transform.translation),
            SoundCategory::Sfx,
        ) {
            warn!("alert vocal (kit {}): {e:#}", voice.alert);
        }
    }
}

/// The pet's voice (`SMSG_PET_ACTION_SOUND` → the net bridge; decision 2039): the two talk
/// selectors, straight onto [`play_bark`] at states 1 and 2.
///
/// The reference's handler `0x6040c0` is four instructions of its own once the guid is resolved:
/// selector `0` → `0x623a40(1)`, selector `1` → `0x623a40(2)`, **anything else → nothing** (there
/// is no default arm — `cmp eax,1; jne` falls to the epilogue). So the third value vmangos could
/// invent would be silent here too, and that is the client's answer, not a gap in ours.
///
/// **This is a warlock-demon surface, and the two ends say so independently.** Only four voices
/// in the shipped file carry these columns — the Imp, Succubus, Doomguard and Voidwalker
/// (`benilla_formats`' `only_the_four_demon_voices_carry_pet_barks`) — and vmangos only *sends*
/// the packet for `GetPetType() == SUMMON_PET`, i.e. those same demons, at a **10% roll** per
/// order (`Unit.cpp:8939`, `PetHandler.cpp:522`, `PetAI.cpp:335`; the other 90% is
/// `SendPetAIReaction`, the growl). A hunter's pet never reaches either half.
fn pet_talk_vocals(
    mut talks: MessageReader<crate::net::PetTalkMessage>,
    units: Query<(&NetEntity, &Transform)>,
    voices: Option<Res<CreatureVoices>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
) {
    if talks.is_empty() {
        return;
    }
    let (Some(voices), Some(mut kits), Some(assets)) = (voices, kits, assets) else {
        return;
    };
    let listener = listener.pos;
    for t in talks.read() {
        let state = match t.talk {
            benilla_protocol::messages::PET_TALK_ORDER => BARK_PET_ORDER,
            benilla_protocol::messages::PET_TALK_ATTACK => BARK_PET_ATTACK,
            _ => continue,
        };
        let Ok((net, transform)) = units.get(t.unit) else {
            continue;
        };
        // `0x6040c0` resolves the guid with `0x468460(ecx = 8)` — the **TYPEMASK_UNIT** bit test,
        // which a player's cumulative mask also carries (a mind-controlled player is somebody's
        // pet too) but a GameObject's does not.
        if !matches!(net.kind, EntityKind::Unit | EntityKind::Player) {
            continue;
        }
        // The pet's OWN voice row, not a mount-preferred one: `0x6040c0` resolves the packet's
        // guid and hands that object straight to `0x623a40`, which reads `[unit+0xb40]` — the
        // base row (the `+0xb44` redirect belongs to `0x60c480`, which is
        // not on this path).
        let Some(voice) = net.display_id.and_then(|d| voices.0.for_display(d)) else {
            continue;
        };
        play_bark(
            &mut kits,
            &assets,
            &mut out,
            &config,
            listener,
            t.unit,
            transform.translation,
            voice,
            state,
        );
    }
}

/// A dismissed pet's parting sound (`SMSG_PET_DISMISS_SOUND` → the net bridge; decision 2039) —
/// `CreatureSoundData` column 29, at a bare world point.
///
/// **Not a bark, and that is the whole shape of it.** `0x604140` never touches `0x623a40`: it
/// resolves the row *fresh by model id* (`CreatureModelData[id].SoundID` → `CreatureSoundData`,
/// no display step and no fallback), reads `[row+0x74]`, and plays it through the plain kit call
/// at the packet's own point — no unit, no voice latch, no `AISOUNDDESC` mute, no attach point.
/// It cannot be otherwise: there is no unit left to hang any of those on.
///
/// That fresh, inlined resolve is also why column 29 was believed dead — a census over the
/// consumers of the *cached* `[unit+0xb40]` row can never reach it.
///
/// **vmangos never sends this packet**, so nothing here fires against our server today. It is
/// built because the reference is the spec, and because it is what makes loading column 29 a
/// mechanism rather than a guess.
fn pet_dismiss_sounds(
    mut dismissals: MessageReader<crate::net::PetDismissSoundMessage>,
    voices: Option<Res<CreatureVoices>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
) {
    if dismissals.is_empty() {
        return;
    }
    let (Some(voices), Some(mut kits), Some(assets)) = (voices, kits, assets) else {
        return;
    };
    let listener = listener.pos;
    for d in dismissals.read() {
        let kit = voices
            .0
            .for_model(d.model_id)
            .map(|v| v.pet_dismiss)
            .unwrap_or(0);
        if kit == 0 {
            continue;
        }
        if let Err(e) = play_kit(
            &mut kits,
            &assets,
            &mut out,
            &config,
            listener,
            KitRef::Id(kit),
            Some(d.pos),
            SoundCategory::Sfx,
        ) {
            warn!("pet dismiss sound (kit {kit}): {e:#}");
        }
    }
}

/// The ambient body loop (`loop_sound`, CreatureSoundData col 23): a creature whose body
/// inherently sounds — an elemental's rumble, a slime's gurgle, a shredder's engine — hums
/// continuously while alive. The client's `0x623800` gate: health > 0, `UNIT_DYNAMIC_FLAGS` DEAD
/// (`0x20`, the feign-death
/// visual) clear, the column nonzero, and a "not already playing" latch — here the tracked
/// channel's own liveness, reconciled every frame. That reconcile shape covers the client's
/// field-delta watchers (death stops it, resurrection restarts it) *and* doubles as the restart
/// trigger the kit player's out-of-range cull documents (walk away → the loop stops; walk back →
/// it re-arms). Despawn teardown is the mixer's source-channel reaper. INTERIM, named: the loop
/// is **forced** regardless of the kit's own 0x200 flag — every col-23 kit in 5875 is authored
/// `*Loop*` yet half omit the flag, so respecting it would silence half the class (`0x461d80`'s
/// flag handling is unpinned; the record covers the reading).
fn creature_body_loops(
    units: Query<(
        Entity,
        &NetEntity,
        &ObjectStore,
        &Transform,
        Option<&MountChild>,
    )>,
    mounts: Query<&NetEntity, With<MountBody>>,
    // The loop kit this system last armed per unit — a mount transition swaps the resolved
    // kit, and the superseded loop must stop (the liveness latch is per (source, kit)).
    mut armed: Local<EntityHashMap<u32>>,
    voices: Option<Res<CreatureVoices>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
) {
    let (Some(voices), Some(mut kits), Some(assets)) = (voices, kits, assets) else {
        return;
    };
    let listener = listener.pos;
    for (entity, net, store, transform, mount_child) in &units {
        if !matches!(net.kind, EntityKind::Unit | EntityKind::Player) {
            continue;
        }
        // The mounted redirect (`0x60c480` — `+0xb44 ?: +0xb40`; 0441 fold-back): the loop
        // column reads the mount-preferred
        // row — a mounted mechanostrider's engine hums; dismounting swaps back to the rider's.
        let display = mount_child
            .and_then(|mc| mounts.get(mc.0).ok())
            .and_then(|m| m.display_id)
            .or(net.display_id);
        let kit = display
            .and_then(|d| voices.0.for_display(d))
            .map(|v| v.loop_sound)
            .unwrap_or(0);
        // `0x623800`'s own gate verbatim (`0x623817` health, `0x62381e` the flag): RAW health ≤ 0
        // — absent = 0, deliberately not `unit_is_dead`'s max-health guard, which would leave a
        // unit whose snapshot has not landed humming — or `UNIT_DYNFLAG_DEAD` set, the feign-death
        // bit the reference re-evaluates this very gate on (`0x60053c`, decision 1022).
        let alive = store.0.unit_health().unwrap_or(0) > 0 && !store.0.unit_reads_dead();
        let desired = if alive { kit } else { 0 };
        // Stop a superseded loop first: death, or a mount transition that changed the row.
        if let Some(&prev) = armed.get(&entity) {
            if prev != desired && source_kit_playing(&out, entity, prev) {
                stop_source_kit(&mut out, entity, prev);
            }
        }
        if desired == 0 {
            armed.remove(&entity);
            continue;
        }
        armed.insert(entity, desired);
        if !source_kit_playing(&out, entity, desired) {
            // Out of range this returns without allocating a channel — the next frame retries,
            // which is exactly the re-arm-on-audible the kit player asks its triggers for.
            if let Err(e) = play_kit_ext(
                &mut kits,
                &assets,
                &mut out,
                &config,
                listener,
                KitRef::Id(desired),
                Some(transform.translation),
                SoundCategory::Sfx,
                PlayExtras {
                    source: Some(entity),
                    force_loop: true,
                    ..default()
                },
            ) {
                warn!("body loop (kit {desired}): {e:#}");
            }
        }
    }
}

/// Route the CreatureSoundData anim tags: fidgets, stand, wing flap/glide. (The `$AH0–3`
/// custom-attack columns are NOT free-standing anim vocals — they are the swing dispatch's
/// natural-weapon impact sound, record-gated and latch-exclusive with the generic weapon
/// impact; [`super::combat`] plays them off [`crate::creature_anim::SwingImpact`], decision
/// 0525.)
/// A mounted unit's fidgets are the MOUNT model's own tags, fired by the mount CHILD entity
/// carrying the mount's display — so they already resolve the mount's voice row, the split-
/// entity shape of the client's mounted redirect (`0x60c480`: `+0xb44 ?: +0xb40` — 0441
/// fold-back).
///
/// **The three tag families gate differently, and the difference is byte-exact** (decision 1401):
///
/// - **`$FD1..$FD4` → fidget 1..4, ungated.** `0x6232c0` → `0x623440` → `0x6230a0` reads
///   `row[+0x38 + 4*(n-1)]` literally and touches no gate at all. A zero slot bails silently, with
///   no fallback to another column.
/// - **`$WNG`/`$WGG` → wing flap/glide, also ungated — but by *constant*, not by construction.**
///   Both take the class-bark route (`0x5fff9c` pushes class 7, `0x5fff78` class 10, into
///   `[vtable+0x88]`), so they *are* rolled — against thresholds of **100**, which the inclusive
///   compare makes a tautology. They sit outside the mute guard `{0,1,2,3,8}` and run on the
///   uncapped bus. Playing them unconditionally is faithful; it is worth knowing that it is
///   faithful for a reason living in a table rather than in the control flow.
/// - **`$FDX` → the stand vocal, gated three times over**, which is what this function had wrong:
///   the local-player skip (`0x6236c2` — you never hear your own), then a **40.6 %** chance roll
///   ([`STAND_CHANCE`]), then a **10 s cooldown on one world-global timestamp**
///   ([`STAND_COOLDOWN`]). Ungated, a crocodile croaks every 4 s forever: `Crocodile.m2` keys
///   `$FDX` at t=0.000 of a 4.000 s **loop-flag** Stand variation carrying 45 % of the variation
///   weight, and a t=0 key re-fires on every wrap. Row 136's stand column is kit 571
///   `BasiliskStand2`, so it is audible — and it is the only model in 5875 that authors `$FDX`
///   *and* reaches a nonzero stand column (censused across all 20 models that reach one of the 19
///   rows carrying a stand kit).
fn creature_anim_vocals(
    mut events: MessageReader<AnimSoundEvent>,
    // GlobalTransform: the mount child's local Transform is the seat-relative ~origin — world
    // position is the only correct read for both parented and top-level sources.
    units: Query<(&NetEntity, &GlobalTransform, Has<SelfPlayer>)>,
    // The class-5 window. A `Local` is exactly one instance, which is the point: the reference's
    // `[0xc4e0e4]` is a single timestamp for the whole world, not one per unit.
    mut stand_window: Local<Option<std::time::Duration>>,
    time: Res<Time>,
    voices: Option<Res<CreatureVoices>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
) {
    if events.is_empty() {
        return;
    }
    let (Some(voices), Some(mut kits), Some(assets)) = (voices, kits, assets) else {
        return;
    };
    let listener = listener.pos;
    let now = time.elapsed();
    for ev in events.read() {
        let Ok((net, transform, is_self)) = units.get(ev.entity) else {
            continue;
        };
        let Some(voice) = net.display_id.and_then(|d| voices.0.for_display(d)) else {
            continue;
        };
        // `$FDX`'s three gates, in the reference's own order. All of them run BEFORE the
        // column-is-zero bail below, and the window is stamped on any *allowed* attempt — so a
        // silent or distance-culled stand vocal burns the ten seconds for every creature in the
        // world, exactly as `0x623290` does.
        if ev.ident == *b"$FDX" {
            if is_self {
                continue; // `0x6236c2`: you never hear your own stand vocal
            }
            if !bark_chance_pass(STAND_CHANCE, kits.roll()) {
                continue;
            }
            if stand_window.is_some_and(|last| now < last + STAND_COOLDOWN) {
                continue;
            }
            *stand_window = Some(now);
        }
        let kit = match &ev.ident {
            b"$FD1" => voice.fidget[0],
            b"$FD2" => voice.fidget[1],
            b"$FD3" => voice.fidget[2],
            b"$FD4" => voice.fidget[3],
            b"$FDX" => voice.stand,
            b"$WNG" => voice.wing_flap,
            b"$WGG" => voice.wing_glide,
            _ => 0,
        };
        if kit == 0 {
            continue;
        }
        if let Err(e) = play_kit(
            &mut kits,
            &assets,
            &mut out,
            &config,
            listener,
            KitRef::Id(kit),
            Some(transform.translation()),
            SoundCategory::Sfx,
        ) {
            warn!("creature vocal (kit {kit}): {e:#}");
        }
    }
}

/// Registration hook for [`super::SoundPlugin`].
/// The **fall-landing wound vocal** — the client-side hard-landing predictor's sound leg
/// (`0x602d00 → call [vtable+0x88] class 2` at `0x602d84`; decision 0412): a landing past the HARD
/// threshold plays the unit's ordinary CreatureSoundData **wound vocal, normal row** (class 2 →
/// column `+0xc` = `injury[0]` — the same row a landed melee hit voices; the crit/crushing rows
/// are unused here). NOT wire-driven: the server's `SMSG_ENVIRONMENTALDAMAGELOG` set plays no
/// sound at all — the grunt is predicted at the landing frame off the client's own fall height.
/// The dust leg of the same predictor lives in `creature_anim::env_damage`; both gate on the
/// shared [`crate::creature_anim::HARD_LANDING_DESCENT`].
fn fall_landing_vocals(
    mut landings: MessageReader<crate::creature_anim::HardLanding>,
    units: Query<(&NetEntity, &Transform)>,
    voices: Option<Res<CreatureVoices>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
) {
    if landings.is_empty() {
        return;
    }
    let (Some(voices), Some(mut kits), Some(assets)) = (voices, kits, assets) else {
        return;
    };
    let listener = listener.pos;
    for l in landings.read() {
        if l.descent <= crate::creature_anim::HARD_LANDING_DESCENT {
            continue;
        }
        let Ok((net, transform)) = units.get(l.entity) else {
            continue;
        };
        let kit = net
            .display_id
            .and_then(|d| voices.0.for_display(d))
            .map(|v| v.injury[0])
            .unwrap_or(0);
        if kit == 0 {
            continue;
        }
        if let Err(e) = play_kit(
            &mut kits,
            &assets,
            &mut out,
            &config,
            listener,
            KitRef::Id(kit),
            Some(transform.translation),
            SoundCategory::Sfx,
        ) {
            warn!("fall-landing vocal (kit {kit}): {e:#}");
        }
    }
}

pub(super) fn plugin(app: &mut App) {
    app.add_systems(Startup, load_creature_voices.after(AssetSet::Open))
        .add_systems(
            Update,
            (
                death_vocals,
                creature_anim_vocals,
                ai_reaction_vocals,
                pet_talk_vocals,
                pet_dismiss_sounds,
                fall_landing_vocals,
                creature_body_loops,
            )
                .in_set(WorldStage::Present),
        );
}

#[cfg(test)]
mod tests {
    use super::super::kit::occupies_voice_slot;
    use super::*;

    /// **The measured report.** Every `SMSG_AI_REACTION` HOSTILE arrival for the director's bear
    /// pet in one traced session (2026-08-17, `RUST_LOG=benilla_app::sound=debug`), in seconds
    /// from the first: 63 of them inside 19 s — three inside a single frame, then a ~1.53 s
    /// metronome, then two ~10 Hz runs. vmangos sends them that fast by design (`Unit::Attack`
    /// fires one on every target acquisition, `Unit::SendPetAIReaction` one on every pet autocast
    /// and every self-picked target — `PetAI::DoAttack` and `PetAI::UpdateAI`'s autocast leg), so
    /// this is the traffic the client is *supposed* to survive.
    const BEAR_BURST_S: [f32; 63] = [
        0.000, 0.000, 0.000, 1.417, 2.950, 4.481, 5.997, 7.530, 9.046, 10.589, 10.670, 10.792,
        12.312, 12.413, 12.546, 12.603, 12.713, 12.813, 12.912, 13.013, 13.112, 13.214, 13.315,
        13.424, 13.528, 13.624, 13.740, 13.829, 13.928, 14.032, 14.137, 14.239, 14.331, 14.449,
        14.545, 14.646, 14.745, 14.847, 16.393, 16.499, 16.684, 16.789, 16.909, 17.013, 17.096,
        17.205, 17.303, 17.411, 17.540, 17.612, 17.711, 17.811, 17.920, 18.032, 18.111, 18.232,
        18.340, 18.418, 18.553, 18.623, 18.728, 18.827, 18.928,
    ];

    /// Kit 478 `BearAggro` has exactly one variation, `mBearAggroA.wav` — **3.166 s** at
    /// 22 050 Hz, measured off the install. Long enough that the burst above stacks up to some
    /// thirty copies of one roar over each other, which is what the director heard as phasing.
    const BEAR_AGGRO_S: f32 = 3.166;

    /// Replay a burst through the `[unit+0xb20]` slot and return the arrivals that actually
    /// sounded. The slot holds at most one live channel; the pump reaps it when the sound stops,
    /// and that reap IS the handle release.
    fn barks_that_sound(arrivals: &[f32], clip: f32) -> Vec<f32> {
        let bear = Entity::from_raw_u32(1).expect("valid entity id");
        let mut slot: Option<(Option<Entity>, Latch, f32)> = None;
        let mut sounded = Vec::new();
        for &t in arrivals {
            if slot.is_some_and(|(_, _, ends)| ends <= t) {
                slot = None;
            }
            if slot.is_some_and(|(src, l, _)| occupies_voice_slot(src, l, bear)) {
                continue; // category 0 is the lowest — a live bark wins, this one is dropped
            }
            slot = Some((Some(bear), Latch::Voice(BARK_AGGRO), t + clip));
            sounded.push(t);
        }
        sounded
    }

    /// **The bug and the fix, on the measured data** (decision 1399). Ungated — what benilla
    /// did — every arrival sounds: 63 overlapping 3.166 s roars. Through the voice slot the same
    /// burst is five barks, no two of them overlapping.
    #[test]
    fn the_voice_slot_collapses_the_measured_bear_burst() {
        assert_eq!(
            BEAR_BURST_S.len(),
            63,
            "ungated, every arrival sounds — the reported symptom",
        );

        let sounded = barks_that_sound(&BEAR_BURST_S, BEAR_AGGRO_S);
        assert_eq!(
            sounded,
            vec![0.0, 4.481, 9.046, 12.312, 16.393],
            "the slot admits a bark only once the last one has finished",
        );
        for pair in sounded.windows(2) {
            assert!(
                pair[1] - pair[0] >= BEAR_AGGRO_S,
                "two barks {pair:?} overlap — the phasing the report describes",
            );
        }
    }

    /// The slot is **per unit**, not global: a second bear beside the first keeps its own voice.
    /// A world-wide throttle would have collapsed the burst too, and would have been wrong.
    #[test]
    fn the_voice_slot_is_per_unit() {
        let (a, b) = (
            Entity::from_raw_u32(1).expect("valid entity id"),
            Entity::from_raw_u32(2).expect("valid entity id"),
        );
        let barking = (Some(a), Latch::Voice(BARK_AGGRO));
        assert!(occupies_voice_slot(barking.0, barking.1, a));
        assert!(!occupies_voice_slot(barking.0, barking.1, b));
    }

    /// **The priority rule, at every boundary the reference's two jumps and one `jle` draw**
    /// (decision 2039). The equal case is the one that matters most: it is what collapses a burst
    /// of identical barks to one, and it is why the aggro flood of 1399 stays collapsed now that
    /// the slot carries a state instead of a bare bool.
    #[test]
    fn a_held_bark_admits_only_a_strictly_higher_state() {
        // A free slot admits everything, including the lowest state.
        for state in [BARK_AGGRO, BARK_PET_ORDER, BARK_PET_ATTACK, BARK_DEATH] {
            assert!(bark_admitted(None, state), "free slot, state {state}");
        }
        // Equal is refused — `jle`, not `jl`.
        for state in [BARK_AGGRO, BARK_PET_ORDER, BARK_PET_ATTACK, BARK_DEATH] {
            assert!(!bark_admitted(Some(state), state), "equal, state {state}");
        }
        // A pet's ATTACK bark cuts through its own aggro flare and its own order bark — the whole
        // reason the state had to become a payload: a hunter's pet draws aggro flares in bursts
        // (1399), and under a bare liveness gate the pet's own voice would never be heard.
        assert!(bark_admitted(Some(BARK_AGGRO), BARK_PET_ORDER));
        assert!(bark_admitted(Some(BARK_AGGRO), BARK_PET_ATTACK));
        assert!(bark_admitted(Some(BARK_PET_ORDER), BARK_PET_ATTACK));
        // ...and nothing cuts through the death cry, which is the table's maximum.
        assert!(bark_admitted(Some(BARK_PET_ATTACK), BARK_DEATH));
        for state in [BARK_AGGRO, BARK_PET_ORDER, BARK_PET_ATTACK] {
            assert!(
                !bark_admitted(Some(BARK_DEATH), state),
                "over death: {state}"
            );
        }
        // The order bark does NOT interrupt an attack bark — the two pet states are ordered, and
        // the wire sends them close together.
        assert!(!bark_admitted(Some(BARK_PET_ATTACK), BARK_PET_ORDER));
    }

    /// The five-arm jump table, column for column — and its two silences.
    #[test]
    fn the_bark_table_maps_each_state_to_its_column() {
        let voice = benilla_formats::CreatureVoice {
            exertion: [0; 2],
            injury: [0; 3],
            death: 314,
            stun: 0,
            stand: 0,
            footstep_class: 0,
            aggro: 694,
            wing_flap: 0,
            wing_glide: 0,
            alert: 1107,
            fidget: [0; 4],
            custom_attack: [0; 4],
            loop_sound: 0,
            impact_type: 0,
            jump_start: 0,
            jump_end: 0,
            pet_attack: 9097,
            pet_order: 9098,
            pet_dismiss: 9096,
        };
        assert_eq!(bark_kit(&voice, BARK_AGGRO), 694);
        assert_eq!(bark_kit(&voice, BARK_PET_ORDER), 9098);
        assert_eq!(bark_kit(&voice, BARK_PET_ATTACK), 9097);
        assert_eq!(bark_kit(&voice, BARK_DEATH), 314);
        // State 3 is a real arm of the table that reads no column, and everything past 4 is the
        // range check's default. Both play nothing — and neither is `alert` (col 13) nor
        // `pet_dismiss` (col 29): those two reach their kits through entirely different
        // functions, off this table altogether.
        assert_eq!(bark_kit(&voice, 3), 0);
        assert_eq!(bark_kit(&voice, 5), 0);
        assert_eq!(bark_kit(&voice, 255), 0);
    }
}
