//! The app-side **gossip feed** (decision 0081 phase 3) — the inward half of the gossip seam around
//! [`benilla_ui::script`]'s `gossip` module, the exact twin of [`crate::ui_items`]'s container seam.
//!
//! The net bridge fills [`GossipState`] from the wire (`SMSG_GOSSIP_MESSAGE` → options + an ask-once
//! `CMSG_NPC_TEXT_QUERY` for the greeting; `SMSG_NPC_TEXT_UPDATE` → the greeting; `SMSG_GOSSIP_COMPLETE`
//! → cleared). Each frame [`feed_gossip`] turns that state into a [`GossipMenu`] snapshot, pushes it
//! into the VM ([`benilla_ui::script::UiScript::set_gossip`]), and fires `GOSSIP_SHOW` on open (or a
//! content change while open — the NPC's name resolving) / `GOSSIP_CLOSED` on clear.
//! [`drain_gossip`] pulls the Lua intents back out: `SelectGossipOption` →
//! [`ClientCommand::GossipSelectOption`] (mapped through the wire option `index`; coded options are
//! guarded, never sent — decision 0081), and `CloseGossip` → a local clear (vanilla's client-side
//! close sends no packet; verified against the 1.12 opcode set — there is no `CMSG_GOSSIP_CLOSE`).
//!
//! **The menu opens only with its greeting resolved, and the hold fires nothing** — a first visit
//! to a text id keeps the frame exactly as it was for the query round trip: hidden if it was
//! hidden (never options over an empty page — B292, decision 1508), and **still painted with the
//! previous menu if it was open** (a sub-menu's first visit repaints in place when its text lands,
//! never hides and re-shows the window — decision 1994). Both are the reference's own law: on a
//! cache miss `0x4e2010` sets its select latch and returns — no greeting write, no event — and its
//! greeting write and `GOSSIP_SHOW` are adjacent and unconditional on the one success path, so
//! "gossip frame open with a blank greeting" is not a reachable state, and neither is "gossip frame
//! hidden by a pending reply". [`GossipState::text_pending`] is the latch; [`feed_gossip`] fires
//! nothing while it holds; [`GossipState::open_menu`]/[`GossipState::text_arrived`] are the two
//! wire edges that set and resolve it.

use std::collections::HashMap;

use benilla_protocol::messages::{select_greeting, GossipOption, NpcTextBlock};
use bevy::prelude::*;

use benilla_ui::script::{GossipMenu, GossipOptionView, GossipQuestRow, ScriptValue, UiScript};

use crate::names::NameCache;
use crate::net::{ClientCommand, Guid, NetCommands, ObjectStore, SelfPlayer};
use crate::ui_quest::{row_is_active, row_is_one_click};
use crate::ui_script::{UiFeed, UiInput};
use crate::ui_session::{close_npc_session_out_of_range, npc_switched, NpcSession};

/// The open gossip menu, filled by the net bridge ([`crate::net`]) and read by [`feed_gossip`]. The
/// ask-once `records` cache keyed by `text_id` (mirroring [`crate::items::Items`]'s template cache)
/// means a revisit to the same NPC never re-queries the text. Cleared on `SMSG_GOSSIP_COMPLETE`,
/// a client-side close, and disconnect; the record cache survives (NPC text is static per id).
///
/// The cache holds the **record**, not a greeting. Which of its 8 blocks greets you is drawn per
/// menu-open from the NPC's gender and a die roll (`benilla_protocol::messages::select_greeting`),
/// exactly as the reference re-draws it in its gossip handler — so the same `text_id` can greet you
/// differently on the next visit, and two NPCs sharing a `text_id` can differ by gender. Caching a
/// resolved string, as this did, froze the first draw for the whole session.
#[derive(Resource, Default)]
pub(crate) struct GossipState {
    /// The NPC whose menu is open; `None` = no menu open.
    pub(crate) npc: Option<u64>,
    /// The open menu's `NpcText` id (drives the text query / cache).
    pub(crate) text_id: u32,
    /// The greeting drawn for THIS menu-open, or `None` while its query is in flight — the state
    /// [`GossipState::text_pending`] reads, on which the feed fires nothing and the drains refuse
    /// selects (B292; module doc). A record that names no line ("Missing gossip text!") never
    /// parks here: [`GossipState::resolve_greeting`] ends the interaction instead, as the
    /// reference does.
    pub(crate) greeting: Option<String>,
    /// The selectable option rows (wire `GossipOption`: `index` echoed on select, `icon`, `coded`,
    /// `message`).
    pub(crate) options: Vec<GossipOption>,
    /// The quest rows riding the same packet — `(quest_id, dialog-status icon, level, title)`.
    /// The level joined the tuple with 1751 window 21: the reference's own
    /// `GetGossipAvailableQuests`/`GetGossipActiveQuests` return `(title, level)` PAIRS and stock
    /// `GossipFrame.lua` strides its walk by 2 over them, so a dropped level is a broken stride
    /// rather than a missing number. It was on the wire all along. The gossip
    /// window lists them above the options; a click sends `CMSG_QUESTGIVER_QUERY_QUEST` /
    /// `_COMPLETE_QUEST` (decision 0088).
    pub(crate) quests: Vec<(u32, u32, u32, String)>,
    /// Ask-once NPC-text record cache keyed by `text_id` — the 8 undrawn blocks.
    records: HashMap<u32, Vec<NpcTextBlock>>,
}

