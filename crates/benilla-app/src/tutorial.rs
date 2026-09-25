//! The tutorial system (the reference's `[0x4b5150, 0x4b5a60)`): the two
//! bit banks, the fire-once trigger, the acknowledge-and-send setter, the timers, the popup
//! sound, and the trigger sites the app can produce — everything behind the stock
//! `TutorialFrame.xml`.
//!
//! ## Two banks, not one (`0xb711b8`, `0xb711e4`)
//!
//! Both are filled byte for byte from the same `SMSG_TUTORIAL_FLAGS`, and until it lands no
//! tutorial can fire. **Bank A** is the fire-once bank: [`Tutorials::trigger`] tests it, sets it,
//! and raises `TUTORIAL_TRIGGER(id + 1)` — never sending. **Bank B** is the acknowledged bank:
//! [`Tutorials::acknowledge`] (Lua's `FlagTutorial` and the C++ auto-acknowledge sites) tests it,
//! sets the bit in BOTH banks, cancels the id's pending timer, and sends `CMSG_TUTORIAL_FLAG`.
//! `ClearTutorials`/`ResetTutorials` write both banks and send. So A ⊇ B, and doing the thing
//! (moving, chatting, adding a friend…) suppresses the tutorial about it, account-wide.
//!
//! ## The trigger (`0x4b5390`)
//!
//! `TriggerTutorial(id, delayMs)`: no bank → silent; bank-A bit set → silent; else the bit is
//! set and, with a zero delay, the `TutorialPopup` cue plays and the event fires now; with any
//! other delay (unsigned — the 10 s Targeting popup is the one site) a timer holds it. The
//! reference bounds-checks nothing; ours refuses an id past the bank as a no-op.
//!
//! ## The sites (the callers of `0x4b5390` and `0x4b54c0`)
//!
//! Fifty-one trigger sites and seven acknowledge sites exist in the reference. The ones this app
//! can produce are wired — the self-descriptor edges here, the packets and drains at their own
//! seams through [`TutorialEvent`] — and the record names the rest with their conditions.

use std::time::{Duration, Instant};

use bevy::prelude::*;

use benilla_ui::script::{ScriptValue, UiScript};

use crate::char_select::ClientState;
use crate::net::{ClientCommand, EnteredWorldMessage, NetCommands, ObjectStore, SelfPlayer};
use crate::player::Player;
use crate::ui_script::{UiFeed, UiInput};

/// The popup cue `0x4b5390` plays on both legs (`0x846b68`), gated on `MasterSoundEffects` at
/// the mixer like every SFX kit.
const TUTORIAL_POPUP_SOUND: &str = "TutorialPopup";

/// The internal (0-based) ids of the sites this app produces, named by their published title.
pub(crate) mod id {
    pub(crate) const QUESTGIVERS: u32 = 0x00;
    pub(crate) const MOVEMENT: u32 = 0x01;
    pub(crate) const CAMERAS: u32 = 0x02;
    pub(crate) const TARGETING: u32 = 0x03;
    pub(crate) const LOOTING: u32 = 0x06;
    pub(crate) const BACKPACK: u32 = 0x07;
    pub(crate) const TRAINERS: u32 = 0x0d;
    pub(crate) const GROUPING: u32 = 0x11;
    pub(crate) const VENDORS: u32 = 0x13;
    pub(crate) const QUEST_LOG: u32 = 0x14;
    pub(crate) const FRIENDS: u32 = 0x15;
    pub(crate) const CHATTING: u32 = 0x16;
    pub(crate) const EQUIPPABLE_ITEMS: u32 = 0x17;
    pub(crate) const DEATH: u32 = 0x18;
    pub(crate) const RESTED: u32 = 0x19;
    pub(crate) const FATIGUE: u32 = 0x1a;
    pub(crate) const SWIMMING: u32 = 0x1b;
    pub(crate) const BREATH: u32 = 0x1c;
    pub(crate) const RESTING: u32 = 0x1d;
    pub(crate) const HEARTHSTONES: u32 = 0x1e;
    pub(crate) const PVP_COMBAT: u32 = 0x1f;
    pub(crate) const TRAVEL: u32 = 0x22;
    pub(crate) const WELCOME: u32 = 0x29;
    pub(crate) const RANGED_WEAPONS: u32 = 0x2b;
    pub(crate) const RAID_GROUPS: u32 = 0x2d;
    pub(crate) const MEETING_STONES: u32 = 0x2e;
    pub(crate) const BATTLEGROUND_QUEUE: u32 = 0x2f;
    pub(crate) const PORT_TO_BATTLEGROUND: u32 = 0x30;
    pub(crate) const KEYRINGS: u32 = 0x31;
}

