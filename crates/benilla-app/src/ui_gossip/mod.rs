//! The gossip window's app side: [`GossipState`] holds the menu and the NPC-text records, which
//! the getters read through the host at the call ([`live`]); the greeting's draw signals on the
//! gossip lane at its packet ([`lane`]), the selects apply in their calls over its slots
//! ([`GossipVerbs`]), and so does `CloseGossip`'s close core.
//! As in the reference's `0x4e2010`, a menu shows only once its greeting is drawn.

use benilla_protocol::messages::{select_greeting, GossipOption, NpcTextBlock};
use bevy::prelude::*;

use benilla_ui::script::{GossipMenu, GossipOptionView, GossipQuestRow};

use crate::query_cache::QueryCache;
use crate::ui_quest::row_is_active;
use crate::ui_script::{SweepApp, SweepLeg};
use crate::ui_session::{LatchApp, LatchedWindow, NpcWindow};

/// The open gossip menu, plus the NPC-text records by id, which outlive it. The cache holds whole
/// records: the reference draws the greeting afresh at each open, from the NPC's gender and a roll.
#[derive(Resource, Default)]
pub(crate) struct GossipState {
    pub(crate) npc: Option<u64>,
    pub(crate) text_id: u32,
    /// This open's drawn greeting, unexpanded; `None` while its text query is out.
    pub(crate) greeting: Option<String>,
    /// The text buffer `0xbbb678`: the last drawn greeting, expanded at its draw (`0x4e2274`,
    /// `0x4e229a`), which `GetGossipText` returns (`0x4e28c0`). No close or message clears it.
    pub(crate) text: String,
    pub(crate) options: Vec<GossipOption>,
    /// The packet's quest rows, `(quest_id, dialog-status icon, level, title)`. The level is kept
    /// because `GetGossipAvailableQuests` and `GetGossipActiveQuests` return `(title, level)`
    /// pairs, walked in twos by stock `GossipFrame.lua` (lines 66 and 90).
    pub(crate) quests: Vec<(u32, u32, u32, String)>,
    /// The NPC-text store (`0xc0e264`), which [`crate::wdb`] saves; the menu sends its own query.
    records: QueryCache<u32, Vec<NpcTextBlock>>,
    /// The drawn record had no line for the NPC's gender: the close core is owed.
    missing_text: bool,
    /// The reference's text-pending latch `[0xbbb670]`: set on the text's cache miss
    /// (`0x4e2050`), cleared once the record is found (`0x4e206e`); no close touches it.
    pending: bool,
    /// A greeting was drawn: the caller owes its expansion into [`Self::text`] and `GOSSIP_SHOW`.
    drawn: bool,
}

impl GossipState {
    pub(crate) fn cached_record(&self, text_id: u32) -> Option<&[NpcTextBlock]> {
        self.records.get(text_id).map(Vec::as_slice)
    }

    /// The NPC-text store, the one [`crate::wdb`] saves.
    pub(crate) fn record_store(&self) -> &QueryCache<u32, Vec<NpcTextBlock>> {
        &self.records
    }

    pub(crate) fn record_store_mut(&mut self) -> &mut QueryCache<u32, Vec<NpcTextBlock>> {
        &mut self.records
    }

    pub(crate) fn remember_record(&mut self, text_id: u32, blocks: Vec<NpcTextBlock>) {
        self.records.insert(text_id, blocks);
    }

    /// `None` when the record has not arrived or has no line in the NPC's gender column.
    pub(crate) fn draw_greeting(&self, text_id: u32, npc_gender: u8) -> Option<String> {
        let blocks = self.cached_record(text_id)?;
        select_greeting(blocks, npc_gender, greeting_roll()).map(str::to_string)
    }

    /// The reference's text-pending latch (`[0xbbb670]`): a menu whose text query is out, or was
    /// never sent (`text_id` 0). While it holds, nothing fires and the option select is refused.
    pub(crate) fn text_pending(&self) -> bool {
        self.pending
    }

    /// Latch a `SMSG_GOSSIP_MESSAGE` menu; `true` when the caller owes the `CMSG_NPC_TEXT_QUERY`.
    /// A `text_id` of 0 stays pending: the reference's `DBCache::Get` never queries id 0.
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
        // A miss, id 0 included (`0x55d530`), holds the latch until the record lands.
        self.pending = true;
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