impl GossipState {
    /// The cached record for `text_id`, if `CMSG_NPC_TEXT_QUERY` was already answered for it.
    pub(crate) fn cached_record(&self, text_id: u32) -> Option<&[NpcTextBlock]> {
        self.records.get(&text_id).map(Vec::as_slice)
    }

    /// Record an NPC-text answer (`SMSG_NPC_TEXT_UPDATE`) into the ask-once cache.
    pub(crate) fn remember_record(&mut self, text_id: u32, blocks: Vec<NpcTextBlock>) {
        self.records.insert(text_id, blocks);
    }

    /// Draw this menu-open's greeting out of `text_id`'s cached record for an NPC of `npc_gender`
    /// — `None` if the record hasn't arrived yet, or names no line for that gender.
    pub(crate) fn draw_greeting(&self, text_id: u32, npc_gender: u8) -> Option<String> {
        let blocks = self.cached_record(text_id)?;
        select_greeting(blocks, npc_gender, greeting_roll()).map(str::to_string)
    }

    /// The reference's text-pending latch (`[0xbbb670]`): a
    /// session is latched but its greeting has not resolved — its `CMSG_NPC_TEXT_QUERY` is in
    /// flight, or (`text_id == 0`) was never sent. While it holds, [`feed_gossip`] fires nothing
    /// and both drains refuse every select (decisions 1508, 1994).
    pub(crate) fn text_pending(&self) -> bool {
        self.npc.is_some() && self.greeting.is_none()
    }

    /// A gossip menu arrived (`SMSG_GOSSIP_MESSAGE`): latch the session. Returns `true` when the
    /// greeting still needs its `CMSG_NPC_TEXT_QUERY` (first visit to this text id) — the caller
    /// sends it, and the frame keeps whatever it shows until [`Self::text_arrived`] resolves it
    /// (the hold, module doc). A cached record resolves the menu right here; a `text_id` of 0
    /// never queries and never resolves — the reference's `DBCache::Get` refuses id 0 before
    /// sending anything, so its frame stays as it was with option selects latched off, which is
    /// what our pending state is.
    pub(crate) fn open_menu(
        &mut self,
        npc: u64,
        text_id: u32,
        options: Vec<GossipOption>,
        quests: Vec<(u32, u32, u32, String)>,
        npc_gender: u8,
    ) -> bool {
        self.npc = Some(npc);
        self.text_id = text_id;
        self.options = options;
        self.quests = quests;
        self.greeting = None;
        if text_id == 0 {
            return false;
        }
        match self.cached_record(text_id).is_some() {
            true => {
                self.resolve_greeting(npc_gender);
                false
            }
            false => true,
        }
    }

    /// The NPC-text answer (`SMSG_NPC_TEXT_UPDATE`): seed the ask-once cache, and if it answers
    /// the menu still waiting on it, open that menu now. A late answer for a menu we already
    /// closed or switched away from just seeds the cache; an answer for a menu already open does
    /// not re-roll its greeting (the reference's cache callback only re-enters for the pending
    /// query it was registered by).
    pub(crate) fn text_arrived(&mut self, text_id: u32, blocks: Vec<NpcTextBlock>, npc_gender: u8) {
        self.remember_record(text_id, blocks);
        if self.npc.is_some() && self.text_id == text_id && self.greeting.is_none() {
            self.resolve_greeting(npc_gender);
        }
    }

    /// Draw the open menu's greeting from its (present) record — or, when the record names no
    /// line for this NPC's gender column, end the interaction: the reference's `missing` path
    /// logs "Missing gossip text!" and fires `GOSSIP_CLOSED` instead of ever opening the frame
    /// (`0x4e216e`). Ours never opened either, so the clear is the whole
    /// close.
    fn resolve_greeting(&mut self, npc_gender: u8) {
        match self.draw_greeting(self.text_id, npc_gender) {
            Some(line) => self.greeting = Some(line),
            None => {
                debug!(
                    "ui_gossip: missing gossip text (id {}) — ending the interaction",
                    self.text_id
                );
                self.clear();
            }
        }
    }
}

/// The reference's uniform float in `[1.0, 2.0)` for the greeting draw: it stuffs PRNG bits straight
/// into a mantissa (`(rand & 0x7fffff) | 0x3f800000`), which is what makes the range `[1, 2)` rather
/// than the `[0, 1)` you would expect. We build the same shape from our own source — the *stream*
/// can't match (it is the client's own PRNG, seeded per run) but the distribution is what the law
/// needs.
fn greeting_roll() -> f32 {
    f32::from_bits((rand::random::<u32>() & 0x7f_ffff) | 0x3f80_0000)
}

impl GossipState {
    /// Close the open menu (`SMSG_GOSSIP_COMPLETE` / a client-side close). Keeps the greeting cache.
    pub(crate) fn clear(&mut self) {
        self.npc = None;
        self.text_id = 0;
        self.greeting = None;
        self.options.clear();
        self.quests.clear();
    }

    /// Disconnect: drop the open menu (the greeting cache is static, like item templates — kept),
    /// mirroring [`crate::items::Items::clear_session`].
    pub(crate) fn clear_session(&mut self) {
        self.clear();
    }
}

mod net;
pub(crate) use net::gossip_complete as end_interaction;

/// The gossip window's feed + drain (decision 0081), cloned from [`crate::ui_items::UiItemsPlugin`].
pub(crate) struct UiGossipPlugin;

