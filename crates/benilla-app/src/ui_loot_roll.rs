//! The group loot roll feed: `SMSG_LOOT_START_ROLL`, `SMSG_LOOT_ROLL`, `SMSG_LOOT_ROLL_WON` and
//! `SMSG_LOOT_ALL_PASSED` become the stock `GroupLootFrame`'s events and the roll chat lines, and
//! its `RollOnLoot` votes go back out as `CMSG_LOOT_ROLL`.
//!
//! A fresh roll's `START_LOOT_ROLL` waits for its item template, as in the reference: `0x61b310`
//! ends in the cache lookup `0x55ba30`, whose hit fires the event at once (`0x61b430`) and whose
//! miss fires it from the arrival callback `0x61b460`. `GroupLootFrame` paints once, from `OnShow`,
//! so an event ahead of the template paints a blank frame. Deviation: a negative answer, or none
//! within [`TEMPLATE_HOLD_MAX_MS`], opens the frame unresolved, because a frame that never opens
//! cannot even be passed on; the reference's callback fires on success only.
//!
//! A held roll's `arg2` is still the wire's countdown (`0x61b430` pushes `node+0x3c`); only the bar
//! is short, as `GetLootRollTimeLeft` counts from the packet's arrival.
//!
//! `rollID` is a client-local serial; the wire addresses a roll by `(lootedTarget, itemSlot)`. Our
//! own vote closes our frame at once, as `RollOnLoot` (`0x61bdf0`) sends the CMSG and fires
//! `CANCEL_LOOT_ROLL` in one call. On a bind-on-pickup roll, Need or Greed only raises
//! `CONFIRM_LOOT_ROLL` and `ConfirmLootRoll` sends, the reference's C `RollOnLoot` gate; Pass is
//! never gated.

use benilla_protocol::messages::{roll_vote, LootAllPassed, LootRoll, LootRollWon, LootStartRoll};
use bevy::prelude::*;

use benilla_ui::script::{LootRollEntry, LootRollsState, ScriptValue, UiScript};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands, SelfGuid};
use crate::ui_chat::{ChatEvent, ChatEventKind, ChatLog};
use crate::ui_script::{UiFeed, UiInput};

/// Frames a pending line retries its names before dropping: a negative-cached name never resolves.
const LINE_MAX_TRIES: u16 = 120;

/// How long a fresh roll waits for its item template before `START_LOOT_ROLL` fires anyway: wall
/// clock, not frames, as the wait comes out of the roll's own minute.
const TEMPLATE_HOLD_MAX_MS: u32 = 2_000;

/// `item_template.bonding` bind on pickup (vmangos `ItemPrototype.h` `ItemBondingType`); only 1
/// gets `GroupLootFrame_OnShow`'s gold backdrop.
const BIND_WHEN_PICKED_UP: u32 = 1;

/// One group loot roll currently open on our screen.
struct ActiveRoll {
    /// The id Lua addresses the roll by.
    roll_id: u32,
    looted_target: u64,
    item_slot: u32,
    item_id: u32,
    /// The drop's random-suffix roll: the reference's `SetLootRollItem` (`0x5364a0`) copies it
    /// into the tooltip's `+0x424` with no item object, so it is the hover's only enchant source.
    random_property_id: u32,
    /// The wire's `countdownTime`, kept whole: `START_LOOT_ROLL`'s `arg2` and the bar's maximum.
    countdown_ms: u32,
    /// Milliseconds left, saturating at 0; the server's resolution closes the roll, not the tick.
    remaining_ms: u32,
    /// How long the roll has waited for its item template.
    held_ms: u32,
    /// Whether `START_LOOT_ROLL` has gone out.
    announced: bool,
}

/// One queued chat announcement, awaiting the names it needs.
struct PendingLine {
    line: RollLine,
    tries: u16,
}

/// Which announcement a [`PendingLine`] renders.
enum RollLine {
    Announce(LootRoll),
    Won(LootRollWon),
    AllPassed(LootAllPassed),
}

/// Every group loot roll open on our screen; cleared on disconnect.
#[derive(Resource, Default)]
pub(crate) struct LootRolls {
    active: Vec<ActiveRoll>,
    /// Never reused within a session, so a late packet for a closed roll cannot hit a fresh one.
    next_id: u32,
    pending: Vec<PendingLine>,
    /// Closed rolls, one `CANCEL_LOOT_ROLL` each.
    cancelled: Vec<u32>,
}

impl LootRolls {
    /// `SMSG_LOOT_START_ROLL`: list the roll under a fresh id. A duplicate `(looted_target,
    /// item_slot)` is ignored, as vmangos can re-send every active roll on reconnect
    /// (`Group::SendLootStartRollsForPlayer`, under `SendLootRollUponReconnect`).
    pub(crate) fn start(&mut self, p: LootStartRoll) {
        if self
            .active
            .iter()
            .any(|r| r.looted_target == p.looted_target && r.item_slot == p.item_slot)
        {
            debug!(
                "ui_loot_roll: duplicate start for {:#x} slot {} — ignored",
                p.looted_target, p.item_slot
            );
            return;
        }
        self.next_id += 1;
        let roll_id = self.next_id;
        self.active.push(ActiveRoll {
            roll_id,
            looted_target: p.looted_target,
            item_slot: p.item_slot,
            item_id: p.item_id,
            random_property_id: p.random_property_id,
            countdown_ms: p.countdown_ms,
            remaining_ms: p.countdown_ms,
            held_ms: 0,
            announced: false,
        });
    }