/// The level-gated arm of the `SMSG_LEVELUP_INFO` handler (`0x5e38c0`): in the handler's own order,
/// each fires when the new level is at least its threshold.
const LEVEL_TRIGGERS: [(u32, u32); 8] = [
    (3, 0x16),  // Chatting
    (4, 0x0d),  // Trainers
    (4, 0x02),  // Cameras
    (5, 0x26),  // Groups
    (7, 0x15),  // Friends
    (7, 0x25),  // Professions
    (8, 0x20),  // Jumping
    (10, 0x0c), // Learning Talents
];

/// The Targeting popup's delay at its one site (`0x514a8d mov edx,0x2710`).
pub(crate) const TARGETING_DELAY_MS: u32 = 10_000;

/// The Movement popup's silence window (`0x482ff7`: elapsed − stamp − 90 000 ≥ 0) — inferred as
/// ninety seconds after world enter with no movement input (the stamp getter `0x5143e0` is
/// read, its meaning is not).
const MOVEMENT_SILENCE: Duration = Duration::from_millis(90_000);

/// A site's ask of the tutorial system — written from wherever the reference's site lives.
#[derive(Message, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TutorialEvent {
    /// `TriggerTutorial(id, delayMs)`: the fire-once leg.
    Trigger { id: u32, delay_ms: u32 },
    /// `SetTutorialFlag(id)` (`0x4b54c0`): the acknowledge-and-send leg.
    Acknowledge { id: u32 },
}

impl TutorialEvent {
    pub(crate) const fn trigger(id: u32) -> Self {
        Self::Trigger { id, delay_ms: 0 }
    }
}

/// A send the drains owe the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TutorialSend {
    Flag(u32),
    Clear,
    Reset,
}

/// A bank: `bit = bytes[id >> 3] & (1 << (id & 7))` — the client's `word = id >> 5` on
/// little-endian dwords, byte-wise.
fn bank_bit(bank: &[u8], id: u32) -> Option<bool> {
    bank.get((id >> 3) as usize)
        .map(|b| b & (1 << (id & 7)) != 0)
}

fn set_bank_bit(bank: &mut [u8], id: u32) {
    if let Some(b) = bank.get_mut((id >> 3) as usize) {
        *b |= 1 << (id & 7);
    }
}

/// The two banks, the timers, and what the frame owes the VM and the wire.
#[derive(Resource, Default)]
pub(crate) struct Tutorials {
    /// Bank A (`0xb711b8`…): fire-once. `None` until the packet lands.
    fire_once: Option<Vec<u8>>,
    /// Bank B (`0xb711e4`…): acknowledged.
    acknowledged: Option<Vec<u8>>,
    /// The delayed triggers (`TUTORIALTIMER` nodes): `(id, due)`.
    timers: Vec<(u32, Instant)>,
    /// Published ids (`id + 1`) whose event and cue are owed this frame.
    fired: Vec<u32>,
    sends: Vec<TutorialSend>,
    /// Item pushes for us since the last feed (`0x491a60`'s sites): `(entry, bag, slot)`.
    pushes: Vec<(u32, u8, u32)>,
}

impl Tutorials {
    /// `SMSG_TUTORIAL_FLAGS`: both banks copied from the payload.
    pub(crate) fn apply_flags(&mut self, bytes: &[u8]) {
        self.fire_once = Some(bytes.to_vec());
        self.acknowledged = Some(bytes.to_vec());
    }

    /// The bring-up `0x4b5330`: both banks zeroed if present (the bit count stays).
    fn world_enter(&mut self) {
        for bank in [&mut self.fire_once, &mut self.acknowledged]
            .into_iter()
            .flatten()
        {
            bank.iter_mut().for_each(|b| *b = 0);
        }
    }

    /// The teardown `0x4b5380`: every pending timer flushed.
    fn world_leave(&mut self) {
        self.timers.clear();
    }

    /// `TriggerTutorial(id, delayMs)` (`0x4b5390`).
    pub(crate) fn trigger(&mut self, id: u32, delay_ms: u32, now: Instant) {
        let Some(bank) = self.fire_once.as_mut() else {
            return; // no bank yet: silent
        };
        match bank_bit(bank, id) {
            Some(false) => set_bank_bit(bank, id),
            _ => return, // already set — or past the bank, which the reference would overrun
        }
        if delay_ms == 0 {
            self.fired.push(id + 1);
        } else {
            self.timers
                .push((id, now + Duration::from_millis(u64::from(delay_ms))));
        }
    }

    /// `SetTutorialFlag(id)` (`0x4b54c0`): bank B's bit gates; both banks set; the id's timer
    /// cancelled; the 0-based id sent. The reference dereferences an absent bank B; ours treats
    /// that as a no-op.
    pub(crate) fn acknowledge(&mut self, id: u32) {
        let Some(bank_b) = self.acknowledged.as_mut() else {
            return;
        };
        if bank_bit(bank_b, id) != Some(false) {
            return;
        }
        self.timers.retain(|(t, _)| *t != id);
        set_bank_bit(bank_b, id);
        if let Some(bank_a) = self.fire_once.as_mut() {
            set_bank_bit(bank_a, id);
        }
        self.sends.push(TutorialSend::Flag(id));
    }

