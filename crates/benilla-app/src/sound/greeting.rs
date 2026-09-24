//! NPC vocal lines — the two systems an NPC greets/farewells with, corrected against the
//! director's observations and the reference.
//!
//! Both systems share only the **data chain** (`NetEntity.display_id` →
//! `CreatureDisplayInfo.field[11]` NPCSoundID → `NPCSounds {hello +0x4, goodbye +0x8, pissed +0xc}`
//! → `SoundEntries` kit) and the **per-unit channel latch** (`[unit+0xb1c]`: a unit refuses a new
//! line while its previous one still sounds; the played channel is tagged with the unit entity via
//! [`kit::source_playing`], and the pump's reap-on-stop is the release). Playback is 3D at the NPC.
//!
//! ## System 1 — the SELECT greeting (`0x60c270`, left-click)
//!
//! Left-clicking a unit ([`crate::target::select_on_click`], the client's `0x4925d0` which fires
//! *before* `SetTarget`) plays the **variation-cycling** welcome ([`NpcGreetingRequest`]). A global
//! sequence counter per NPC ([`GreetSeq`], the client's `[0xc4d970]`, reset when the greeted NPC
//! changes) steps `0x623910`'s law ([`greet_line`]): **5 HELLO takes** (each a free-pick within the
//! hello kit, so they vary) → **the PISSED kit's variations in order** → **wrap + reset**. This is
//! "left-click greets, repeat left-clicks cycle, eventually the annoyed line."
//!
//! ## System 2 — the interaction-WINDOW hello/goodbye (`0x60c3b0`, right-click)
//!
//! Opening any interaction window (gossip/vendor/trainer/bank/mail/trade/quest — each class's
//! `SetActiveNPC 0x4e1fa0`-style method, `0x4930d0` with **14 call sites, none a mouseover**) plays
//! the NPC's HELLO **once, non-cycling, `variation = -1`** (`0x60c464`: FMOD free-picks a take — so
//! it *sounds* different from the cycling select greeting, but it is the **same** `+0x4` slot, not a
//! separate "vendor" line). The window **closing to nothing** plays GOODBYE (`+0x8`, `welcome=0`).
//! A **swap** — opening NPC B's window while A's is open — plays B's hello and **suppresses A's
//! goodbye** (`0x4930fd`: the internal clear gets `dl = (new_guid != 0) = 1`). So goodbye is gated
//! on "the dialog is closing to *nothing*," NOT on the target changing — the v1 "goodbye ≈
//! target-clear" reading is refuted. benilla tracks the **active interaction NPC** as the union of
//! its shipped window sessions ([`MerchantOpen`]/[`GossipState`]/[`QuestGiver`]/[`TrainerOpen`]);
//! despawn resolves nothing, the client's suppressed teardown clear. The trainer belongs in that
//! union like every other interaction window: gossip→trainer on the same NPC is a *swap*, not a
//! close-to-nothing, so its goodbye is suppressed — the trainer's absence from the union is why the
//! 0237 arc briefly mis-played a goodbye on that transition.

use bevy::prelude::*;

use benilla_formats::NpcGreetingCatalog;
use benilla_protocol::EntityKind;

use crate::net::{GuidIndex, NetEntity, ObjectStore};
use crate::ui_gossip::GossipState;
use crate::ui_merchant::MerchantOpen;
use crate::ui_quest::QuestGiver;
use crate::ui_trainer::TrainerOpen;
use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_world::schedule::WorldStage;
use benilla_world::view::WorldCamera;

use super::kit::{self, play_kit_ext, KitRef, Latch, PlayExtras, SoundCategory, SoundKits};
use super::{SoundConfig, SoundOutput};

/// The number of HELLO takes the select greeter plays before it steps into the pissed kit
/// (`0x62394a: cmp ebx,5; jge …`).
const HELLO_TAKES: usize = 5;

/// Request to play an NPC's **select** greeting (System 1, the variation-cycling path) — written by
/// [`crate::target::select_on_click`] on the left-click select gesture (`0x60c270`, "before
/// `SetTarget`"; director-confirmed: left-click greets and repeat left-clicks cycle).
#[derive(Message, Clone, Copy)]
pub(crate) struct NpcGreetingRequest {
    pub(crate) npc: Entity,
}

/// The display→greeting catalog (`CreatureDisplayInfo.NPCSoundID` → `NPCSounds`).
#[derive(Resource)]
struct NpcGreetings(NpcGreetingCatalog);

/// System 1's per-NPC variation counter — the client's `[0xc4d970]` (the sequence index) keyed by
/// `[0xc4d908]` (the tracked NPC; the counter resets when it changes). Advanced only on a play that
/// actually happened (the latch gate sits before the counter, `0x60c28c`).
#[derive(Default)]
struct GreetSeq {
    tracked: Option<Entity>,
    seq: usize,
}