    /// `SMSG_LOOT_ROLL`: queue the chat line.
    pub(crate) fn announce(&mut self, p: LootRoll) {
        self.pending.push(PendingLine {
            line: RollLine::Announce(p),
            tries: 0,
        });
    }

    /// `SMSG_LOOT_ROLL_WON`: queue the line and close the frame.
    pub(crate) fn won(&mut self, p: LootRollWon) {
        self.pending.push(PendingLine {
            line: RollLine::Won(p),
            tries: 0,
        });
        self.close(p.looted_target, p.item_slot);
    }

    /// `SMSG_LOOT_ALL_PASSED`: queue the line and close the frame; the item stays lootable.
    pub(crate) fn all_passed(&mut self, p: LootAllPassed) {
        self.pending.push(PendingLine {
            line: RollLine::AllPassed(p),
            tries: 0,
        });
        self.close(p.looted_target, p.item_slot);
    }

    /// Drop the roll if we still hold it and queue its `CANCEL_LOOT_ROLL`, announced or not: the
    /// reference's `0x61b9e0` and `0x61b640` check only the node's closed byte `+0x3a`.
    fn close(&mut self, looted_target: u64, item_slot: u32) {
        if let Some(i) = self
            .active
            .iter()
            .position(|r| r.looted_target == looted_target && r.item_slot == item_slot)
        {
            let r = self.active.remove(i);
            self.cancelled.push(r.roll_id);
        }
    }

    /// Our own vote: close the frame and return the roll's wire identity; `None` for a stale click.
    fn vote(&mut self, roll_id: u32) -> Option<(u64, u32)> {
        let i = self.active.iter().position(|r| r.roll_id == roll_id)?;
        let r = self.active.remove(i);
        self.cancelled.push(r.roll_id);
        Some((r.looted_target, r.item_slot))
    }

    /// Tick every open roll's bar down, and the template hold of every unannounced roll up.
    fn tick(&mut self, delta_ms: u32) {
        for r in &mut self.active {
            r.remaining_ms = r.remaining_ms.saturating_sub(delta_ms);
            if !r.announced {
                r.held_ms = r.held_ms.saturating_add(delta_ms);
            }
        }
    }

    /// Session teardown: no `CANCEL_LOOT_ROLL`, as the UI goes down with the session.
    pub(crate) fn clear(&mut self) {
        self.active.clear();
        self.pending.clear();
        self.cancelled.clear();
    }
}

/// The group rolls' packet handlers.
mod net {
    use benilla_protocol::messages::{LootAllPassed, LootRoll, LootRollWon, LootStartRoll};
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::LootRolls;
    use crate::net::NetHandlerApp;

    /// Register the roll handlers, called from [`super::UiLootRollPlugin`].
    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::LootStartRoll, on_packet)
            .net_handler(K::LootRoll, on_packet)
            .net_handler(K::LootRollWon, on_packet)
            .net_handler(K::LootAllPassed, on_packet)
            .net_handler(K::Disconnected, on_session_end);
    }

    fn on_packet(In(ev): In<SessionEvent>, mut rolls: ResMut<LootRolls>) {
        match ev {
            SessionEvent::LootStartRoll(p) => loot_start_roll(p, &mut rolls),
            SessionEvent::LootRoll(p) => loot_roll(p, &mut rolls),
            SessionEvent::LootRollWon(p) => loot_roll_won(p, &mut rolls),
            SessionEvent::LootAllPassed(p) => loot_all_passed(p, &mut rolls),
            _ => {}
        }
    }

    fn on_session_end(In(_): In<SessionEvent>, mut rolls: ResMut<LootRolls>) {
        rolls.clear();
    }

    fn loot_start_roll(p: LootStartRoll, rolls: &mut LootRolls) {
        debug!(
            "net: loot roll opened on item {} ({:#x} slot {}), {} ms",
            p.item_id, p.looted_target, p.item_slot, p.countdown_ms
        );
        rolls.start(p);
    }

    fn loot_roll(p: LootRoll, rolls: &mut LootRolls) {
        debug!(
            "net: loot roll announce — roller {:#x} number {} type {}",
            p.roller, p.roll_number, p.roll_type
        );
        rolls.announce(p);
    }

    fn loot_roll_won(p: LootRollWon, rolls: &mut LootRolls) {
        debug!(
            "net: loot roll won by {:#x} with {} (type {})",
            p.winner, p.roll_number, p.roll_type
        );
        rolls.won(p);
    }

    fn loot_all_passed(p: LootAllPassed, rolls: &mut LootRolls) {
        debug!("net: loot roll — everyone passed on item {}", p.item_id);
        rolls.all_passed(p);
    }
}

pub(crate) struct UiLootRollPlugin;