    /// Cache a `SMSG_NPC_TEXT_UPDATE` record and resolve the menu waiting on it, if any. An open
    /// menu's greeting is not re-drawn: the reference's cache callback re-enters only for the
    /// query it was registered for.
    pub(crate) fn text_arrived(&mut self, text_id: u32, blocks: Vec<NpcTextBlock>, npc_gender: u8) {
        self.remember_record(text_id, blocks);
        if self.pending && self.text_id == text_id && text_id != 0 {
            self.resolve_greeting(npc_gender);
        }
    }

    /// Draw the open menu's greeting, or owe the close when the record has no line for the NPC's
    /// gender: the reference's missing-text path (`0x4e216e`) logs "Missing gossip text!" and
    /// runs the close core (`0x4e2197`), which [`Self::take_missing_text`] hands the caller.
    fn resolve_greeting(&mut self, npc_gender: u8) {
        self.pending = false;
        match self.draw_greeting(self.text_id, npc_gender) {
            Some(line) => {
                self.greeting = Some(line);
                self.drawn = true;
            }
            None => {
                debug!(
                    "ui_gossip: missing gossip text (id {}) — ending the interaction",
                    self.text_id
                );
                self.missing_text = true;
            }
        }
    }

    /// The missing-text path's close, owed to the caller, which holds the world the core needs.
    pub(crate) fn take_missing_text(&mut self) -> bool {
        std::mem::take(&mut self.missing_text)
    }

    /// The greeting drawn since the last take, unexpanded: its expansion and `GOSSIP_SHOW` are
    /// owed to the caller, which holds the world the expansion reads.
    pub(crate) fn take_drawn(&mut self) -> Option<String> {
        std::mem::take(&mut self.drawn)
            .then(|| self.greeting.clone())
            .flatten()
    }
}

/// The greeting draw's roll in `[1.0, 2.0)`, built as the reference builds it: random bits in the
/// mantissa of 1.0 (`(rand & 0x7fffff) | 0x3f800000`), from our own random source.
fn greeting_roll() -> f32 {
    f32::from_bits((rand::random::<u32>() & 0x7f_ffff) | 0x3f80_0000)
}

impl GossipState {
    /// Forget the menu, its slots and its latches; the records stay.
    pub(crate) fn clear(&mut self) {
        self.npc = None;
        self.text_id = 0;
        self.greeting = None;
        self.options.clear();
        self.quests.clear();
        self.missing_text = false;
        self.pending = false;
        self.drawn = false;
        self.text.clear();
    }

    /// `ResetGossipOptions` (`0x4e26a0`): every option slot's index to -1 and every quest id to 0,
    /// so no getter or select finds a row; the text, the NPC cell and the latches stay.
    pub(crate) fn reset_slots(&mut self) {
        self.options.clear();
        self.quests.clear();
    }

    /// The disconnect clear; NPC text is static per id, so the records stay.
    pub(crate) fn clear_session(&mut self) {
        self.clear();
    }
}

pub(crate) mod lane;
pub(crate) mod live;
pub(crate) mod net;
mod verbs;
pub(crate) use lane::{GossipEvent, GossipLane};
pub(crate) use live::GossipLive;
pub(crate) use verbs::GossipVerbs;
#[cfg(test)]
mod verb_tests;
/// The opener the server's menu runs, for the latch's tests.
#[cfg(test)]
pub(crate) use net::gossip_menu as open_menu;

/// The gossip window's handlers and lane.
pub(crate) struct UiGossipPlugin;

impl Plugin for UiGossipPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        lane::add_gossip_lane(app);
        app.init_resource::<GossipState>()
            .add_latched_window::<GossipState>()
            // `0x490ac1` → `0x4e1f60`: the core `0x4e1fa0(0)`, then the slots' reset (`0x4e1f89`).
            .add_sweep_leg(SweepLeg::Gossip, leave_world);
    }
}

/// The menu's cell `[0xbc3f58]`; `CloseGossip` (`0x4e2b20`) is the core with a zero guid, run
/// inside its call ([`crate::script_calls::ScriptCallHost`]).
impl LatchedWindow for GossipState {
    const WINDOW: NpcWindow = NpcWindow::Gossip;

    fn cell(&self) -> Option<u64> {
        self.npc
    }

    fn dispatched(world: &mut World) {
        close_core(world);
    }
}

