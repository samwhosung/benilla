//! The gossip window's app side: [`GossipState`] holds the menu and the NPC-text records,
//! [`feed_gossip`] pushes the menu and fires its events, the selects apply in their calls over its
//! slots ([`GossipVerbs`]), and [`drain_gossip`] runs `CloseGossip`'s teardown.
//! As in the reference's `0x4e2010`, a menu opens only once its greeting is drawn.

use std::collections::HashMap;

use benilla_protocol::messages::{select_greeting, GossipOption, NpcTextBlock};
use bevy::prelude::*;

use benilla_ui::script::{GossipMenu, GossipOptionView, GossipQuestRow, UiScript};

use crate::names::NameCache;
use crate::net::{Guid, NetCommands, ObjectStore, SelfPlayer};
use crate::query_cache::Ask;
use crate::ui_quest::row_is_active;
use crate::ui_script::{SweepApp, SweepLeg, UiFeed};
use crate::ui_session::{LatchApp, LatchedWindow, NpcWindow};

/// The open gossip menu, plus the NPC-text records by id, which outlive it. The cache holds whole
/// records: the reference draws the greeting afresh at each open, from the NPC's gender and a roll.
#[derive(Resource, Default)]
pub(crate) struct GossipState {
    pub(crate) npc: Option<u64>,
    pub(crate) text_id: u32,
    /// This open's drawn greeting; `None` while its text query is out.
    pub(crate) greeting: Option<String>,
    pub(crate) options: Vec<GossipOption>,
    /// The packet's quest rows, `(quest_id, dialog-status icon, level, title)`. The level is kept
    /// because `GetGossipAvailableQuests` and `GetGossipActiveQuests` return `(title, level)`
    /// pairs, walked in twos by stock `GossipFrame.lua` (lines 66 and 90).
    pub(crate) quests: Vec<(u32, u32, u32, String)>,
    records: HashMap<u32, Vec<NpcTextBlock>>,
    /// The drawn record had no line for the NPC's gender: the close core is owed.
    missing_text: bool,
    /// The reference's text-pending latch `[0xbbb670]`: set on the text's cache miss
    /// (`0x4e2050`), cleared once the record is found (`0x4e206e`); no close touches it.
    pending: bool,
    /// The messages latched so far, which the feed shows each of once.
    generation: u64,
    /// The source whose close a call began, its cell zeroed there (`0x4e1fcc`): the latch
    /// release and `GOSSIP_CLOSED` hold the whole world, so the drain runs them.
    pub(crate) closing: Option<u64>,
}

impl GossipState {
    pub(crate) fn cached_record(&self, text_id: u32) -> Option<&[NpcTextBlock]> {
        self.records.get(&text_id).map(Vec::as_slice)
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
        self.generation += 1;
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
            Some(line) => self.greeting = Some(line),
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
        self.closing = None;
    }

    /// The disconnect clear; NPC text is static per id, so the records stay.
    pub(crate) fn clear_session(&mut self) {
        self.clear();
    }
}

mod net;
mod verbs;
pub(crate) use verbs::GossipVerbs;
#[cfg(test)]
mod verb_tests;
/// The opener the server's menu runs, for the latch's tests.
#[cfg(test)]
pub(crate) use net::gossip_menu as open_menu;

/// The gossip window's handlers, feed and drain.
pub(crate) struct UiGossipPlugin;

impl Plugin for UiGossipPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<GossipState>()
            .add_latched_window::<GossipState>()
            // `0x490ac1` → `0x4e1f60` → `0x4e1fa0(0)`, the core.
            .add_sweep_leg(SweepLeg::Gossip, close_core)
            .add_systems(
                Update,
                (
                    feed_gossip
                        .in_set(UiFeed)
                        .in_set(crate::ui_session::NpcWindowFeed),
                    drain_gossip.in_set(crate::ui_script::UiDrain),
                ),
            );
    }
}

/// The menu's cell `[0xbc3f58]`; `CloseGossip` (`0x4e2b20`) is the core with a zero guid.
impl LatchedWindow for GossipState {
    const WINDOW: NpcWindow = NpcWindow::Gossip;

    /// The cell, or the source a call zeroed it from until the drain's teardown.
    fn cell(&self) -> Option<u64> {
        self.npc.or(self.closing)
    }

    fn dispatched(world: &mut World) {
        close_core(world);
    }

    fn take_close_verb(script: &mut UiScript) -> bool {
        script.take_gossip_close()
    }
}