    /// `ClearTutorials()`: the timers flushed, every bit of both banks set, the empty clear sent.
    fn clear(&mut self) {
        self.timers.clear();
        for bank in [&mut self.fire_once, &mut self.acknowledged]
            .into_iter()
            .flatten()
        {
            bank.iter_mut().for_each(|b| *b = 0xFF);
        }
        self.sends.push(TutorialSend::Clear);
    }

    /// `ResetTutorials()`: every bit of both banks cleared — the timers NOT flushed — the empty
    /// reset sent.
    fn reset(&mut self) {
        for bank in [&mut self.fire_once, &mut self.acknowledged]
            .into_iter()
            .flatten()
        {
            bank.iter_mut().for_each(|b| *b = 0);
        }
        self.sends.push(TutorialSend::Reset);
    }

    /// `ProcessTutorialTimers`: every due node fires the same cue and event.
    fn tick(&mut self, now: Instant) {
        let mut due = Vec::new();
        self.timers.retain(|&(id, at)| {
            if at <= now {
                due.push(id + 1);
                false
            } else {
                true
            }
        });
        self.fired.extend(due);
    }

    /// `SMSG_ITEM_PUSH_RESULT` for one of ours — the item-received handler's five sites, resolved
    /// on the next feed with the item's template.
    pub(crate) fn item_received(&mut self, entry: u32, bag: u8, slot: u32) {
        self.pushes.push((entry, bag, slot));
    }

    #[cfg(test)]
    fn bits(&self) -> (Option<&[u8]>, Option<&[u8]>) {
        (self.fire_once.as_deref(), self.acknowledged.as_deref())
    }
}

/// The self descriptor's last-seen fields, for the edge-triggered `0x4b5390` sites: the level-up
/// arm, the ghost flag, the resting flag, the PvP flag, the rest state.
#[derive(Default)]
struct SelfWatch {
    level: Option<u32>,
    player_flags: Option<u32>,
    unit_flags: Option<u32>,
    rest_state: Option<u8>,
    swimming: Option<bool>,
}

const PLAYER_FLAGS_GHOST: u32 = 0x10;
const PLAYER_FLAGS_RESTING: u32 = 0x20;
const UNIT_FLAG_PVP_ATTACKABLE: u32 = 0x1000;

/// The hearthstone's item entry (`0x1b24`).
const HEARTHSTONE: u32 = 6948;
const INVTYPE_RANGED: u32 = 15;
const CLASS_HUNTER: u8 = 3;
/// The keyring bag and its slot band (`edi == 0xff`, slot ∈ [0x51, 0x70]).
const KEYRING_BAG: u8 = 0xff;
const KEYRING_SLOTS: std::ops::RangeInclusive<u32> = 0x51..=0x70;

/// Before the script tick: the sites' asks, the descriptor edges, the timers, the cue and the
/// event, and the acknowledged bank's push to whichever VM is live.
fn feed_tutorials(
    script: Option<NonSendMut<UiScript>>,
    mut tutorials: ResMut<Tutorials>,
    mut asks: MessageReader<TutorialEvent>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    items: Res<crate::items::Items>,
    mut sounds: ResMut<crate::sound::MessageSounds>,
    mut told: Local<crate::ui_script::VmMemo<Option<Vec<u8>>>>,
) {
    let now = Instant::now();
    for ask in asks.read() {
        match *ask {
            TutorialEvent::Trigger { id, delay_ms } => tutorials.trigger(id, delay_ms, now),
            TutorialEvent::Acknowledge { id } => tutorials.acknowledge(id),
        }
    }

    // The item-received handler's sites, over the item's template: Backpack unconditionally;
    // Equippable Items for an inventory type with an equip slot (inferred as any non-zero type —
    // the reference's 23-bit mask (`0x809200`) is not decoded bit by bit); Hearthstones by entry;
    // Ranged Weapons for a ranged type on a non-hunter; Keyrings by the slot band.
    let pushes = std::mem::take(&mut tutorials.pushes);
    let class = self_q.single().ok().and_then(|s| s.0.unit_class());
    for (entry, bag, slot) in pushes {
        tutorials.trigger(id::BACKPACK, 0, now);
        if let Some(info) = items.template_cached(entry) {
            if info.inventory_type != 0 {
                tutorials.trigger(id::EQUIPPABLE_ITEMS, 0, now);
            }
            if info.inventory_type == INVTYPE_RANGED && class != Some(CLASS_HUNTER) {
                tutorials.trigger(id::RANGED_WEAPONS, 0, now);
            }
        }
        if entry == HEARTHSTONE {
            tutorials.trigger(id::HEARTHSTONES, 0, now);
        }
        if bag == KEYRING_BAG && KEYRING_SLOTS.contains(&slot) {
            tutorials.trigger(id::KEYRINGS, 0, now);
        }
    }

    tutorials.tick(now);

    let Some(mut script) = script else {
        return;
    };
    for published in std::mem::take(&mut tutorials.fired) {
        sounds.push_cue(TUTORIAL_POPUP_SOUND);
        script.fire_event(
            "TUTORIAL_TRIGGER",
            vec![ScriptValue::Int(i64::from(published))],
        );
    }
    // **The bank is owed to every VM, not to the process**. What the acknowledged
    // bank holds is the app's; whether a VM has been *told* it is that VM's, so the memory sits
    // behind a [`crate::ui_script::VmMemo`] and expires with the session it was written against
    // (1290/1291 — a `/reload` is a logout and a login back to back). A plain "changed since the
    // last push" flag is the same mistake 2113 names: it answers "did the bank move?" when the
    // question is "does THIS VM know it?", and after a reload the answer was no for the rest of
    // the session.
    let told = told.get(&script);
    if *told != tutorials.acknowledged {
        told.clone_from(&tutorials.acknowledged);
        script.set_tutorial_bank(tutorials.acknowledged.clone());
    }
}