/// The leave-world leg `0x4e1f60`: the close core (`0x4e1f84`), then `ResetGossipOptions`
/// (`0x4e1f89` → `0x4e26a0`) whether or not a menu was open, so the getters answer empty lists
/// until the next gossip.
fn leave_world(world: &mut World) {
    close_core(world);
    world.resource_mut::<GossipState>().reset_slots();
}

/// The close core, `0x4e1fa0` with a zero guid: nothing with no menu (`0x4e1fb8`, a zero guid
/// against a zero cell), else the latch release (`0x4e1fc5`), the cell zeroed (`0x4e1fcc`), and
/// `GOSSIP_CLOSED` (`0x4e1fdc`); the slots, the text and its latch stay, and their getters read
/// them still. A menu still waiting on its text was latched at its open, so its close fires too.
/// `SMSG_GOSSIP_COMPLETE` (`0x4e2824`), the missing-text path (`0x4e2197`), `0x493310` and the
/// leave-world sweep's leg ([`leave_world`]) call it as it is.
/// It runs behind the gossip lane, so it never fires ahead of a show still queued there.
pub(crate) fn close_core(world: &mut World) {
    let Some(npc) = world.resource::<GossipState>().npc else {
        return;
    };
    lane::behind_lane(world, |world| {
        crate::ui_session::release(world, npc);
        world.resource_mut::<GossipState>().npc = None;
        crate::ui_session::fire(world, |script| {
            script.fire_event("GOSSIP_CLOSED", vec![]);
        });
    });
}

/// The `GetGossipOptions()` type string for a wire `GOSSIP_ICON` byte, which the reference's
/// `0x4e28d0` uses as a bare index into the 14-entry table at `0x84b7ac`.
///
/// Deviation: 14 (a NULL, pushed as `nil`) and 15 up (a read past the table's end) give
/// `"gossip"`, because the reference's behaviour there is a missing bounds check.
fn gossip_icon_type(icon: u8) -> &'static str {
    GOSSIP_ICON_TYPES
        .get(icon as usize)
        .copied()
        .unwrap_or("gossip")
}

/// The reference's icon-type table at `0x84b7ac`, verbatim; 11-13 are real `gossip` aliases.
/// `auctioneer` names art 1.12 never shipped, so its row has no icon, in the reference too.
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