/// The close core, `0x4e1fa0` with a zero guid: nothing with no menu (`0x4e1fb8`, a zero guid
/// against a zero cell), else the latch release (`0x4e1fc5`), the cell zeroed (`0x4e1fcc`), and
/// `GOSSIP_CLOSED` (`0x4e1fdc`); the slots, the text and its latch stay, and their getters read
/// them still. A menu still waiting on its text was latched at its open, so its close fires too.
/// `SMSG_GOSSIP_COMPLETE` (`0x4e2824`), the missing-text path (`0x4e2197`) and the leave-world
/// sweep's leg (`0x490ac1` → `0x4e1f60`) call it as it is; a `CloseGossip` call has zeroed the
/// cell already ([`GossipState::closing`]).
pub(crate) fn close_core(world: &mut World) {
    let source = {
        let mut state = world.resource_mut::<GossipState>();
        state.closing.take().or(state.npc)
    };
    let Some(npc) = source else {
        return;
    };
    crate::ui_session::release(world, npc);
    world.resource_mut::<GossipState>().npc = None;
    crate::ui_session::note_closed(world, NpcWindow::Gossip);
    crate::ui_session::fire(world, |script| {
        script.fire_event("GOSSIP_CLOSED", vec![]);
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

/// The Lua-facing menu, the slots and the text as the getters read them, which a close leaves;
/// `None` before any text. [`feed_gossip`] never calls it while the text is pending.
fn snapshot(state: &GossipState) -> Option<GossipMenu> {
    Some(GossipMenu {
        greeting: state.greeting.clone()?,
        // Active or available by the wire icon: the reference's `{3,4}` test in `0x4e2430` and
        // `0x4e2580`, behind `GetGossipAvailableQuests` and `GetGossipActiveQuests`.
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
            })
            .collect(),
    })
}

/// Push the menu into the VM and, with the window open, fire `GOSSIP_SHOW` once for each message
/// as its greeting is drawn; the close is the core's ([`close_core`]). The menu the VM holds is the
/// memory: `SelectGossipOption`'s reset writes both sides, so it reads as no change. The slots wait
/// outside the VM's turn, since the gossip verbs write them in the host.
fn feed_gossip(
    mut vm: crate::script_calls::VmWith<Res<'static, GossipState>>,
    self_q: Query<(&ObjectStore, &Guid), With<SelfPlayer>>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    objects: crate::net::Objects,
    unit_names: crate::ui_unit::NameTables,
    states: Res<crate::world_state::WorldStates>,
    mut shown_message: crate::ui_script::WindowMemo<u64>,
) {
    vm.turn(|script, state| {
        let Some(script) = script else {
            return;
        };
        let shown_message = shown_message.get(script);
        let shown = script.gossip();
        // The stock window titles with `UnitName("npc")` (`GossipFrame.lua:33`), which looks the
        // name up with a callback (`0x5171b8`) at the show a gossip packet brings; its landing
        // fires nothing. Asked before the hold, so the name query goes out beside the text query.
        let ask = Ask::at(shown.is_none() || state.is_changed(), &commands);
        if let Some(g) = state.npc {
            let _ = names.resolve(&objects, g, ask);
        }
        // While the text query is out, fire nothing and keep the last snapshot: the reference
        // returns at `0x4e2068` with no write and no event, so its frame keeps what it last showed.
        // Reading the hold as a close would flash an open window.
        if state.text_pending() {
            return;
        }
        let mut fresh = snapshot(&state);
        // Expand the greeting's `$N`/`$B`/`$G`/`$<n>w` macros client-side, as the reference does.
        if let Some(greeting) = fresh.as_mut().map(|m| &mut m.greeting) {
            let player = crate::npc_text::player_identity(&self_q, &names, unit_names.get());
            *greeting = crate::npc_text::substitute(
                greeting,
                &crate::npc_text::MacroContext {
                    subject: player.as_ref(),
                    states: &states,
                },
            );
        }
        let new_message = std::mem::replace(shown_message, state.generation) != state.generation;
        if fresh != shown {
            script.set_gossip(fresh.clone());
        }
        // `GOSSIP_SHOW` has one producer, `0x4e2010` (`0x4e22b5`, no argument), run by a message
        // (`0x4e27df`) and by its text's arrival (`0x4e22f2`), which the hold above waits on. A
        // close is the core's, and a disconnect's clear tells the VM nothing.
        if new_message && fresh.is_some() && state.npc.is_some() {
            script.fire_event("GOSSIP_SHOW", vec![]);
        }
    })
}

/// The rest of a `CloseGossip()` call's close core, which holds the whole world; the
/// selects apply in their calls ([`verbs::GossipVerbs`]).
fn drain_gossip(
    mut vm: crate::script_calls::VmWith<Res<'static, GossipState>>,
    mut world_commands: Commands,
) {
    let closed = vm.run(|script| script.is_some_and(|s| s.take_gossip_close()));
    if closed || vm.own().closing.is_some() {
        debug!("ui_gossip: client-side close (no packet)");
        crate::ui_session::queue(&mut world_commands, close_core);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::ClientCommand;
    use bevy::ecs::system::RunSystemOnce;

    /// The one type in [`GOSSIP_ICON_TYPES`] with no art in the 1.12 install.
    const GOSSIP_ICON_TYPE_UNSHIPPED: &str = "auctioneer";

    /// The stock window titles itself with `UnitName("npc")`, which looks the name up with a
    /// callback (`0x5171b8`), at the `GOSSIP_SHOW` a menu brings; the frames between read. A
    /// refused name is asked by no frame, and again by the next menu.
    #[test]
    fn a_refused_npc_name_is_asked_at_the_next_menu_never_by_a_frame() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<GossipState>()
            .init_resource::<NameCache>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::world_state::WorldStates>()
            .add_systems(Update, feed_gossip);
        app.insert_non_send_resource(UiScript::new().unwrap());
        // A creature not streamed: its name is asked by the guid's entry bits.
        const NPC: u64 = 0xF130_0000_4500_0001;
        let menu = |app: &mut App| {
            let mut state = app.world_mut().resource_mut::<GossipState>();
            state.remember_record(
                7,
                vec![NpcTextBlock {
                    probability: 1.0,
                    male: "Hello".into(),
                    female: "Hello".into(),
                }],
            );
            state.open_menu(NPC, 7, Vec::new(), Vec::new(), 0);
        };
        menu(&mut app);
        app.update();
        assert_eq!(crate::query_cache::queries(&rx).len(), 1);
        app.world_mut()
            .resource_mut::<NameCache>()
            .insert_creature(0x45, None);
        for _ in 0..600 {
            app.update();
        }
        assert!(crate::query_cache::queries(&rx).is_empty());
        menu(&mut app);
        app.update();
        assert_eq!(crate::query_cache::queries(&rx).len(), 1);
    }

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

    /// A first visit keeps the menu closed until its text answers, then opens it complete: the
    /// reference's greeting write and `GOSSIP_SHOW` sit together on its one success path
    /// (`0x4e229a`, `0x4e22b0`), and its cache-miss exit fires nothing.
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

    // ── The hold, through the real feed ─────────────────────────────────────────

    /// Records the two gossip events the VM sees, in order.
    const RECORDER: &str = r#"
        SEEN = {}
        local f = CreateFrame("Frame")
        f:RegisterEvent("GOSSIP_SHOW")
        f:RegisterEvent("GOSSIP_CLOSED")
        f:SetScript("OnEvent", function() table.insert(SEEN, event) end)
    "#;

    /// The real [`feed_gossip`] and [`drain_gossip`] over a VM, in plugin order; `prepare` seeds
    /// the VM, and the receiver gets what the drain sent.
    fn feed_app(
        prepare: impl FnOnce(&mut UiScript),
    ) -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<GossipState>()
            .init_resource::<NameCache>()
            .init_resource::<crate::net::GuidIndex>()
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

    /// One frame, then the events the VM saw and `GetGossipText()` (`""` for nil).
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

    /// A host that applies the gossip calls alone, as the whole client's applies them.
    struct GossipHost<'a, 'w, 's>(&'a mut super::verbs::GossipVerbs<'w, 's>);

    impl benilla_ui::script::CallHost for GossipHost<'_, '_, '_> {
        fn apply(
            &mut self,
            script: &mut UiScript,
            call: benilla_ui::script::ScriptCall,
        ) -> Option<benilla_ui::script::CallTail> {
            if let benilla_ui::script::ScriptCall::Gossip(call) = call {
                self.0.apply(script, call);
            }
            None
        }
    }

    /// Run `lua` with the gossip host lent.
    fn select(app: &mut App, lua: &'static str) {
        app.world_mut()
            .run_system_once(
                move |mut script: NonSendMut<UiScript>, mut verbs: super::verbs::GossipVerbs| {
                    script
                        .hosted(&mut GossipHost(&mut verbs), |s| s.run(lua))
                        .expect("the chunk runs");
                },
            )
            .expect("the hosted chunk runs as a one-shot system");
    }

    /// The selects sent; the channel also carries the feed's name query.
    fn selects_sent(rx: &crossbeam_channel::Receiver<ClientCommand>) -> usize {
        rx.try_iter()
            .filter(|c| matches!(c, ClientCommand::GossipSelectOption { .. }))
            .count()
    }

    const GUARD: u64 = 0xF130_0000_0000_0007; // HIGHGUID_UNIT

    /// A guard's sub-menu on a first-visit text: nothing fires while the query is out, the old
    /// menu stays with its selects refused, and the text repaints in place, as the reference's
    /// exit at `0x4e2068` leaves it. A close fired at the pending step would flash the window.
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

        // "The bank" is clicked: a new menu on the same NPC with an unseen text id. Nothing fires
        // for the round trip, and a click on the old menu's row is refused.
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
        select(&mut app, "SelectGossipOption(1)");
        let _ = tick(&mut app);
        assert_eq!(
            selects_sent(&rx),
            0,
            "a select during the hold is refused (the latch)"
        );

        // The text lands: one in-place repaint, and selects work again.
        state(&mut app).text_arrived(51, record("The bank is north of here."), 0);
        assert_eq!(
            tick(&mut app),
            (
                vec!["GOSSIP_SHOW".to_string()],
                "The bank is north of here.".to_string()
            )
        );
        select(&mut app, "SelectGossipOption(1)");
        let _ = tick(&mut app);
        assert_eq!(selects_sent(&rx), 1);

        // Controls: a real end still closes, and a first visit from a closed frame stays shut
        // until its text answers.
        // The close leaves the text and the slots, which the getters still read (`0x4e1fa0`).
        close_core(app.world_mut());
        assert_eq!(
            tick(&mut app),
            (
                vec!["GOSSIP_CLOSED".to_string()],
                "The bank is north of here.".to_string()
            )
        );
        assert!(state(&mut app).open_menu(GUARD, 52, option("Goodbye"), Vec::new(), 0));
        assert_eq!(
            tick(&mut app),
            (Vec::new(), "The bank is north of here.".to_string()),
            "hidden stays hidden through the hold"
        );
        state(&mut app).text_arrived(52, record("Yes?"), 0);
        assert_eq!(
            tick(&mut app),
            (vec!["GOSSIP_SHOW".to_string()], "Yes?".to_string())
        );
    }

    /// `GOSSIP_SHOW` fires only in `0x4e2010`, which a message (`0x4e27df`) or its text's arrival
    /// (`0x4e22f2`) runs: the NPC's name landing fires nothing, and the next message fires again.
    #[test]
    fn the_npc_name_landing_fires_no_second_gossip_show() {
        const SPEAKER: u64 = 0x10;
        let (mut app, _rx) = feed_app(|s| s.run(RECORDER).unwrap());
        let menu = |app: &mut App| {
            let mut st = state(app);
            st.remember_record(50, record("Well met."));
            st.open_menu(SPEAKER, 50, option("Goodbye"), Vec::new(), 0);
        };
        menu(&mut app);
        assert_eq!(tick(&mut app).0, ["GOSSIP_SHOW"]);
        app.world_mut()
            .resource_mut::<NameCache>()
            .insert_player(SPEAKER, "Gossiper".into(), None);
        assert!(tick(&mut app).0.is_empty(), "a landed name fires nothing");
        menu(&mut app);
        assert_eq!(tick(&mut app).0, ["GOSSIP_SHOW"], "the next message does");
    }

    /// The same sequence through stock `GossipFrame.xml`: the window stays up, silent, on the
    /// first menu, and the repaint plays no open kit, as `ShowUIPanel` returns early on a visible
    /// frame.
    #[test]
    fn stock_gossip_frame_stays_visible_through_a_first_visit_submenu() {
        use benilla_ui::script::SoundRequest;
        let _data = benilla_formats::wow_data_or_skip!();
        let (mut app, _rx) = feed_app(|s| {
            s.set_screen_size(1024.0, 768.0);
            s.set_text_measurer(Box::new(crate::ui_script::FixedWidthFont(6.0)));
            for f in crate::ui_script::test_ui::production_order(&[
                crate::ui_script::test_ui::GOSSIP_UI,
                &[r"Interface\FrameXML\GossipFrame.xml"],
            ]) {
                crate::ui_script::test_ui::load_ui(s, f);
            }
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

        // The click's answer holds on its text: the window must not blink.
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

        // The text lands: repainted in place, with no kit.
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
        assert!(snapshot(&state).is_none());

        // Id 0: no query wanted, and the menu never opens.
        let mut state = GossipState::default();
        assert!(!state.open_menu(0x42, 0, Vec::new(), Vec::new(), 0));
        assert!(snapshot(&state).is_none());
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