/// The self descriptor's `0x4b5390` edge sites: the `SMSG_LEVELUP_INFO` arm on the level rising,
/// the ghost and resting flags, the PvP flag, the rested table's one live row, and the mover
/// entering water — a change each, never the login descriptor. The memory is per world session:
/// the bring-up resets it, so a re-login's first descriptor arms silently like the first one.
fn watch_self(
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    player: Res<Player>,
    mut cascade: ResMut<WorldEnterCascade>,
    mut watch: Local<SelfWatch>,
    mut asks: MessageWriter<TutorialEvent>,
) {
    if std::mem::take(&mut cascade.reset_watch) {
        *watch = SelfWatch::default();
    }
    // The descriptor edges — a change, never the login descriptor (the watchers run on
    // field changes; the first sight arms silently).
    if let Ok(store) = self_q.single() {
        let level = store.0.unit_level();
        let player_flags = store.0.player_flags();
        let unit_flags = store.0.unit_flags();
        let rest_state = store.0.player_rest_state();
        if let (Some(prev), Some(new)) = (watch.level, level) {
            if new > prev {
                for (threshold, id) in LEVEL_TRIGGERS {
                    if new >= threshold {
                        asks.write(TutorialEvent::trigger(id));
                    }
                }
            }
        }
        if let Some(prev) = watch.player_flags {
            let rose = |bit: u32| prev & bit == 0 && player_flags & bit != 0;
            if rose(PLAYER_FLAGS_GHOST) {
                asks.write(TutorialEvent::trigger(id::DEATH));
            }
            if rose(PLAYER_FLAGS_RESTING) {
                asks.write(TutorialEvent::trigger(id::RESTING));
            }
        }
        if let Some(prev) = watch.unit_flags {
            if prev & UNIT_FLAG_PVP_ATTACKABLE == 0 && unit_flags & UNIT_FLAG_PVP_ATTACKABLE != 0 {
                asks.write(TutorialEvent::trigger(id::PVP_COMBAT));
            }
        }
        if let (Some(prev), Some(new)) = (watch.rest_state, rest_state) {
            // The rested table: only rest state 1 names a tutorial (the other rows carry the
            // sentinel), on the field's change.
            if new != prev && new == 1 {
                asks.write(TutorialEvent::trigger(id::RESTED));
            }
        }
        watch.level = level;
        watch.player_flags = Some(player_flags);
        watch.unit_flags = Some(unit_flags);
        watch.rest_state = rest_state;
    }
    if let Some(prev) = watch.swimming {
        if !prev && player.swimming {
            asks.write(TutorialEvent::trigger(id::SWIMMING));
        }
    }
    watch.swimming = Some(player.swimming);
}

/// After the script tick: the four verbs' writes, then every send.
fn drain_tutorials(
    script: Option<NonSendMut<UiScript>>,
    mut tutorials: ResMut<Tutorials>,
    commands: Res<NetCommands>,
) {
    if let Some(mut script) = script {
        for id in script.take_tutorial_flag_requests() {
            tutorials.acknowledge(id);
        }
        for _ in 0..script.take_tutorial_clears() {
            tutorials.clear();
        }
        for _ in 0..script.take_tutorial_resets() {
            tutorials.reset();
        }
    }
    for send in std::mem::take(&mut tutorials.sends) {
        let command = match send {
            TutorialSend::Flag(id) => ClientCommand::TutorialFlag { id },
            TutorialSend::Clear => ClientCommand::TutorialClear,
            TutorialSend::Reset => ClientCommand::TutorialReset,
        };
        let _ = commands.0.send(command);
    }
}