/// Which line + variation a select play at sequence `seq` produces (`0x623910`). `variation = None`
/// is the client's `-1` free-pick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GreetLine {
    /// HELLO (`+0x4`), free-pick take.
    Hello,
    /// PISSED (`+0xc`), take `variation`.
    Pissed(usize),
}

/// The select-greeter sequence law (`0x623910`, byte-exact): the first [`HELLO_TAKES`] plays are
/// HELLO free-picks; then the pissed kit's `pissed_variations` takes in order; then it **wraps** —
/// a final HELLO free-pick that resets the counter to 0. Returns the line to play now and the
/// counter value to commit for next time.
fn greet_line(seq: usize, pissed_variations: usize) -> (GreetLine, usize) {
    if seq < HELLO_TAKES {
        (GreetLine::Hello, seq + 1)
    } else {
        let p = seq - HELLO_TAKES;
        if p < pissed_variations {
            (GreetLine::Pissed(p), seq + 1)
        } else {
            // edi >= count: HELLO, variation -1, return 0 — the wrap (`0x62…`, counter reset).
            (GreetLine::Hello, 0)
        }
    }
}

/// The vocal an active-interaction-NPC transition produces (System 2, the client's `SetActiveNPC`
/// semantics): opening or swapping to a new NPC plays its **hello** (and suppresses any prior
/// goodbye); closing to *nothing* plays the old NPC's **goodbye**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowVocal {
    Hello(u64),
    Goodbye(u64),
    None,
}

/// The active interaction NPC: the union of every shipped window session that shares the left panel
/// slot (they are mutually exclusive after the OnHide→CloseX reconcile — decision 0095). System 2
/// diffs THIS across frames, so **every** NPC-bound window must be in it — a window left out makes a
/// swap *to* it look like a close-to-nothing and spuriously plays the prior window's goodbye. That
/// omission is exactly what mis-played a goodbye on gossip→trainer until the trainer joined the union
/// (decision 0237); a new window (bank, mail, …) must be added here too.
fn active_interaction_npc(
    merchant: &MerchantOpen,
    gossip: &GossipState,
    quest: &QuestGiver,
    trainer: &TrainerOpen,
) -> Option<u64> {
    merchant
        .vendor
        .or(gossip.npc)
        .or(quest.npc)
        .or(trainer.trainer)
}

/// System 2's transition law. `prev`/`active` are the previously- and currently-active interaction
/// NPC guids (the union of the open window sessions). A swap (`Some(a) → Some(b)`) is a hello for
/// `b` with **no** goodbye for `a` — the byte-verified suppression (`dl = new_guid != 0`).
fn window_vocal(prev: Option<u64>, active: Option<u64>) -> WindowVocal {
    match (prev, active) {
        (p, a) if p == a => WindowVocal::None,
        // open or swap → hello for the new NPC (prior goodbye suppressed).
        (_, Some(new)) => WindowVocal::Hello(new),
        // closing to nothing → the old NPC's goodbye.
        (Some(old), None) => WindowVocal::Goodbye(old),
        (None, None) => WindowVocal::None,
    }
}

fn load_npc_greetings(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_npc_greeting_catalog(&mut chain)
    };
    match loaded {
        Ok(cat) => {
            info!("sound: {} NPC greeting rows", cat.len());
            commands.insert_resource(NpcGreetings(cat));
        }
        Err(e) => warn!("sound: NPC greetings failed to load: {e:#}"),
    }
}

/// The shared preconditions + data lookup: a non-player unit (type-mask bit 3 set, bit 4 clear),
/// alive, whose display carries an NPCSoundID row. Returns the greeting row + world position.
fn resolve(
    npc: Entity,
    units: &Query<(&NetEntity, &Transform, Option<&ObjectStore>)>,
    greetings: &NpcGreetings,
) -> Option<(benilla_formats::NpcGreeting, Vec3)> {
    let (net, transform, store) = units.get(npc).ok()?;
    if !matches!(net.kind, EntityKind::Unit) {
        return None;
    }
    if store.is_some_and(|s| s.0.unit_is_dead()) {
        return None;
    }
    let row = *greetings.0.for_display(net.display_id?)?;
    Some((row, transform.translation))
}