impl Plugin for UiLootRollPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<LootRolls>().add_systems(
            Update,
            (
                // The feed before the input pass shows a new roll the same frame; the drain after
                // it sends a vote the same frame.
                feed_loot_rolls.in_set(UiFeed),
                drain_loot_rolls.after(UiInput),
            ),
        );
    }
}

/// Pick and fill an announcement: the enUS strings of `GlobalStrings.lua:2614-2633`, inlined where
/// the reference reads the install's. It branches on `roll_number` first: a Greed vote is
/// `(128, 2)`, a Greed dice roll `(1..=100, 2)`. The dice line has no self form: the reference's
/// shared tail (`0x61c320`) picks `ROLLED_GREED` for rollType 2, else `ROLLED_NEED`, with no self
/// test.
///
/// `detailed` is `showLootSpam` (`0xb4e2bc`). At 0, vote and dice lines drop (`0x61c0b0`, `None`
/// here) and the won line (`0x61b9e0`) takes `*_NO_SPAM_NEED`/`_GREED` with the roll number, Need
/// on `rollType == 1`; all-passed is never gated (`0x61b640`).
fn format_line(
    line: &RollLine,
    name: Option<&str>,
    is_self: bool,
    link: &str,
    detailed: bool,
) -> Option<String> {
    if !detailed {
        if let RollLine::Announce(_) = line {
            // `0x61c0b9`: the composer frees the node and returns.
            return None;
        }
    }
    Some(format_line_detailed(line, name, is_self, link, detailed))
}

fn format_line_detailed(
    line: &RollLine,
    name: Option<&str>,
    is_self: bool,
    link: &str,
    detailed: bool,
) -> String {
    match line {
        // A dice result, checked before the vote shapes.
        RollLine::Announce(p) if p.is_dice() => {
            let n = p.roll_number;
            let w = name.unwrap_or_default();
            // `LOOT_ROLL_ROLLED_NEED`/`_GREED`, with no self form: `is_self` is unread. Need on
            // type 1, else Greed, where the reference's tail takes Greed on 2, else Need: the two
            // agree on the 1 and 2 vmangos sends (`Group.cpp:1163,1214`).
            match p.roll_type {
                roll_vote::NEED => format!("Need Roll - {n} for {link} by {w}"),
                _ => format!("Greed Roll - {n} for {link} by {w}"),
            }
        }
        RollLine::Announce(p) => match (is_self, p.vote()) {
            // `LOOT_ROLL_NEED_SELF`, `_GREED_SELF`, `_PASSED_SELF`.
            (true, Some(roll_vote::NEED)) => format!("You have selected Need for: {link}"),
            (true, Some(roll_vote::GREED)) => format!("You have selected Greed for: {link}"),
            (true, _) => format!("You passed on: {link}"),
            // `LOOT_ROLL_NEED`, `_GREED`, `_PASSED`.
            (false, Some(roll_vote::NEED)) => {
                format!("{} has selected Need for: {link}", name.unwrap_or_default())
            }
            (false, Some(roll_vote::GREED)) => {
                format!(
                    "{} has selected Greed for: {link}",
                    name.unwrap_or_default()
                )
            }
            (false, _) => format!("{} passed on: {link}", name.unwrap_or_default()),
        },
        // `*_NO_SPAM_*`: with detail off this is the roll's only line, so it carries the number.
        RollLine::Won(p) if !detailed => {
            let kind = if p.roll_type == roll_vote::NEED {
                "Need"
            } else {
                "Greed"
            };
            let n = p.roll_number;
            if is_self {
                format!("You won: {link} |cff818181({kind} - {n})|r")
            } else {
                format!(
                    "{} won: {link} |cff818181({kind} - {n})|r",
                    name.unwrap_or_default()
                )
            }
        }
        // `LOOT_ROLL_YOU_WON`, `LOOT_ROLL_WON`.
        RollLine::Won(_) if is_self => format!("You won: {link}"),
        RollLine::Won(_) => format!("{} won: {link}", name.unwrap_or_default()),
        // `LOOT_ROLL_ALL_PASSED` names nobody.
        RollLine::AllPassed(_) => format!("Everyone passed on: {link}"),
    }
}

/// Render one queued announcement, or `None` while a name it needs is in flight.
fn render(
    line: &RollLine,
    self_guid: Option<u64>,
    items: &Items,
    names: &NameCache,
    commands: &NetCommands,
    rolls: crate::items::RollCatalogs,
    detailed: bool,
) -> Option<String> {
    // Every line embeds the item link, so the template comes first.
    let (looted_item, roll, roller) = match line {
        RollLine::Announce(p) => (p.item_id, p.random_property_id, Some(p.roller)),
        RollLine::Won(p) => (p.item_id, p.random_property_id, Some(p.winner)),
        RollLine::AllPassed(p) => (p.item_id, p.random_property_id, None),
    };
    let t = items.template(looted_item, 0, commands)?;
    // The link names the rolled drop, suffix included, as the roll frame does.
    let link = crate::ui_items::item_link_full(
        looted_item,
        0,
        roll,
        0,
        &rolls.name(&t.name.clone(), roll),
        t.quality,
    );

    let is_self = roller.is_some() && roller == self_guid;

    // Every third-person line needs a name, and so does our own dice line (no self form).
    let needs_name = match line {
        RollLine::Announce(p) => p.is_dice() || !is_self,
        RollLine::Won(_) => !is_self,
        RollLine::AllPassed(_) => false,
    };
    let name = match roller {
        Some(g) if needs_name => Some(names.resolve(g, commands)?.to_string()),
        _ => None,
    };

    // Never `None` here: the caller drops suppressed lines, as a `None` from here means retry.
    format_line(line, name.as_deref(), is_self, &link, detailed)
}