/// The world-enter bring-up: the banks zeroed, then the bank captured during the login handshake
/// applied if there was one (the world stream's copy lands through `SessionEvent` otherwise).
fn on_world_enter(
    mut entered: MessageReader<EnteredWorldMessage>,
    mut tutorials: ResMut<Tutorials>,
    mut cascade: ResMut<WorldEnterCascade>,
) {
    for e in entered.read() {
        tutorials.world_enter();
        if let Some(bytes) = &e.tutorial_flags {
            tutorials.apply_flags(bytes);
        }
        cascade.armed = true;
        cascade.entered_at = Some(Instant::now());
        cascade.moved = false;
        cascade.reset_watch = true;
    }
}

/// The enter-world cascade `0x4908c0`'s two unconditional triggers — Welcome (`0x490a48`) then
/// Questgivers (`0x490a51`) — run once per world session when the local player object exists,
/// which is after the bank has landed on a stock server; and the Movement popup's silence window.
#[derive(Resource, Default)]
pub(crate) struct WorldEnterCascade {
    armed: bool,
    entered_at: Option<Instant>,
    /// A movement input happened this session (the Movement auto-acknowledge site).
    pub(crate) moved: bool,
    /// The descriptor watcher's memory is owed a reset (a new world session).
    reset_watch: bool,
}

fn run_world_enter_cascade(
    mut cascade: ResMut<WorldEnterCascade>,
    mut tutorials: ResMut<Tutorials>,
    self_q: Query<(), With<SelfPlayer>>,
) {
    let now = Instant::now();
    if cascade.armed && !self_q.is_empty() {
        cascade.armed = false;
        tutorials.trigger(id::WELCOME, 0, now);
        tutorials.trigger(id::QUESTGIVERS, 0, now);
    }
    if let Some(at) = cascade.entered_at {
        if !cascade.moved && now.duration_since(at) >= MOVEMENT_SILENCE {
            cascade.entered_at = None;
            tutorials.trigger(id::MOVEMENT, 0, now);
        }
    }
}

/// The world-input sites (`0x514840`), bundled for the controller: a movement input
/// acknowledges Movement and arms the 10 s Targeting popup; a mouse-look acknowledges Cameras.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct InputHooks<'w> {
    /// Optional, so a controller test's partial app runs without the tutorial plugin.
    asks: Option<MessageWriter<'w, TutorialEvent>>,
    cascade: Option<ResMut<'w, WorldEnterCascade>>,
}

impl InputHooks<'_> {
    /// A movement input this frame (`test ecx,0x1030` — INFERRED as the forward/side axes).
    pub(crate) fn moved(&mut self) {
        if let Some(c) = self.cascade.as_mut() {
            c.moved = true;
        }
        if let Some(asks) = self.asks.as_mut() {
            asks.write(TutorialEvent::Acknowledge { id: id::MOVEMENT });
            asks.write(TutorialEvent::Trigger {
                id: id::TARGETING,
                delay_ms: TARGETING_DELAY_MS,
            });
        }
    }

    /// A mouse-look this frame (`[ebp+8] & 3`, the two buttons).
    pub(crate) fn mouselooked(&mut self) {
        if let Some(asks) = self.asks.as_mut() {
            asks.write(TutorialEvent::Acknowledge { id: id::CAMERAS });
        }
    }
}

/// The window edges the reference triggers or acknowledges from inside its handlers: the
/// trainer list (Trainers acknowledged, `0x4d74ae`), the vendor list (Vendors, `0x4fad32`), the
/// taxi map (Travel, `0x4dbaa2`), and the group turning raid (Raid Groups, `0x4ba60d` — INFERRED
/// as the raid-type edge; the row names only its non-zero argument).
#[derive(Default)]
struct WindowWatch {
    trainer: Option<u64>,
    vendor: Option<u64>,
    taxi: bool,
    raid: Option<bool>,
}

fn watch_windows(
    trainer: Res<crate::ui_trainer::TrainerOpen>,
    merchant: Res<crate::ui_merchant::MerchantOpen>,
    taxi: Res<crate::ui_taxi::TaxiState>,
    group: Res<crate::ui_party::GroupState>,
    mut watch: Local<WindowWatch>,
    mut asks: MessageWriter<TutorialEvent>,
) {
    if trainer.trainer.is_some() && trainer.trainer != watch.trainer {
        asks.write(TutorialEvent::Acknowledge { id: id::TRAINERS });
    }
    watch.trainer = trainer.trainer;
    if merchant.vendor.is_some() && merchant.vendor != watch.vendor {
        asks.write(TutorialEvent::trigger(id::VENDORS));
    }
    watch.vendor = merchant.vendor;
    let taxi_open = taxi.open.is_some();
    if taxi_open && !watch.taxi {
        asks.write(TutorialEvent::trigger(id::TRAVEL));
    }
    watch.taxi = taxi_open;
    let raid = group.in_group && group.group_type == crate::ui_party::GROUPTYPE_RAID;
    if raid && watch.raid == Some(false) {
        asks.write(TutorialEvent::trigger(id::RAID_GROUPS));
    }
    watch.raid = Some(raid);
}