/// Play one line for `npc` at kit `kit_id` (0 = no such line → nothing), variation `variant`
/// (`None` = free-pick), latch-gated on the unit's channel (`0x60c28c`) and tagged with the unit.
fn play_line(
    npc: Entity,
    kit_id: u32,
    variant: Option<usize>,
    pos: Vec3,
    listener: Vec3,
    kits: &mut SoundKits,
    assets: &WorldAssets,
    out: &mut SoundOutput,
    config: &SoundConfig,
) {
    if kit_id == 0 || kit::source_playing(out, npc) {
        return;
    }
    if let Err(e) = play_kit_ext(
        kits,
        assets,
        out,
        config,
        listener,
        KitRef::Id(kit_id),
        Some(pos),
        SoundCategory::Sfx,
        PlayExtras {
            variant,
            source: Some(npc),
            // The one play that takes `[unit+0xb1c]`: while this channel lives, the NPC refuses
            // another hello. Decision 1399's marker — before it, every source-tagged channel on
            // the unit answered this question and a body loop could mute the greeting.
            latch: Latch::Greeting,
            ..default()
        },
    ) {
        warn!("npc vocal (kit {kit_id}): {e:#}");
    }
}

/// System 1 — the select greeting: drain [`NpcGreetingRequest`]s and play the cycling welcome.
fn play_select_greetings(
    mut reqs: MessageReader<NpcGreetingRequest>,
    mut seq: Local<GreetSeq>,
    units: Query<(&NetEntity, &Transform, Option<&ObjectStore>)>,
    greetings: Option<Res<NpcGreetings>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    cam: Query<&Transform, (With<WorldCamera>, Without<NetEntity>)>,
) {
    if reqs.is_empty() {
        return;
    }
    let (Some(greetings), Some(mut kits), Some(assets)) = (greetings, kits, assets) else {
        reqs.clear();
        return;
    };
    let listener = cam.single().map(|t| t.translation).unwrap_or(Vec3::ZERO);

    for req in reqs.read() {
        let Some((row, pos)) = resolve(req.npc, &units, &greetings) else {
            continue;
        };
        // A different NPC resets the sequence counter (`0x60c326`).
        if seq.tracked != Some(req.npc) {
            *seq = GreetSeq {
                tracked: Some(req.npc),
                seq: 0,
            };
        }
        // The latch gate sits before the counter — a re-click while the line still sounds neither
        // plays nor advances (`0x60c28c`).
        if kit::source_playing(&out, req.npc) {
            continue;
        }
        let (line, next) = greet_line(seq.seq, kits.variations(row.pissed));
        let (kit_id, variant) = match line {
            GreetLine::Hello => (row.hello, None),
            GreetLine::Pissed(p) => (row.pissed, Some(p)),
        };
        if kit_id == 0 {
            continue;
        }
        // Play, and commit the next counter only if the play actually started (`0x60c398`); the
        // latch gate above guarantees the channel was idle, so a live channel now = success.
        play_line(
            req.npc, kit_id, variant, pos, listener, &mut kits, &assets, &mut out, &config,
        );
        if kit::source_playing(&out, req.npc) {
            seq.seq = next;
        }
    }
}

/// System 2 — the interaction-window hello/goodbye: diff the active interaction NPC (the union of
/// the open window sessions) and play hello on open/swap, goodbye on close-to-nothing.
fn play_window_greetings(
    merchant: Res<MerchantOpen>,
    gossip: Res<GossipState>,
    quest: Res<QuestGiver>,
    trainer: Res<TrainerOpen>,
    index: Res<GuidIndex>,
    mut prev_active: Local<Option<u64>>,
    units: Query<(&NetEntity, &Transform, Option<&ObjectStore>)>,
    greetings: Option<Res<NpcGreetings>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    cam: Query<&Transform, (With<WorldCamera>, Without<NetEntity>)>,
) {
    let active = active_interaction_npc(&merchant, &gossip, &quest, &trainer);
    let vocal = window_vocal(*prev_active, active);
    if matches!(vocal, WindowVocal::None) {
        return;
    }
    *prev_active = active;

    let (Some(greetings), Some(mut kits), Some(assets)) = (greetings, kits, assets) else {
        return;
    };
    let listener = cam.single().map(|t| t.translation).unwrap_or(Vec3::ZERO);

    // guid → entity via the net index; a despawned NPC resolves nothing (the suppressed teardown
    // clear, `0x605950`).
    let (guid, hello) = match vocal {
        WindowVocal::Hello(g) => (g, true),
        WindowVocal::Goodbye(g) => (g, false),
        WindowVocal::None => return,
    };
    let Some(&npc) = index.0.get(&guid) else {
        return;
    };
    let Some((row, pos)) = resolve(npc, &units, &greetings) else {
        return;
    };
    let kit_id = if hello { row.hello } else { row.goodbye };
    // Both window lines free-pick (`variation = -1`), single, latch-gated.
    play_line(
        npc, kit_id, None, pos, listener, &mut kits, &assets, &mut out, &config,
    );
}