/// Post the queued announcements as `CHAT_MSG_LOOT` lines, as the reference does.
fn drain_lines(
    rolls: &mut LootRolls,
    self_guid: Option<u64>,
    items: &Items,
    names: &NameCache,
    commands: &NetCommands,
    chat: &mut ChatLog,
    catalogs: crate::items::RollCatalogs,
    detailed: bool,
) {
    let pending = std::mem::take(&mut rolls.pending);
    let mut still = Vec::new();
    for mut p in pending {
        // Detail off drops vote and dice lines here, never through `render`'s retrying `None`.
        if !detailed && matches!(p.line, RollLine::Announce(_)) {
            continue;
        }
        match render(
            &p.line, self_guid, items, names, commands, catalogs, detailed,
        ) {
            Some(text) => chat.push_event(ChatEvent::text_only(ChatEventKind::Loot, text)),
            None => {
                p.tries += 1;
                if p.tries < LINE_MAX_TRIES {
                    still.push(p);
                }
            }
        }
    }
    rolls.pending = still;
}

/// The Lua-facing snapshot; its template fields are `None` or `false` until the template answers.
fn snapshot(
    rolls: &LootRolls,
    items: &Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
    catalogs: crate::items::RollCatalogs,
) -> LootRollsState {
    let entries = rolls
        .active
        .iter()
        .map(|r| {
            let t = items.template(r.item_id, 0, commands);
            let texture = t
                .and_then(|t| icons?.catalog.get(t.display_info_id))
                .and_then(|d| d.icon.clone());
            LootRollEntry {
                roll_id: r.roll_id,
                name: t.map(|t| catalogs.name(&t.name, r.random_property_id)),
                texture,
                // The reference's `GetLootRollItemInfo` returns a literal 1 (`0x4c3160`); the
                // packet carries no count.
                quantity: 1,
                quality: t.map(|t| t.quality),
                bind_on_pickup: t.is_some_and(|t| t.bonding == BIND_WHEN_PICKED_UP),
                time_left_ms: r.remaining_ms,
                item_id: r.item_id,
                random_property_id: r.random_property_id,
                // `GetLootRollItemLink`, the chat lines' link; `None` until the template answers.
                link: t.map(|t| {
                    crate::ui_items::item_link_full(
                        r.item_id,
                        0,
                        r.random_property_id,
                        0,
                        &catalogs.name(&t.name, r.random_property_id),
                        t.quality,
                    )
                }),
            }
        })
        .collect();
    LootRollsState { rolls: entries }
}

/// Tick the open rolls, push them into the VM, fire their events and post their chat lines.
fn feed_loot_rolls(
    script: Option<NonSendMut<UiScript>>,
    mut rolls: ResMut<LootRolls>,
    items: Res<Items>,
    names: Res<NameCache>,
    icons: Option<Res<ItemDisplays>>,
    commands: Res<NetCommands>,
    self_guid: Res<SelfGuid>,
    mut chat: ResMut<ChatLog>,
    time: Res<Time>,
    mut last: Local<crate::ui_script::VmMemo<LootRollsState>>,
    props: Option<Res<crate::items::RandomProperties>>,
    enchants: Option<Res<crate::items::Enchants>>,
    // `showLootSpam`, read as each line is composed, as the reference's composers read it
    // (`0x61ba3a`, `0x61bafe`, `0x61c0b9`).
    loot: Res<crate::ui_loot::LootConfig>,
) {
    rolls.tick(time.delta().as_millis() as u32);

    let Some(mut script) = script else {
        return;
    };
    let last = last.get(&script);
    let catalogs = crate::items::RollCatalogs {
        props: props.as_deref(),
        enchants: enchants.as_deref(),
    };
    drain_lines(
        &mut rolls,
        self_guid.0,
        &items,
        &names,
        &commands,
        &mut chat,
        catalogs,
        loot.show_loot_spam,
    );

    // The snapshot goes in before the events: a `GroupLootFrame`'s `OnShow` reads its item from it.
    let fresh = snapshot(&rolls, &items, icons.as_deref(), &commands, catalogs);
    if fresh != *last {
        script.set_loot_rolls(fresh.clone());
        *last = fresh;
    }

    // The hold: a fresh roll announces once its template is in the snapshot above.
    for r in &mut rolls.active {
        if r.announced {
            continue;
        }
        if items.template(r.item_id, 0, &commands).is_none() {
            // The module doc's deviation: an entry the server does not know, or an expired hold.
            let unknown = items.template_answered_unknown(r.item_id);
            if !unknown && r.held_ms < TEMPLATE_HOLD_MAX_MS {
                continue;
            }
            debug!(
                "ui_loot_roll: roll {} opening unresolved ({})",
                r.roll_id,
                if unknown {
                    "the server does not know the entry"
                } else {
                    "template hold expired"
                }
            );
        }
        r.announced = true;
        debug!(
            "ui_loot_roll: roll {} opened ({} ms, held {} ms)",
            r.roll_id, r.countdown_ms, r.held_ms
        );
        script.fire_event(
            "START_LOOT_ROLL",
            vec![
                ScriptValue::Int(r.roll_id as i64),
                ScriptValue::Int(r.countdown_ms as i64),
            ],
        );
    }
    for roll_id in std::mem::take(&mut rolls.cancelled) {
        debug!("ui_loot_roll: roll {roll_id} cancelled");
        script.fire_event("CANCEL_LOOT_ROLL", vec![ScriptValue::Int(roll_id as i64)]);
    }
}