/// The store as the getters read it at the call, which a close leaves: the text buffer, the used
/// option slots in slot order, an index with its top bit set skipped as unused (`0x4e28ec`), and
/// the quest rows, active or available by the wire icon (the `{3,4}` test of `0x4e2430` and
/// `0x4e2580`).
pub(crate) fn menu(state: &GossipState) -> GossipMenu {
    GossipMenu {
        greeting: state.text.clone(),
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
            .filter(|o| (o.index as i32) >= 0)
            .map(|o| GossipOptionView {
                label: o.message.clone(),
                icon_type: gossip_icon_type(o.icon).into(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one type in [`GOSSIP_ICON_TYPES`] with no art in the 1.12 install.
    const GOSSIP_ICON_TYPE_UNSHIPPED: &str = "auctioneer";

    fn option(index: u32, icon: u8, label: &str) -> GossipOption {
        GossipOption {
            index,
            icon,
            coded: false,
            message: label.into(),
        }
    }

    /// The getters' store: the text buffer, empty before any draw, and the used slots in slot
    /// order, an index with its top bit set skipped (`0x4e28ec`).
    #[test]
    fn the_menu_reads_the_text_buffer_and_the_used_slots() {
        let mut state = GossipState::default();
        assert_eq!(menu(&state), GossipMenu::default());
        state.text = "Hello".into();
        state.options = vec![
            option(3, 1, "Browse"),
            option(0x8000_0000, 0, "Hidden"),
            option(4, 2, "Fly"),
        ];
        let read = menu(&state);
        assert_eq!(read.greeting, "Hello");
        let rows: Vec<_> = read
            .options
            .iter()
            .map(|o| (o.label.as_str(), o.icon_type.as_str()))
            .collect();
        assert_eq!(rows, [("Browse", "vendor"), ("Fly", "taxi")]);
    }

    /// A first visit draws nothing until its text answers, then draws once: the reference's
    /// greeting write and `GOSSIP_SHOW` sit together on its one success path (`0x4e229a`,
    /// `0x4e22b0`), and its cache-miss exit draws nothing.
    #[test]
    fn first_visit_draws_once_its_text_answers() {
        let mut state = GossipState::default();
        let options = vec![option(0, 9, "I wish to join the battle!")];
        assert!(
            state.open_menu(0x42, 50, options, Vec::new(), 0),
            "first visit asks for the text"
        );
        assert_eq!(
            state.take_drawn(),
            None,
            "nothing drawn while the query is out"
        );
        assert_eq!(menu(&state).options.len(), 1, "the slots are the message's");
        state.text_arrived(
            50,
            vec![NpcTextBlock {
                probability: 0.0,
                male: "The Alliance needs you!".into(),
                female: String::new(),
            }],
            0,
        );
        assert_eq!(
            state.take_drawn().as_deref(),
            Some("The Alliance needs you!")
        );
        assert_eq!(state.take_drawn(), None, "drawn once");
    }

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
            state.take_drawn().as_deref(),
            Some("Back again?"),
            "drawn at once"
        );
    }

    /// The reference's missing-text path (`0x4e216e`) ends the interaction, and its
    /// `DBCache::Get` never queries id 0.
    #[test]
    fn missing_text_ends_the_interaction_and_id_zero_never_queries() {
        // The record's male column is empty, so a male NPC has no line.
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
        assert!(
            state.take_missing_text(),
            "missing gossip text — the close core is owed"
        );
        assert_eq!(state.take_drawn(), None);

        // Id 0: no query wanted, and nothing is ever drawn.
        let mut state = GossipState::default();
        assert!(!state.open_menu(0x42, 0, Vec::new(), Vec::new(), 0));
        assert_eq!(state.take_drawn(), None);
    }

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
        assert_eq!(state.take_drawn(), None, "still waiting on id 70");
        assert!(
            state.cached_record(50).is_some(),
            "the answer seeded the cache"
        );

        // Already open: a duplicate answer leaves the drawn line alone.
        state.text_arrived(70, blocks(), 0);
        assert_eq!(state.take_drawn().as_deref(), Some("Hail."));
        state.greeting = Some("The drawn line".into());
        state.text_arrived(70, blocks(), 0);
        assert_eq!(state.take_drawn(), None, "no second draw");
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
        // The record outlives the menu; a revisit re-draws from it.
        assert!(state.cached_record(50).is_some());
        assert_eq!(
            state.draw_greeting(50, 0).as_deref(),
            Some("Greetings $N"),
            "the all-zero record draws block 0 on every roll"
        );
        assert!(state.npc.is_none());
    }

    /// The record is cached, not the greeting, so NPCs sharing a `text_id` read their own columns.
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

    /// The reference's table (`0x84b7ac`), each index beside the `GOSSIP_OPTION_*` the world DB
    /// sends it with.
    #[test]
    fn the_icon_byte_indexes_the_clients_own_table() {
        assert_eq!(gossip_icon_type(0), "gossip");
        assert_eq!(gossip_icon_type(1), "vendor"); // GOSSIP_OPTION_VENDOR / _ARMORER
        assert_eq!(gossip_icon_type(2), "taxi"); // _TAXIVENDOR
        assert_eq!(gossip_icon_type(3), "trainer"); // _TRAINER
        assert_eq!(gossip_icon_type(4), "healer"); // _SPIRITHEALER / _SPIRITGUIDE
        assert_eq!(gossip_icon_type(5), "binder"); // _INNKEEPER
        assert_eq!(gossip_icon_type(6), "banker"); // _BANKER
        assert_eq!(gossip_icon_type(7), "petition"); // _PETITIONER
        assert_eq!(gossip_icon_type(8), "tabard"); // _TABARDDESIGNER
        assert_eq!(gossip_icon_type(9), "battlemaster"); // _BATTLEFIELD
        assert_eq!(gossip_icon_type(10), "auctioneer"); // _AUCTIONEER
        assert_eq!(gossip_icon_type(11), "gossip");
        assert_eq!(gossip_icon_type(13), "gossip");
        // The deviation past the table: see `gossip_icon_type`.
        assert_eq!(gossip_icon_type(14), "gossip");
        assert_eq!(gossip_icon_type(255), "gossip");
    }

    /// Stock `GossipFrame.lua:123` builds the icon path from the type, so every type has its BLP
    /// but `auctioneer`, which 1.12 never shipped.
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