impl Plugin for UiGossipPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<GossipState>().add_systems(
            Update,
            (
                // Range-close before the feed so the clear turns into GOSSIP_CLOSED the same frame;
                // push before the input pass so an open/close is on screen the same frame; drain
                // after it so a click's intent goes out the same frame (mirrors ui_items).
                close_npc_session_out_of_range::<GossipState>.before(feed_gossip),
                feed_gossip.in_set(UiFeed),
                drain_gossip.after(UiInput),
            ),
        );
    }
}

/// The gossip menu is an NPC session: the standardized range guard ([`crate::ui_session`])
/// client-side-closes it — the exact `CloseGossip` clear, no packet — when the player walks out of
/// the NPC's service range or the NPC despawns.
impl NpcSession for GossipState {
    fn npc(&self) -> Option<u64> {
        self.npc
    }

    fn close(&mut self) {
        self.clear();
    }
}

/// The lowercase Era `GetGossipOptions()` icon *type* for a wire `GOSSIP_ICON` byte — the value
/// the XML resolves to a `Interface\GossipFrame\<Type>GossipIcon` texture.
///
/// **The byte is a bare index into [`GOSSIP_ICON_TYPES`], with no bounds check anywhere on the
/// client's path** (decision 1335): `GetGossipOptions 0x4e28d0` reads the stored byte and does
/// `mov edx,[eax*4 + 0x84b7ac]` straight into a 14-entry pointer table, then `lua_pushstring`s it.
///
/// The old map read the byte through **vmangos's `GossipDef.h` enum *names*** (`INTERACT_1`,
/// `MONEY_BAG`, `TALK`, …), which name a **later** client's icon art. It was wrong for six of the
/// eleven values; 5 — the icon every `GOSSIP_OPTION_INNKEEPER` row in the world DB sends — had no
/// entry at all and fell through to the chat bubble, which is the icon half of B249 (decision 1331).
///
/// Two of the reference's out-of-range behaviours we deliberately do **not** reproduce, because
/// both are its missing guard rather than its design:
/// index **14** is a NULL in the table, so the reference pushes `nil` as the option's type and the
/// row draws whatever the XML's own fallback gives it — the chat bubble, which is what we return
/// directly. Index **≥15** walks off the end into the gossip Lua *binding* table, pushing API-name
/// literals and `strlen`'d `.text` addresses as type strings, and eventually faults. We clamp.
fn gossip_icon_type(icon: u8) -> &'static str {
    GOSSIP_ICON_TYPES
        .get(icon as usize)
        .copied()
        .unwrap_or("gossip")
}

/// The client's icon-name table verbatim — `0x84b7ac`, 14 entries, indexed by the wire
/// `GOSSIP_ICON` byte (see [`gossip_icon_type`]). Indices 11-13 are genuine `gossip` aliases in the
/// binary, not padding.
///
/// Ten of the eleven distinct names have a `Interface\GossipFrame\<Type>GossipIcon.blp` behind
/// them on the 5875 chain; **`auctioneer` does not** — the table names art the client never shipped
/// (auction houses came with 1.9, the icon later). [`gossip_icon_types_name_the_shipped_art`] pins
/// both halves of that, and the XML deliberately has no `auctioneer` key: a missing texture draws
/// nothing, our fallback draws the bubble, and the bubble is the better wrong answer.
const GOSSIP_ICON_TYPES: [&str; 14] = [
    "gossip",
    "vendor",
    "taxi",
    "trainer",
    "healer",
    "binder",
    "banker",
    "petition",
    "tabard",
    "battlemaster",
    "auctioneer",
    "gossip",
    "gossip",
    "gossip",
];

/// Build the Lua-facing snapshot from [`GossipState`] — `None` when no menu is open. The second
/// `?` is the hold's type-level edge (a `GossipMenu` always has its greeting — B292, module doc);
/// [`feed_gossip`] gates on [`GossipState::text_pending`] before ever calling this, so a `None`
/// it sees means no session, never "pending" — a pending frame keeps its last snapshot.
fn snapshot(state: &GossipState) -> Option<GossipMenu> {
    state.npc?;
    Some(GossipMenu {
        greeting: state.greeting.clone()?,
        // Quest rows riding the gossip packet (decision 0088): split active-vs-available by the same
        // predicate the quest window's greeting panel uses — the WIRE ICON. The reference runs the
        // identical `{3,4}` test here, just lazily (`0x4e2430`/`0x4e2580`, behind
        // `GetGossipAvailableQuests`/`GetGossipActiveQuests`) rather than at parse time. Decision 0758.
        quests: state
            .quests
            .iter()
            .map(|(_id, icon, level, title)| GossipQuestRow {
                title: title.clone(),
                level: *level,
                active: row_is_active(*icon),
            })
            .collect(),
        options: state
            .options
            .iter()
            .map(|o| GossipOptionView {
                label: o.message.clone(),
                icon_type: gossip_icon_type(o.icon).into(),
                coded: o.coded,
            })
            .collect(),
    })
}