/// Drain the Lua votes: each `RollOnLoot` is one `CMSG_LOOT_ROLL` and closes our frame.
fn drain_loot_rolls(
    script: Option<NonSendMut<UiScript>>,
    mut rolls: ResMut<LootRolls>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    // A bind-on-pickup Need or Greed sends nothing yet and its frame stays up:
    // `UIParent_OnEvent` turns `CONFIRM_LOOT_ROLL` into its popup.
    for (roll_id, roll_type) in script.take_loot_roll_confirms() {
        debug!("ui_loot_roll: BoP confirm for {roll_type} on roll {roll_id}");
        script.fire_event(
            "CONFIRM_LOOT_ROLL",
            vec![
                ScriptValue::Int(roll_id as i64),
                ScriptValue::Int(roll_type as i64),
            ],
        );
    }
    for (roll_id, roll_type) in script.take_loot_roll_votes() {
        match rolls.vote(roll_id) {
            Some((looted_target, item_slot)) => {
                debug!("ui_loot_roll: vote {roll_type} on roll {roll_id}");
                let _ = commands.0.send(ClientCommand::LootRoll {
                    looted_target,
                    item_slot,
                    roll_type,
                });
            }
            None => debug!("ui_loot_roll: RollOnLoot({roll_id}) — no such roll, ignored"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start(target: u64, slot: u32, item_id: u32) -> LootStartRoll {
        LootStartRoll {
            looted_target: target,
            item_slot: slot,
            item_id,
            random_property_id: 0,
            countdown_ms: 60_000,
        }
    }

    fn announce(roller: u64, roll_number: u8, roll_type: u8) -> LootRoll {
        LootRoll {
            looted_target: 0xAA,
            item_slot: 0,
            roller,
            item_id: 17182,
            random_property_id: 0,
            roll_number,
            roll_type,
        }
    }

    const LINK: &str = "|cffa335ee|Hitem:17182:0:0:0|h[Sulfuras]|h|r";

    /// `(128, 2)` is a Greed vote and `(57, 2)` a Greed dice roll: `roll_type` alone cannot tell.
    #[test]
    fn line_table_covers_every_shape() {
        let other = Some("Bob");
        let me = Some("Aldric");
        // (packet, name, is_self, expected)
        let cases: &[(LootRoll, Option<&str>, bool, &str)] = &[
            // The three votes (`Group.cpp:970-990`), which have _SELF forms.
            (
                announce(1, 0, 0),
                other,
                false,
                "Bob has selected Need for: {L}",
            ),
            (
                announce(1, 0, 0),
                None,
                true,
                "You have selected Need for: {L}",
            ),
            (
                announce(1, 128, roll_vote::GREED),
                other,
                false,
                "Bob has selected Greed for: {L}",
            ),
            (
                announce(1, 128, roll_vote::GREED),
                None,
                true,
                "You have selected Greed for: {L}",
            ),
            (announce(1, 128, 128), other, false, "Bob passed on: {L}"),
            (announce(1, 128, 128), None, true, "You passed on: {L}"),
            // The dice results (`Group.cpp:1163,1214`): no _SELF form, so our own rolls name us.
            (
                announce(1, 57, roll_vote::NEED),
                other,
                false,
                "Need Roll - 57 for {L} by Bob",
            ),
            (
                announce(1, 57, roll_vote::NEED),
                me,
                true,
                "Need Roll - 57 for {L} by Aldric",
            ),
            (
                announce(1, 57, roll_vote::GREED),
                other,
                false,
                "Greed Roll - 57 for {L} by Bob",
            ),
            (
                announce(1, 57, roll_vote::GREED),
                me,
                true,
                "Greed Roll - 57 for {L} by Aldric",
            ),
        ];
        for (p, name, is_self, expect) in cases {
            let got = format_line(&RollLine::Announce(*p), *name, *is_self, LINK, true);
            assert_eq!(
                got,
                Some(expect.replace("{L}", LINK)),
                "roll_number {} roll_type {} name {name:?} is_self {is_self}",
                p.roll_number,
                p.roll_type
            );
        }
    }

    /// `LOOT_ROLL_ROLLED_NEED_SELF` is in `GlobalStrings.lua`, but no client code reads it.
    #[test]
    fn our_own_dice_roll_prints_third_person() {
        for roll_type in [roll_vote::NEED, roll_vote::GREED] {
            let got = format_line(
                &RollLine::Announce(announce(1, 57, roll_type)),
                Some("Aldric"),
                true, // is_self
                LINK,
                true,
            )
            .expect("detail on emits the dice line");
            assert!(
                got.ends_with("by Aldric"),
                "our own roll must name us third-person: {got}"
            );
            assert!(
                !got.contains("You roll"),
                "no _SELF dice form exists: {got}"
            );
        }
    }

    #[test]
    fn greed_vote_and_greed_roll_differ() {
        let vote = format_line(
            &RollLine::Announce(announce(1, 128, roll_vote::GREED)),
            Some("Bob"),
            false,
            LINK,
            true,
        )
        .unwrap();
        let dice = format_line(
            &RollLine::Announce(announce(1, 57, roll_vote::GREED)),
            Some("Bob"),
            false,
            LINK,
            true,
        )
        .unwrap();
        assert_ne!(vote, dice);
        assert!(vote.contains("has selected Greed"), "{vote}");
        assert!(dice.contains("Greed Roll - 57"), "{dice}");
    }

    #[test]
    fn resolution_lines() {
        let won = LootRollWon {
            looted_target: 0xAA,
            item_slot: 0,
            item_id: 17182,
            random_property_id: 0,
            winner: 1,
            roll_number: 84,
            roll_type: roll_vote::NEED,
        };
        assert_eq!(
            format_line(&RollLine::Won(won), Some("Bob"), false, LINK, true),
            Some(format!("Bob won: {LINK}"))
        );
        // The client reads `LOOT_ROLL_YOU_WON`: the won line has a self form.
        assert_eq!(
            format_line(&RollLine::Won(won), None, true, LINK, true),
            Some(format!("You won: {LINK}"))
        );
        let passed = LootAllPassed {
            looted_target: 0xAA,
            item_slot: 0,
            item_id: 17182,
            random_property_id: 0,
        };
        assert_eq!(
            format_line(&RollLine::AllPassed(passed), Some("Bob"), false, LINK, true),
            Some(format!("Everyone passed on: {LINK}"))
        );
    }

    /// `showLootSpam` 0 (the CVar `0xb4e2bc`).
    #[test]
    fn detail_off_suppresses_the_roll_lines_and_reshapes_the_winner() {
        for p in [
            announce(1, 128, roll_vote::NEED),
            announce(1, 128, roll_vote::GREED),
            announce(1, 128, roll_vote::PASS),
            announce(1, 57, roll_vote::NEED),
            announce(1, 57, roll_vote::GREED),
        ] {
            assert_eq!(
                format_line(&RollLine::Announce(p), Some("Bob"), false, LINK, false),
                None,
                "roll_number {} roll_type {}",
                p.roll_number,
                p.roll_type
            );
        }

        // The won line carries the roll number, and Need on `rollType == 1`.
        let mut won = LootRollWon {
            looted_target: 0xAA,
            item_slot: 0,
            item_id: 17182,
            random_property_id: 0,
            winner: 1,
            roll_number: 84,
            roll_type: roll_vote::NEED,
        };
        assert_eq!(
            format_line(&RollLine::Won(won), Some("Bob"), false, LINK, false),
            Some(format!("Bob won: {LINK} |cff818181(Need - 84)|r"))
        );
        assert_eq!(
            format_line(&RollLine::Won(won), None, true, LINK, false),
            Some(format!("You won: {LINK} |cff818181(Need - 84)|r"))
        );
        won.roll_type = roll_vote::GREED;
        assert_eq!(
            format_line(&RollLine::Won(won), Some("Bob"), false, LINK, false),
            Some(format!("Bob won: {LINK} |cff818181(Greed - 84)|r"))
        );
        // Anything but Need reads Greed, even a Pass-typed win.
        won.roll_type = roll_vote::PASS;
        assert!(format_line(&RollLine::Won(won), None, true, LINK, false)
            .unwrap()
            .contains("(Greed - 84)"));

        // All-passed is never gated: `0x61b640` does not read the CVar.
        let passed = LootAllPassed {
            looted_target: 0xAA,
            item_slot: 0,
            item_id: 17182,
            random_property_id: 0,
        };
        assert_eq!(
            format_line(&RollLine::AllPassed(passed), None, false, LINK, false),
            Some(format!("Everyone passed on: {LINK}"))
        );
    }

    #[test]
    fn start_lists_the_roll_with_monotonic_ids_and_announces_nothing() {
        let mut r = LootRolls::default();
        r.start(start(0xAA, 0, 17182));
        r.start(start(0xAA, 1, 4306));
        assert_eq!(r.active.len(), 2);
        assert_eq!(r.active[0].roll_id, 1);
        assert_eq!(r.active[1].roll_id, 2);
        // The countdown stays whole, as `START_LOOT_ROLL`'s `arg2`; the announce waits.
        for roll in &r.active {
            assert_eq!(roll.countdown_ms, 60_000);
            assert_eq!(roll.remaining_ms, 60_000);
            assert!(
                !roll.announced,
                "the packet does not announce — the template does"
            );
            assert_eq!(roll.held_ms, 0);
        }
    }

    /// vmangos can re-send every active roll on reconnect (`SendLootStartRollsForPlayer`).
    #[test]
    fn duplicate_start_is_ignored() {
        let mut r = LootRolls::default();
        r.start(start(0xAA, 0, 17182));
        r.start(start(0xAA, 0, 17182));
        assert_eq!(r.active.len(), 1);
        assert_eq!(r.next_id, 1, "and the duplicate burns no id");
    }

    #[test]
    fn resolution_closes_the_matching_roll_only() {
        let mut r = LootRolls::default();
        r.start(start(0xAA, 0, 17182));
        r.start(start(0xAA, 1, 4306));

        r.won(LootRollWon {
            looted_target: 0xAA,
            item_slot: 0,
            item_id: 17182,
            random_property_id: 0,
            winner: 1,
            roll_number: 84,
            roll_type: roll_vote::NEED,
        });
        assert_eq!(r.cancelled, vec![1]);
        assert_eq!(r.active.len(), 1);
        assert_eq!(r.active[0].item_slot, 1);

        r.all_passed(LootAllPassed {
            looted_target: 0xAA,
            item_slot: 1,
            item_id: 4306,
            random_property_id: 0,
        });
        assert_eq!(r.cancelled, vec![1, 2]);
        assert!(r.active.is_empty());
        assert_eq!(r.pending.len(), 2);
    }

    #[test]
    fn resolution_for_a_closed_roll_is_idempotent() {
        let mut r = LootRolls::default();
        r.start(start(0xAA, 0, 17182));
        let won = LootRollWon {
            looted_target: 0xAA,
            item_slot: 0,
            item_id: 17182,
            random_property_id: 0,
            winner: 1,
            roll_number: 84,
            roll_type: roll_vote::NEED,
        };
        r.won(won);
        r.won(won);
        assert_eq!(r.cancelled, vec![1], "only the first close cancels");
    }

    #[test]
    fn our_vote_closes_the_frame_and_yields_the_wire_identity() {
        let mut r = LootRolls::default();
        r.start(start(0xAA, 3, 17182));
        assert_eq!(r.vote(1), Some((0xAA, 3)));
        assert!(r.active.is_empty(), "the frame closes client-predicted");
        assert_eq!(r.cancelled, vec![1]);
        assert_eq!(r.vote(1), None);
        assert_eq!(r.vote(99), None);
    }

    #[test]
    fn tick_saturates_without_dropping_the_roll() {
        let mut r = LootRolls::default();
        r.start(start(0xAA, 0, 17182));
        r.tick(20_000);
        assert_eq!(r.active[0].remaining_ms, 40_000);
        r.tick(999_999);
        assert_eq!(r.active[0].remaining_ms, 0);
        assert_eq!(r.active.len(), 1, "the server closes it, not the tick");
    }

    #[test]
    fn tick_ages_the_hold_only_while_the_roll_is_unannounced() {
        let mut r = LootRolls::default();
        r.start(start(0xAA, 0, 17182));
        r.tick(500);
        r.tick(250);
        assert_eq!(r.active[0].held_ms, 750);
        r.active[0].announced = true;
        r.tick(1_000);
        assert_eq!(r.active[0].held_ms, 750, "the hold is over");
        assert_eq!(r.active[0].remaining_ms, 60_000 - 1_750, "the bar is not");
    }

    #[test]
    fn ids_are_never_reused() {
        let mut r = LootRolls::default();
        r.start(start(0xAA, 0, 17182));
        r.vote(1);
        r.start(start(0xBB, 0, 4306));
        assert_eq!(r.active[0].roll_id, 2);
    }

    // ── The template hold ──
    // Driven through a real schedule: the snapshot memo is a `Local`, and `run_system_once` would
    // hand every pass a fresh one, reading each as a first push.

    const ROLLED: u32 = 17182;

    /// A `feed_loot_rolls` app whose listener records what `GetLootRollItemInfo` answers as
    /// `START_LOOT_ROLL` fires: the call stock `GroupLootFrame_OnShow` makes.
    fn hold_app() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<LootRolls>()
            .init_resource::<Items>()
            .init_resource::<NameCache>()
            .init_resource::<SelfGuid>()
            .init_resource::<ChatLog>()
            .init_resource::<crate::ui_loot::LootConfig>()
            // `Time::default()` has a zero delta, so the hold only ages where a test says it does.
            .init_resource::<Time>()
            .insert_resource(NetCommands(tx))
            .add_systems(Update, feed_loot_rolls);

        let script = UiScript::new().unwrap();
        script
            .run(
                "STARTS = {}\n\
                 local f = CreateFrame(\"Frame\")\n\
                 f:RegisterEvent(\"START_LOOT_ROLL\")\n\
                 f:SetScript(\"OnEvent\", function()\n\
                   local _, name = GetLootRollItemInfo(arg1)\n\
                   tinsert(STARTS, arg1 .. \":\" .. arg2 .. \":\" .. (name or \"<blank>\"))\n\
                 end)",
            )
            .unwrap();
        app.insert_non_send_resource(script);
        (app, rx)
    }

    /// `rollID:rollTime:name` for each `START_LOOT_ROLL` seen so far, in order.
    fn starts(app: &mut App) -> Vec<String> {
        let script = app.world_mut().non_send_resource_mut::<UiScript>();
        let n = script.eval::<i64>("return getn(STARTS)").unwrap();
        (1..=n)
            .map(|i| {
                script
                    .eval::<String>(&format!("return STARTS[{i}]"))
                    .unwrap()
            })
            .collect()
    }

    fn advance(app: &mut App, ms: u64) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(ms));
    }

    fn sent(rx: &crossbeam_channel::Receiver<ClientCommand>) -> Vec<ClientCommand> {
        rx.try_iter().collect()
    }

    /// `0x61b310` ends in `0x55ba30(node+0x1c, node+0x10, callback 0x61b460, node, 0)`: a hit fires
    /// at once (`0x61b430`), a miss asks and fires on arrival. The negative answer releases the
    /// roll too, the module doc's deviation.
    #[test]
    fn the_roll_waits_for_its_item_template() {
        for (answer, painted) in [
            (Some(crate::items::test_template("Sulfuras")), "Sulfuras"),
            (None, "<blank>"),
        ] {
            let (mut app, rx) = hold_app();
            app.world_mut()
                .resource_mut::<LootRolls>()
                .start(start(0xAA, 0, ROLLED));

            // Pass one: the template is unknown, so nothing announces and the ask goes out.
            app.update();
            assert!(
                starts(&mut app).is_empty(),
                "no START_LOOT_ROLL while the template is pending"
            );
            assert!(
                sent(&rx).iter().any(
                    |c| matches!(c, ClientCommand::ItemQuery { entry, .. } if *entry == ROLLED)
                ),
                "and the template was asked for"
            );
            // The hold is not a one-shot.
            app.update();
            assert!(starts(&mut app).is_empty());

            // The answer lands: one announce, the wire's countdown as `arg2`, the name readable.
            app.world_mut()
                .resource_mut::<Items>()
                .insert_template(ROLLED, answer.clone());
            app.update();
            assert_eq!(starts(&mut app), vec![format!("1:60000:{painted}")]);
            app.update();
            assert_eq!(starts(&mut app).len(), 1, "and only once");
        }
    }

    /// `0x61b310`'s hit path: the roll announces on the pass the packet lands.
    #[test]
    fn a_cached_template_announces_the_same_pass() {
        let (mut app, _rx) = hold_app();
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(ROLLED, Some(crate::items::test_template("Sulfuras")));
        app.world_mut()
            .resource_mut::<LootRolls>()
            .start(start(0xAA, 0, ROLLED));
        app.update();
        assert_eq!(starts(&mut app), vec!["1:60000:Sulfuras".to_string()]);
    }

    /// The module doc's deviation: an unanswered template opens the frame at the deadline.
    #[test]
    fn an_unanswered_template_opens_the_roll_at_the_deadline() {
        let (mut app, _rx) = hold_app();
        app.world_mut()
            .resource_mut::<LootRolls>()
            .start(start(0xAA, 0, ROLLED));

        // Two steps to one millisecond short of the budget: still holding.
        for _ in 0..2 {
            advance(&mut app, (TEMPLATE_HOLD_MAX_MS / 2 - 1) as u64);
            app.update();
        }
        assert!(
            starts(&mut app).is_empty(),
            "still holding at {} ms",
            TEMPLATE_HOLD_MAX_MS - 2
        );

        // The step that crosses it opens the frame, blank but votable.
        advance(&mut app, 2);
        app.update();
        assert_eq!(starts(&mut app), vec!["1:60000:<blank>".to_string()]);
        advance(&mut app, TEMPLATE_HOLD_MAX_MS as u64);
        app.update();
        assert_eq!(starts(&mut app).len(), 1, "and only once");
    }

    /// A held roll's `CANCEL_LOOT_ROLL` still goes out, as `0x61b9e0` checks only the node's
    /// closed byte; the stock Lua matches no frame to it.
    #[test]
    fn a_roll_resolved_while_held_never_announces() {
        let (mut app, _rx) = hold_app();
        app.world_mut()
            .resource_mut::<LootRolls>()
            .start(start(0xAA, 0, ROLLED));
        app.update();
        assert!(starts(&mut app).is_empty());

        app.world_mut()
            .resource_mut::<LootRolls>()
            .all_passed(LootAllPassed {
                looted_target: 0xAA,
                item_slot: 0,
                item_id: ROLLED,
                random_property_id: 0,
            });
        // Even once the template lands, the roll it belonged to is gone.
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(ROLLED, Some(crate::items::test_template("Sulfuras")));
        app.update();
        app.update();
        assert!(
            starts(&mut app).is_empty(),
            "a roll that closed while held opens no frame"
        );
    }
}