fn on_world_leave(mut tutorials: ResMut<Tutorials>, mut cascade: ResMut<WorldEnterCascade>) {
    tutorials.world_leave();
    cascade.armed = false;
    cascade.entered_at = None;
}

pub(crate) struct TutorialPlugin;

impl Plugin for TutorialPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<Tutorials>()
            .init_resource::<WorldEnterCascade>()
            .add_message::<TutorialEvent>()
            .add_systems(
                Update,
                (
                    on_world_enter.before(run_world_enter_cascade),
                    run_world_enter_cascade.before(feed_tutorials),
                    watch_windows.before(feed_tutorials),
                    watch_self.after(on_world_enter).before(feed_tutorials),
                    // **Not before the in-game UI exists**. This is the one feed
                    // in 2226's 29-system audit that could still lose a login's own payload:
                    // `run_world_enter_cascade` triggers WELCOME and QUESTGIVERS the moment
                    // `SelfPlayer` exists, a 0-delay `trigger` pushes straight into `fired`, and
                    // the `mem::take(&mut tutorials.fired)` below fires `TUTORIAL_TRIGGER` at
                    // whatever VM is in the world. No `VmMemo` sits behind that publication, so
                    // 2226's fresh session cannot bring it back.
                    //
                    // The VM guard already covers the *park* — it sits above the take, so a
                    // parked frame returns with `fired` intact. What it does not cover is the
                    // one-frame window where the wire is in-world and the boot VM is still live
                    // (2214): `on_world_enter` reads `EnteredWorldMessage` out of the same drain
                    // that writes it, so the arm, the trigger and the take can all land in that
                    // one frame — the coin flip being whether `apply_net_updates`' spawn command
                    // has applied yet, which nothing here declares an edge against.
                    //
                    // The gate costs nothing: the cascade still arms and still triggers, `fired`
                    // simply waits, and the first frame with an interface delivers the set.
                    feed_tutorials
                        .in_set(UiFeed)
                        .run_if(crate::ui_script::ingame_ui_up),
                    drain_tutorials.after(UiInput),
                ),
            )
            .add_systems(OnExit(ClientState::InWorld), on_world_leave);
    }
}