/// Push the current menu into the VM and fire the open/close events on a transition (or a content
/// change — the greeting arriving a frame after the menu). Diffed against a `Local` memory, exactly
/// like the container feed's per-bag diff.
fn feed_gossip(
    script: Option<NonSendMut<UiScript>>,
    state: Res<GossipState>,
    self_q: Query<(&ObjectStore, &Guid), With<SelfPlayer>>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    states: Res<crate::world_state::WorldStates>,
    mut last: Local<crate::ui_script::VmMemo<Option<GossipMenu>>>,
    mut last_name: Local<crate::ui_script::VmMemo<Option<String>>>,
    mut last_npc: Local<crate::ui_script::VmMemo<Option<u64>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let last = last.get(&script);
    let last_name = last_name.get(&script);
    let last_npc = last_npc.get(&script);
    // The gossip NPC's name resolves through the NameCache (ask-once — the merchant feed's pattern),
    // delivered as arg1 of GOSSIP_SHOW; `None`/empty while the query is in flight, and the diff below
    // tracks it so a name-only landing still re-fires GOSSIP_SHOW to repaint the title. It rides an
    // event arg rather than a `GossipMenu` field so no benilla-ui engine change is needed for the title.
    // Asked ahead of the hold, so a first visit's name query goes out beside its text query rather
    // than a round trip after it.
    let npc_name = state
        .npc
        .and_then(|g| names.resolve(g, &commands).map(str::to_string));
    // **The hold fires nothing** (decision 1994). While the greeting query is in flight the
    // reference's handler has set its select latch and RETURNED — no greeting write, no event
    // (`0x4e2010`, exit `0x4e2068`) — so its frame keeps whatever it last painted:
    // hidden if it was hidden, the previous menu if it was open, clicks refused (`drain_gossip`).
    // A pending menu is therefore NOT a closed one: the VM keeps its last snapshot, `last` keeps
    // its memory, and the text landing is a plain in-place `GOSSIP_SHOW` below. Reading the hold
    // as a close (the snapshot going `Some → None`) hid the open window for the round trip and
    // re-showed it when the text answered — the flash on a sub-menu's first visit, first visit
    // only because the record cache serves every later one without a hold.
    if state.text_pending() {
        return;
    }
    let mut fresh = snapshot(&state);
    // Expand the greeting's chat-text macros ($N/$B/$G/$<n>w) client-side, as the real client does.
    if let Some(greeting) = fresh.as_mut().map(|m| &mut m.greeting) {
        let player = crate::npc_text::player_identity(&self_q, &names, &commands);
        *greeting = crate::npc_text::substitute(
            greeting,
            &crate::npc_text::MacroContext {
                subject: player.as_ref(),
                states: &states,
            },
        );
    }
    let name_changed = *last_name != npc_name;
    // A different NPC while the menu is already open is a real close+open (decision 0096 /
    // [`crate::ui_session::npc_switched`]); a cross-window switch is handled by OnHide → CloseX on
    // panel displacement (decision 0095). Past the hold, `fresh` is `Some` exactly when a session
    // is latched, so the shown menu's NPC is the session's — and a switch INTO a first-visit hold
    // (open A → pending B) is judged here when B's text answers, as one close+open pair; nothing
    // fires at the pending edge.
    let switched = npc_switched(*last_npc, state.npc);
    if fresh == *last && !name_changed && !switched {
        return;
    }
    script.set_gossip(fresh.clone());
    let name_arg = || vec![ScriptValue::Str(npc_name.clone().unwrap_or_default())];
    if switched {
        // Close the old NPC's menu, open the new: the frame hides then shows, playing both kits.
        // GOSSIP_CLOSED routes through OnHide → CloseGossip (decision 0095), which queues a close
        // intent — consume it so the drain does not clear the menu we just re-opened.
        script.fire_event("GOSSIP_CLOSED", vec![]);
        script.fire_event("GOSSIP_SHOW", name_arg());
        let _ = script.take_gossip_close();
    } else {
        match (&*last, &fresh) {
            // Opened, or the greeting/options/name changed while open → (re)paint via GOSSIP_SHOW.
            (_, Some(_)) => script.fire_event("GOSSIP_SHOW", name_arg()),
            // Closed — the session is over (`clear`). OnHide → CloseGossip queues a close intent
            // for a session already gone; the drain's clear is a no-op, so nothing to consume.
            (Some(_), None) => script.fire_event("GOSSIP_CLOSED", vec![]),
            (None, None) => {}
        }
    }
    *last = fresh;
    *last_name = npc_name;
    *last_npc = state.npc;
}