/// Unit teardown force-stops its live greeting channel (`0x5fbb6c`: Stop + clear the handle) —
/// a despawning NPC doesn't keep talking from where it stood.
fn stop_on_despawn(mut removed: RemovedComponents<NetEntity>, mut out: NonSendMut<SoundOutput>) {
    for entity in removed.read() {
        kit::stop_source(&mut out, entity);
    }
}

/// Registration hook for [`super::SoundPlugin`].
pub(super) fn plugin(app: &mut App) {
    app.add_message::<NpcGreetingRequest>()
        .add_systems(Startup, load_npc_greetings.after(AssetSet::Open))
        .add_systems(
            Update,
            (
                play_select_greetings,
                play_window_greetings,
                stop_on_despawn,
            )
                .in_set(WorldStage::Present),
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The select sequence law (`0x623910`): 5 HELLO free-picks, then the pissed kit's takes in
    /// order, then a wrapping HELLO that resets the counter to 0.
    #[test]
    fn select_sequence_cycles_5_hello_then_pissed_then_wraps() {
        // 5 HELLO takes (seq 0..5), each free-pick, counter advancing.
        for s in 0..5 {
            assert_eq!(greet_line(s, 3), (GreetLine::Hello, s + 1));
        }
        // Then the pissed kit's 3 variations in order.
        assert_eq!(greet_line(5, 3), (GreetLine::Pissed(0), 6));
        assert_eq!(greet_line(6, 3), (GreetLine::Pissed(1), 7));
        assert_eq!(greet_line(7, 3), (GreetLine::Pissed(2), 8));
        // Exhausted (5 + 3): a wrapping HELLO that resets the counter to 0.
        assert_eq!(greet_line(8, 3), (GreetLine::Hello, 0));
        // A pissed-less NPC (0 variations) wraps straight after the 5 hellos.
        assert_eq!(greet_line(5, 0), (GreetLine::Hello, 0));
    }

    /// The window transition law (`SetActiveNPC`): open → hello, close-to-nothing → goodbye, and a
    /// swap → hello for the new NPC with the old goodbye SUPPRESSED.
    #[test]
    fn window_vocal_opens_closes_and_suppresses_swap_goodbye() {
        assert_eq!(window_vocal(None, Some(0x1)), WindowVocal::Hello(0x1)); // open
        assert_eq!(window_vocal(Some(0x1), None), WindowVocal::Goodbye(0x1)); // close to nothing
        assert_eq!(window_vocal(Some(0x1), Some(0x2)), WindowVocal::Hello(0x2)); // swap: no goodbye
        assert_eq!(window_vocal(Some(0x1), Some(0x1)), WindowVocal::None); // no change
        assert_eq!(window_vocal(None, None), WindowVocal::None);
    }

    /// The trainer must count toward the active interaction NPC, so a gossip→trainer handoff on ONE
    /// NPC is a swap (silent), not a close-to-nothing (a spurious goodbye). This reproduces the
    /// director-reported 0237 bug at the union+law level: before the fix the trainer was absent from
    /// the union, so `active` went `Some(npc) → None` and window_vocal spoke the goodbye.
    // Each session carries private cache fields, so it is built via `default()` + a field set rather
    // than a struct literal — the form `field_reassign_with_default` would otherwise flag.
    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn gossip_to_trainer_on_one_npc_is_a_silent_swap() {
        let npc = 0x1234_u64;
        let (empty_merchant, empty_gossip, empty_quest, empty_trainer) = (
            MerchantOpen::default(),
            GossipState::default(),
            QuestGiver::default(),
            TrainerOpen::default(),
        );

        // Nothing open → no active NPC.
        assert_eq!(
            active_interaction_npc(&empty_merchant, &empty_gossip, &empty_quest, &empty_trainer),
            None,
        );

        // Frame 1: the gossip menu is open on `npc`.
        let mut gossip = GossipState::default();
        gossip.npc = Some(npc);
        let before = active_interaction_npc(&empty_merchant, &gossip, &empty_quest, &empty_trainer);

        // Frame 2: the gossip closed and the trainer opened on the SAME npc (the handoff).
        let mut trainer = TrainerOpen::default();
        trainer.trainer = Some(npc);
        let after = active_interaction_npc(&empty_merchant, &empty_gossip, &empty_quest, &trainer);

        assert_eq!(before, Some(npc), "gossip drove the active NPC");
        assert_eq!(after, Some(npc), "the trainer keeps the same NPC active");
        assert_eq!(
            window_vocal(before, after),
            WindowVocal::None,
            "gossip→trainer on one NPC is a silent swap — no goodbye",
        );
    }
}