/// The tutorial flags' packet handler (in the net handler table since 2326).
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::Tutorials;
    use crate::net::NetHandlerApp;

    /// Register the handler — called from [`super::TutorialPlugin`].
    pub(super) fn register(app: &mut App) {
        app.net_handler(SessionEventKind::TutorialFlags, on_flags);
    }

    fn on_flags(In(ev): In<SessionEvent>, mut tutorials: ResMut<Tutorials>) {
        if let SessionEvent::TutorialFlags(bytes) = ev {
            tutorials.apply_flags(&bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn banked() -> Tutorials {
        let mut t = Tutorials::default();
        t.apply_flags(&[0u8; 32]);
        t
    }

    /// No bank: silent. A clear bit: set in A only, the event owed, nothing sent. Twice: once.
    #[test]
    fn the_trigger_fires_once_per_bank_a_and_never_sends() {
        let now = Instant::now();
        let mut t = Tutorials::default();
        t.trigger(id::WELCOME, 0, now);
        assert!(t.fired.is_empty(), "no bank yet: silent");
        let mut t = banked();
        t.trigger(id::WELCOME, 0, now);
        t.trigger(id::WELCOME, 0, now);
        assert_eq!(t.fired, vec![id::WELCOME + 1], "published as id + 1, once");
        let (a, b) = t.bits();
        assert_eq!(bank_bit(a.unwrap(), id::WELCOME), Some(true));
        assert_eq!(
            bank_bit(b.unwrap(), id::WELCOME),
            Some(false),
            "bank B untouched"
        );
        assert!(t.sends.is_empty(), "the trigger sends nothing");
        t.trigger(300, 0, now);
        assert_eq!(t.fired.len(), 1, "past the bank: a no-op");
    }

    /// The delayed leg holds the id until due; an acknowledge cancels it.
    #[test]
    fn a_delayed_trigger_waits_and_an_acknowledge_cancels_it() {
        let now = Instant::now();
        let mut t = banked();
        t.trigger(id::TARGETING, TARGETING_DELAY_MS, now);
        assert!(t.fired.is_empty());
        t.tick(now + Duration::from_millis(9_999));
        assert!(t.fired.is_empty(), "not yet");
        t.tick(now + Duration::from_millis(10_000));
        assert_eq!(t.fired, vec![id::TARGETING + 1]);

        let mut t = banked();
        t.trigger(id::TARGETING, TARGETING_DELAY_MS, now);
        t.acknowledge(id::TARGETING);
        t.tick(now + Duration::from_millis(20_000));
        assert!(t.fired.is_empty(), "the acknowledge cancelled the timer");
        assert_eq!(t.sends, vec![TutorialSend::Flag(id::TARGETING)]);
    }

    /// The acknowledge: bank B gates, both banks set, one send; a second is silent; absent bank
    /// B is a no-op.
    #[test]
    fn the_acknowledge_sets_both_banks_and_sends_once() {
        let mut t = Tutorials::default();
        t.acknowledge(id::CHATTING);
        assert!(t.sends.is_empty(), "no bank: nothing");
        let mut t = banked();
        t.acknowledge(id::CHATTING);
        t.acknowledge(id::CHATTING);
        assert_eq!(t.sends, vec![TutorialSend::Flag(id::CHATTING)]);
        let (a, b) = t.bits();
        assert_eq!(bank_bit(a.unwrap(), id::CHATTING), Some(true));
        assert_eq!(bank_bit(b.unwrap(), id::CHATTING), Some(true));
        // Acknowledged first: the later trigger is silent (A ⊇ B).
        t.trigger(id::CHATTING, 0, Instant::now());
        assert!(t.fired.is_empty());
    }

    /// Clear sets every bit and flushes the timers; Reset clears every bit and keeps them.
    #[test]
    fn clear_and_reset_write_both_banks() {
        let now = Instant::now();
        let mut t = banked();
        t.trigger(id::TARGETING, TARGETING_DELAY_MS, now);
        t.clear();
        assert!(t.timers.is_empty(), "clear flushes the timers");
        let (a, b) = t.bits();
        assert!(a.unwrap().iter().all(|&x| x == 0xFF) && b.unwrap().iter().all(|&x| x == 0xFF));
        t.reset();
        t.trigger(id::TARGETING, TARGETING_DELAY_MS, now);
        assert_eq!(
            t.timers.len(),
            1,
            "a cleared bank lets the trigger through again"
        );
        t.reset();
        assert_eq!(t.timers.len(), 1, "reset does NOT flush the timers");
        let (a, b) = t.bits();
        assert!(a.unwrap().iter().all(|&x| x == 0) && b.unwrap().iter().all(|&x| x == 0));
        assert_eq!(
            t.sends,
            vec![
                TutorialSend::Clear,
                TutorialSend::Reset,
                TutorialSend::Reset
            ]
        );
    }

    /// World enter zeroes present banks and keeps their size; the packet fills both.
    #[test]
    fn world_enter_zeroes_and_the_packet_fills() {
        let mut t = Tutorials::default();
        t.world_enter();
        assert!(t.fire_once.is_none(), "no bank: nothing to zero");
        t.apply_flags(&[0xFFu8; 4]);
        t.world_enter();
        let (a, b) = t.bits();
        assert_eq!(a.unwrap(), &[0u8; 4]);
        assert_eq!(b.unwrap(), &[0u8; 4]);
        assert_eq!(bank_bit(a.unwrap(), 31), Some(false));
        assert_eq!(bank_bit(a.unwrap(), 32), None, "past a 4-byte bank");
    }

    /// A bare app around [`feed_tutorials`] — the one system that owes the VM the bank.
    fn feeder() -> App {
        let mut app = App::new();
        app.init_resource::<Tutorials>()
            .init_resource::<crate::items::Items>()
            .init_resource::<crate::sound::MessageSounds>()
            .add_message::<TutorialEvent>()
            .add_systems(Update, feed_tutorials);
        app
    }

    fn enabled(app: &mut App) -> bool {
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .eval::<bool>("return TutorialsEnabled() ~= nil")
            .unwrap()
    }

    /// **The bank is owed to every VM** — the `/reload` bug, reproduced.
    ///
    /// `ReloadUI()` is `end_ui_session` + the entry load back to back (1290/1291), and only the
    /// first of those mints a VM: the world-entry path that fills the banks
    /// ([`Tutorials::world_enter`]) never runs, so a push gated on "the bank moved" never fires
    /// again. `TutorialsEnabled()` then answers nil for the rest of the session, the Show
    /// Tutorials row loads OFF, and ticking it back on passes `BenillaOptionsFrame_SetTutorialsEnabled`'s
    /// `~=` guard into `ResetTutorials()` — a `CMSG_TUTORIAL_RESET` that re-arms, account-wide,
    /// every popup the player had already dismissed.
    #[test]
    fn a_rebuilt_vm_is_told_the_acknowledged_bank_again() {
        let mut app = feeder();
        // The packet lands: one unacknowledged bit is all `TutorialsEnabled()` scans for.
        let mut bank = vec![0xFFu8; 32];
        bank[0] = 0xFE;
        app.world_mut()
            .resource_mut::<Tutorials>()
            .apply_flags(&bank);

        app.insert_non_send_resource(UiScript::new().expect("VM"));
        app.update();
        assert!(
            enabled(&mut app),
            "the VM that was live when the bank landed"
        );

        // `ReloadUI()`: a fresh VM, and nothing touches the app's banks.
        app.insert_non_send_resource(UiScript::new().expect("VM"));
        assert!(
            !enabled(&mut app),
            "a fresh VM starts knowing no bank — this is the state the row used to load in"
        );
        app.update();
        assert!(enabled(&mut app), "…and must be told it again");
    }

    /// The push is per-VM, not per-frame: a steady session hands the bank over once.
    #[test]
    fn an_unchanged_bank_is_not_re_pushed_every_frame() {
        let mut app = feeder();
        app.world_mut()
            .resource_mut::<Tutorials>()
            .apply_flags(&[0u8; 32]);
        app.insert_non_send_resource(UiScript::new().expect("VM"));
        app.update();
        // A bank the VM has already been told is not handed over again — proven by clearing it
        // behind the feed's back and watching the next frame leave it cleared.
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .set_tutorial_bank(None);
        app.update();
        assert!(!enabled(&mut app), "unchanged ⇒ nothing pushed");
        // A real change is still pushed, on the same VM.
        app.world_mut()
            .resource_mut::<Tutorials>()
            .acknowledge(id::CHATTING);
        app.update();
        assert!(enabled(&mut app), "a moved bank reaches the live VM");
    }

    /// **The login's own tutorials wait for an interface** — built on the REAL
    /// plugin, so it fails if the gate is taken off the registration rather than off a copy of it.
    ///
    /// `TUTORIAL_TRIGGER` has no [`crate::ui_script::VmMemo`] behind it: `trigger` sets the bank
    /// bit and pushes the id into `fired`, and the feed's `mem::take` fires it once at whatever VM
    /// is in the world. So unlike every one-shot 2226 covers, a firing lost to a frameless VM
    /// cannot be recovered by the entry load minting a new session — there is nothing left to
    /// re-spend. `run_world_enter_cascade` puts WELCOME and QUESTGIVERS into exactly that position
    /// on every login.
    ///
    /// Against the ungated shape the first assertion reads *"`fired` is empty — the login's two
    /// tutorials were published to a VM with no frames and are gone for the session"*.
    #[test]
    fn the_world_enter_tutorials_are_not_fired_into_a_ui_less_vm() {
        let mut app = App::new();
        app.add_plugins(TutorialPlugin)
            .init_resource::<crate::items::Items>()
            .init_resource::<crate::sound::MessageSounds>()
            // The plugin's other watchers' inputs — none of them is the subject here; they are
            // present so the REAL plugin can be driven rather than a copy of one of its systems.
            .init_resource::<crate::ui_trainer::TrainerOpen>()
            .init_resource::<crate::ui_merchant::MerchantOpen>()
            .init_resource::<crate::ui_taxi::TaxiState>()
            .init_resource::<crate::ui_party::GroupState>()
            .init_resource::<Player>()
            .add_message::<EnteredWorldMessage>();
        let (tx, _rx) = crossbeam_channel::unbounded();
        app.insert_resource(crate::net::NetCommands(tx));

        // The deferral window (1978/2214): in the world on the wire, the entry load still owed,
        // and a live boot VM that has strings and fonts but not one frame.
        app.insert_resource(State::new(crate::char_select::ClientState::InWorld));
        app.insert_resource(crate::ui_script::PendingEntryUiLoad);
        app.insert_non_send_resource(UiScript::new().expect("the boot VM"));

        {
            let mut t = app.world_mut().resource_mut::<Tutorials>();
            t.apply_flags(&[0u8; 32]);
            t.trigger(id::WELCOME, 0, Instant::now());
        }

        app.update();
        assert_eq!(
            app.world().resource::<Tutorials>().fired,
            vec![id::WELCOME + 1],
            "the window must not publish: there is no frame registered for TUTORIAL_TRIGGER, and \
             the take is one-way"
        );

        // The interface comes up; the same frame delivers what was waiting.
        app.world_mut()
            .remove_resource::<crate::ui_script::PendingEntryUiLoad>();
        app.update();
        assert!(
            app.world().resource::<Tutorials>().fired.is_empty(),
            "…and once there is an interface, it is published exactly once"
        );
    }
}