/// Drain the Lua intents: a selected option → `CMSG_GOSSIP_SELECT_OPTION` (mapped to the wire option
/// `index`; a coded option is guarded and never sent — decision 0081); a close → a local clear (no
/// packet, vanilla).
fn drain_gossip(
    script: Option<NonSendMut<UiScript>>,
    mut state: ResMut<GossipState>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    for pos in script.take_gossip_selects() {
        let Some(npc) = state.npc else { continue };
        // While the greeting query is in flight — the frame may still show the previous menu
        // (1994) — refuse the select, as the reference does: its `SelectGossipOption` (`0x4e2320`)
        // is silently refused while the text-pending latch `0xbbb670` is set.
        if state.text_pending() {
            debug!(
                "ui_gossip: SelectGossipOption({pos}) while the text query is in flight — refused"
            );
            continue;
        }
        // Lua position is 1-based; resolve it to the option's wire `index`.
        match pos
            .checked_sub(1)
            .and_then(|i| state.options.get(i as usize))
        {
            Some(opt) if !opt.coded => {
                debug!("ui_gossip: select option {} (index {})", pos, opt.index);
                let _ = commands.0.send(ClientCommand::GossipSelectOption {
                    guid: npc,
                    option: opt.index,
                });
            }
            Some(_) => debug!("ui_gossip: ignoring coded option {pos} (v1 greys it)"),
            None => debug!("ui_gossip: SelectGossipOption({pos}) out of range — ignored"),
        }
    }
    // Quest-row clicks (decision 0088): map the 1-based row to its quest id and send
    // QUERY_QUEST (available → look at/accept) or COMPLETE_QUEST (active → turn-in progress).
    for pos in script.take_gossip_quest_selects() {
        let Some(npc) = state.npc else { continue };
        // Same session-pending rule as the option selects above (the latch there is the verified
        // one; these rows belong to the same not-yet-resolved menu).
        if state.text_pending() {
            debug!(
                "ui_gossip: SelectGossipQuest({pos}) while the text query is in flight — refused"
            );
            continue;
        }
        match pos
            .checked_sub(1)
            .and_then(|i| state.quests.get(i as usize))
        {
            Some((quest_id, icon, _level, _title)) => {
                let (quest, icon) = (*quest_id, *icon);
                let active = row_is_active(icon);
                // Same opcode law as the greeting panel: an active row is always a turn-in; an
                // available row turns into one only when its one-click flag is set.
                let cmd = if active || row_is_one_click(icon) {
                    ClientCommand::QuestgiverComplete { npc, quest }
                } else {
                    ClientCommand::QuestgiverQuery { npc, quest }
                };
                debug!("ui_gossip: quest row {pos} (quest {quest}, icon {icon}, active {active})");
                let _ = commands.0.send(cmd);
            }
            None => debug!("ui_gossip: SelectGossipQuest({pos}) out of range — ignored"),
        }
    }
    if script.take_gossip_close() {
        debug!("ui_gossip: client-side close (no packet)");
        state.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one type string [`GOSSIP_ICON_TYPES`] names that the 5875 install ships no art for.
    /// Test-only: nothing in the client needs to know, because the XML simply has no key for it.
    const GOSSIP_ICON_TYPE_UNSHIPPED: &str = "auctioneer";

    #[test]
    fn snapshot_is_none_until_a_menu_opens() {
        let mut state = GossipState::default();
        assert!(snapshot(&state).is_none());
        state.npc = Some(0x42);
        state.greeting = Some("Hello".into());
        state.options = vec![GossipOption {
            index: 3,
            icon: 1,
            coded: false,
            message: "Browse".into(),
        }];
        let menu = snapshot(&state).expect("open");
        assert_eq!(menu.greeting, "Hello");
        assert_eq!(menu.options.len(), 1);
        assert_eq!(menu.options[0].icon_type, "vendor");
    }

    /// B292's hold: a first visit (record not cached) keeps the menu CLOSED for the query round
    /// trip — options never render over an empty page — and the text arriving opens it complete.
    /// The reference's law (`0x4e229a`/`0x4e22b0`): the greeting write
    /// and `GOSSIP_SHOW` are adjacent and unconditional on one success path; every other exit
    /// fires no event.
    #[test]
    fn first_visit_holds_the_menu_until_the_text_answers() {
        let mut state = GossipState::default();
        let options = vec![GossipOption {
            index: 0,
            icon: 9,
            coded: false,
            message: "I wish to join the battle!".into(),
        }];
        let wants_query = state.open_menu(0x42, 50, options, Vec::new(), 0);
        assert!(wants_query, "first visit asks for the text");
        assert!(
            snapshot(&state).is_none(),
            "menu held closed while the query is in flight"
        );

        state.text_arrived(
            50,
            vec![NpcTextBlock {
                probability: 0.0,
                male: "The Alliance needs you!".into(),
                female: String::new(),
            }],
            0,
        );
        let menu = snapshot(&state).expect("text landed — the menu opens now, complete");
        assert_eq!(menu.greeting, "The Alliance needs you!");
        assert_eq!(menu.options.len(), 1, "options open WITH the greeting");
    }

    // ── The hold fires nothing (decision 1994): the real feed, driven ─────────────────────────

    /// A Lua event recorder for the two gossip events — what the VM saw, in order.
    const RECORDER: &str = r#"
        SEEN = {}
        local f = CreateFrame("Frame")
        f:RegisterEvent("GOSSIP_SHOW")
        f:RegisterEvent("GOSSIP_CLOSED")
        f:SetScript("OnEvent", function() table.insert(SEEN, event) end)
    "#;

    /// The real [`feed_gossip`] + [`drain_gossip`] over a VM — the [`crate::ui_quest`] harness
    /// shape: a minimal `App` with the feed's resources, the script as the non-send resource,
    /// the two systems in their plugin order. `prepare` seeds the VM (the recorder, or the stock
    /// window); the receiver is what the drain sent.
    fn feed_app(
        prepare: impl FnOnce(&mut UiScript),
    ) -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<GossipState>()
            .init_resource::<NameCache>()
            .init_resource::<crate::world_state::WorldStates>()
            .add_systems(Update, (feed_gossip, drain_gossip).chain());
        let mut script = UiScript::new().unwrap();
        prepare(&mut script);
        app.insert_non_send_resource(script);
        (app, rx)
    }

    fn state(app: &mut App) -> Mut<'_, GossipState> {
        app.world_mut().resource_mut::<GossipState>()
    }

    /// One frame: run the feed, then what the VM saw — the recorder's `SEEN` (drained) and
    /// `GetGossipText()` (`""` for nil).
    fn tick(app: &mut App) -> (Vec<String>, String) {
        app.update();
        let s = app.world_mut().non_send_resource_mut::<UiScript>();
        assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
        let seen = s.eval::<Vec<String>>("return SEEN").unwrap();
        s.run("SEEN = {}").unwrap();
        let text = s.eval::<String>("return GetGossipText() or ''").unwrap();
        (seen, text)
    }

    fn record(line: &str) -> Vec<NpcTextBlock> {
        vec![NpcTextBlock {
            probability: 0.0,
            male: line.into(),
            female: String::new(),
        }]
    }

    fn option(label: &str) -> Vec<GossipOption> {
        vec![GossipOption {
            index: 0,
            icon: 0,
            coded: false,
            message: label.into(),
        }]
    }

    /// The selects the drain actually sent, out of everything on the channel (the feed's own
    /// ask-once name query rides it too).
    fn selects_sent(rx: &crossbeam_channel::Receiver<ClientCommand>) -> usize {
        rx.try_iter()
            .filter(|c| matches!(c, ClientCommand::GossipSelectOption { .. }))
            .count()
    }

    const GUARD: u64 = 0xF130_0000_0000_0007; // HIGHGUID_UNIT

    /// **The hold fires nothing** (decision 1994) — the director's *"the whole window flashes
    /// before it shows the content"* on a Stormwind guard's *"Where is …"* option, first click
    /// only. The real feed is driven through the sequence the wire produces: the greeting menu
    /// open from the cache, then the select's answer — a NEW `SMSG_GOSSIP_MESSAGE` on the SAME NPC
    /// whose text id is a first visit, so the `CMSG_NPC_TEXT_QUERY` round trip — then its
    /// `SMSG_NPC_TEXT_UPDATE`.
    ///
    /// Before 1994 the pending step fired `GOSSIP_CLOSED` (the snapshot went `Some → None` and
    /// the feed read that as a close) and the landing fired `GOSSIP_SHOW`: `HideUIPanel` then
    /// `ShowUIPanel` across a server round trip — the flash. First click only, because the record
    /// cache serves the second visit with no hold. The reference fires nothing at the pending
    /// step (`0x4e2010`, exit `0x4e2068`): its frame keeps the previous menu painted,
    /// selects refused, until the text answers — then one in-place repaint.
    #[test]
    fn the_hold_fires_nothing_a_pending_submenu_keeps_the_frame_painted() {
        let (mut app, rx) = feed_app(|s| s.run(RECORDER).unwrap());

        // The greeting menu, its text already cached: opens at once.
        {
            let mut st = state(&mut app);
            st.remember_record(50, record("What are you looking for?"));
            assert!(!st.open_menu(GUARD, 50, option("The bank"), Vec::new(), 0));
        }
        assert_eq!(
            tick(&mut app),
            (
                vec!["GOSSIP_SHOW".to_string()],
                "What are you looking for?".to_string()
            )
        );

        // "The bank" is clicked; the answer is a new menu on the same NPC with a text id never
        // seen — the query is owed, and for the whole round trip NOTHING fires: the first menu
        // stays in the VM, and a click on one of its (stale) rows is refused, not sent.
        assert!(
            state(&mut app).open_menu(GUARD, 51, option("Goodbye"), Vec::new(), 0),
            "first visit to the sub-menu's text: the query is owed"
        );
        for _ in 0..3 {
            assert_eq!(
                tick(&mut app),
                (Vec::new(), "What are you looking for?".to_string()),
                "the hold fires nothing — the previous menu stays painted"
            );
        }
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("SelectGossipOption(1)")
            .unwrap();
        let _ = tick(&mut app);
        assert_eq!(
            selects_sent(&rx),
            0,
            "a select during the hold is refused (the latch)"
        );

        // The text lands: one in-place repaint, never a close — and selects work again.
        state(&mut app).text_arrived(51, record("The bank is north of here."), 0);
        assert_eq!(
            tick(&mut app),
            (
                vec!["GOSSIP_SHOW".to_string()],
                "The bank is north of here.".to_string()
            )
        );
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("SelectGossipOption(1)")
            .unwrap();
        let _ = tick(&mut app);
        assert_eq!(selects_sent(&rx), 1);

        // Controls. A real end of the session still closes …
        state(&mut app).clear();
        assert_eq!(
            tick(&mut app),
            (vec!["GOSSIP_CLOSED".to_string()], String::new())
        );
        // … and a first-ever visit from a closed frame still holds shut (B292) and opens once,
        // complete, when its text answers.
        assert!(state(&mut app).open_menu(GUARD, 52, option("Goodbye"), Vec::new(), 0));
        assert_eq!(
            tick(&mut app),
            (Vec::new(), String::new()),
            "hidden stays hidden through the hold"
        );
        state(&mut app).text_arrived(52, record("Yes?"), 0);
        assert_eq!(
            tick(&mut app),
            (vec!["GOSSIP_SHOW".to_string()], "Yes?".to_string())
        );
    }

    /// The same sequence through the stock `GossipFrame.xml` (the reference's own file, 1751) —
    /// the symptom at the window: across the round trip it stays **visible**, plays no close kit,
    /// and still lists the first menu's row; the text landing repaints in place, and since
    /// `ShowUIPanel` early-returns on a visible frame (0096's own mechanism) that plays no open
    /// kit either. Needs client data.
    #[test]
    fn stock_gossip_frame_stays_visible_through_a_first_visit_submenu() {
        use benilla_ui::script::SoundRequest;
        let _data = benilla_formats::wow_data_or_skip!();
        let (mut app, _rx) = feed_app(|s| {
            s.set_screen_size(1024.0, 768.0);
            s.set_text_measurer(Box::new(crate::ui_script::FixedWidthFont(6.0)));
            for f in crate::ui_script::test_ui::GOSSIP_UI {
                crate::ui_script::test_ui::load_ui(s, f);
            }
            crate::ui_script::test_ui::load_ui(s, r"Interface\FrameXML\GossipFrame.xml");
            s.run(RECORDER).unwrap();
        });
        // What the window shows: visible?, the greeting, the first row's label, the kits played.
        let window = |app: &mut App| -> (bool, String, String, Vec<SoundRequest>) {
            let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
            (
                s.eval::<bool>("return GossipFrame:IsVisible()").unwrap(),
                s.eval::<String>("return GossipGreetingText:GetText() or ''")
                    .unwrap(),
                s.eval::<String>("return GossipTitleButton1:GetText() or ''")
                    .unwrap(),
                s.take_sounds(),
            )
        };

        {
            let mut st = state(&mut app);
            st.remember_record(50, record("What are you looking for?"));
            st.open_menu(GUARD, 50, option("The bank"), Vec::new(), 0);
        }
        let _ = tick(&mut app);
        assert_eq!(
            window(&mut app),
            (
                true,
                "What are you looking for?".to_string(),
                "The bank".to_string(),
                vec![SoundRequest::KitName("igQuestListOpen".into())]
            ),
            "the greeting menu opens with its kit"
        );

        // The click's answer holds on its text: the window must not so much as blink.
        assert!(state(&mut app).open_menu(GUARD, 51, option("Goodbye"), Vec::new(), 0));
        for _ in 0..3 {
            let (seen, _) = tick(&mut app);
            assert!(seen.is_empty(), "nothing fires during the hold: {seen:?}");
            assert_eq!(
                window(&mut app),
                (
                    true,
                    "What are you looking for?".to_string(),
                    "The bank".to_string(),
                    Vec::new()
                ),
                "the window stays up, unchanged and silent, for the round trip — a hide here IS the flash"
            );
        }

        // The text lands: repainted in place — new greeting, new row, no kit.
        state(&mut app).text_arrived(51, record("The bank is north of here."), 0);
        let (seen, _) = tick(&mut app);
        assert_eq!(seen, vec!["GOSSIP_SHOW".to_string()]);
        assert_eq!(
            window(&mut app),
            (
                true,
                "The bank is north of here.".to_string(),
                "Goodbye".to_string(),
                Vec::new()
            ),
            "an in-place repaint: ShowUIPanel on a visible frame is a no-op, no kit"
        );
    }

    /// A revisit (record cached) opens immediately with no query: the empty frame is a
    /// first-visit-only state.
    #[test]
    fn revisit_opens_immediately_from_the_cache() {
        let mut state = GossipState::default();
        state.remember_record(
            50,
            vec![NpcTextBlock {
                probability: 0.0,
                male: "Back again?".into(),
                female: String::new(),
            }],
        );
        let wants_query = state.open_menu(0x42, 50, Vec::new(), Vec::new(), 0);
        assert!(!wants_query, "cached — no re-query");
        assert_eq!(
            snapshot(&state).expect("open at once").greeting,
            "Back again?"
        );
    }

    /// The reference's `missing` path (`0x4e216e`): a record that names no line for this NPC's
    /// gender column ends the interaction — the frame never opens with a blank page, and never
    /// parks half-open either. And `text_id == 0` never queries at all (`DBCache::Get` refuses
    /// id 0): the menu stays latched closed.
    #[test]
    fn missing_text_ends_the_interaction_and_id_zero_never_queries() {
        // Missing: the record arrives but its male column is empty → no line → session over.
        let mut state = GossipState::default();
        assert!(state.open_menu(0x42, 60, Vec::new(), Vec::new(), 0));
        state.text_arrived(
            60,
            vec![NpcTextBlock {
                probability: 0.0,
                male: String::new(),
                female: "Sister.".into(),
            }],
            0,
        );
        assert_eq!(state.npc, None, "missing gossip text — interaction ended");
        assert!(snapshot(&state).is_none());

        // Id 0: no query wanted, and the menu never opens.
        let mut state = GossipState::default();
        assert!(!state.open_menu(0x42, 0, Vec::new(), Vec::new(), 0));
        assert!(snapshot(&state).is_none());
    }

    /// A late answer for a menu we closed or switched away from only seeds the cache; an answer
    /// for a menu already open does not re-roll its greeting.
    #[test]
    fn late_or_duplicate_answers_only_seed_the_cache() {
        let blocks = || {
            vec![NpcTextBlock {
                probability: 0.0,
                male: "Hail.".into(),
                female: String::new(),
            }]
        };
        // Switched: the waiting menu is for id 70; id 50's late answer must not open it.
        let mut state = GossipState::default();
        assert!(state.open_menu(0x42, 70, Vec::new(), Vec::new(), 0));
        state.text_arrived(50, blocks(), 0);
        assert!(snapshot(&state).is_none(), "still waiting on id 70");
        assert!(
            state.cached_record(50).is_some(),
            "the answer seeded the cache"
        );

        // Already open: a duplicate answer leaves the drawn line alone.
        state.text_arrived(70, blocks(), 0);
        assert_eq!(snapshot(&state).expect("open").greeting, "Hail.");
        state.greeting = Some("The drawn line".into());
        state.text_arrived(70, blocks(), 0);
        assert_eq!(
            state.greeting.as_deref(),
            Some("The drawn line"),
            "no re-roll"
        );
    }

    #[test]
    fn ask_once_text_cache_survives_clear() {
        let mut state = GossipState::default();
        state.remember_record(
            50,
            vec![NpcTextBlock {
                probability: 0.0,
                male: "Greetings $N".into(),
                female: String::new(),
            }],
        );
        state.clear();
        // The open menu is gone but the cached RECORD is still servable (a revisit won't re-query
        // — it re-draws from these blocks).
        assert!(state.cached_record(50).is_some());
        assert_eq!(
            state.draw_greeting(50, 0).as_deref(),
            Some("Greetings $N"),
            "the all-zero record draws block 0 on every roll"
        );
        assert!(state.npc.is_none());
    }

    /// The record is cached, the GREETING is not: a female NPC and a male NPC sharing one `text_id`
    /// must read their own columns. Caching a resolved string (as this did) froze whichever one
    /// asked first for the rest of the session.
    #[test]
    fn one_cached_record_greets_each_gender_from_its_own_column() {
        let mut state = GossipState::default();
        state.remember_record(
            77,
            vec![NpcTextBlock {
                probability: 0.0,
                male: "Well met, friend.".into(),
                female: "Well met, sister.".into(),
            }],
        );
        assert_eq!(
            state.draw_greeting(77, 0).as_deref(),
            Some("Well met, friend.")
        );
        assert_eq!(
            state.draw_greeting(77, 1).as_deref(),
            Some("Well met, sister.")
        );
        assert_eq!(state.draw_greeting(78, 0), None, "record not yet arrived");
    }

    /// The icon byte is an INDEX into the client's own table (`0x84b7ac`, VERIFIED — decision
    /// 1335), and these are the seats that were wrong before 1331. Each is pinned to the
    /// `option_id` the world DB pairs it with, because that pairing is what makes the table
    /// legible: spirit healers send 4, innkeepers 5, bankers 6, petitioners 7, tabard designers 8,
    /// battlemasters 9.
    #[test]
    fn the_icon_byte_indexes_the_clients_own_table() {
        assert_eq!(gossip_icon_type(0), "gossip");
        assert_eq!(gossip_icon_type(1), "vendor"); // GOSSIP_OPTION_VENDOR / _ARMORER
        assert_eq!(gossip_icon_type(2), "taxi"); // _TAXIVENDOR
        assert_eq!(gossip_icon_type(3), "trainer"); // _TRAINER
        assert_eq!(gossip_icon_type(4), "healer"); // _SPIRITHEALER / _SPIRITGUIDE — was "binder"
        assert_eq!(gossip_icon_type(5), "binder"); // _INNKEEPER — B249: was the chat bubble
        assert_eq!(gossip_icon_type(6), "banker"); // _BANKER — was "tabard"
        assert_eq!(gossip_icon_type(7), "petition"); // _PETITIONER — was the chat bubble
        assert_eq!(gossip_icon_type(8), "tabard"); // _TABARDDESIGNER — was the chat bubble
        assert_eq!(gossip_icon_type(9), "battlemaster"); // _BATTLEFIELD
        assert_eq!(gossip_icon_type(10), "auctioneer"); // _AUCTIONEER — was the chat bubble
                                                        // 11-13 are genuine `gossip` aliases in the binary, not padding.
        assert_eq!(gossip_icon_type(11), "gossip");
        assert_eq!(gossip_icon_type(13), "gossip");
        // 14 is a NULL the reference pushes as `nil`, and 15+ walks off its table into the Lua
        // binding array. Both are its missing guard, not its design — we clamp (see the fn's doc).
        assert_eq!(gossip_icon_type(14), "gossip");
        assert_eq!(gossip_icon_type(255), "gossip");
    }

    /// The table must name the art the install actually ships — the check that could not have
    /// passed while the mapping was wrong-by-naming, because the reference builds the path by
    /// concatenation (`GossipFrame.lua:123`:
    /// `SetTexture("Interface\\GossipFrame\\" .. arg[i+1] .. "GossipIcon")`), so a type with no BLP
    /// behind it is a blank row.
    ///
    /// **`auctioneer` is the one exception, and it is the client's, not ours**: index 10 names a
    /// texture 5875 never shipped, so the reference itself draws nothing for it. Asserting the
    /// absence pins that as a fact rather than leaving it as a hole someone later "fixes" by
    /// inventing a path. Skips without client data.
    #[test]
    fn gossip_icon_types_name_the_shipped_art() {
        let data = benilla_formats::wow_data_or_skip!();
        let chain = benilla_formats::open_chain(&data).expect("open chain");
        let blp = |ty: &str| format!("Interface\\GossipFrame\\{ty}GossipIcon.blp");
        for ty in GOSSIP_ICON_TYPES {
            if ty == GOSSIP_ICON_TYPE_UNSHIPPED {
                continue;
            }
            assert!(chain.contains(&blp(ty)), "{} is not on the chain", blp(ty));
        }
        assert!(
            !chain.contains(&blp(GOSSIP_ICON_TYPE_UNSHIPPED)),
            "5875 has grown a {GOSSIP_ICON_TYPE_UNSHIPPED} gossip icon — give the XML its key"
        );
    }
}
