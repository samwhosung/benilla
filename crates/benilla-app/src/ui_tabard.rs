//! The guild tabard designer, the app half of the stock `TabardFrame.xml`. Only
//! `MSG_TABARDVENDOR_ACTIVATE` opens it (`0x5e70c0` → `0x4f5840`); no Lua verb does. Our own body
//! wears the design over a forced tabard geoset (`[cc+0xc]`, `0x47a610`).
//!
//! The designer is a window on the latch, so its core run while it holds the latch fires
//! `CLOSE_TABARD_FRAME` twice, the re-entrancy `0x4f58a0` → `0x4931c0` → `0x493310` → `0x4934a0`.

use bevy::prelude::*;

use benilla_formats::GuildEmblem;
use benilla_protocol::messages::GUILD_EMBLEM_RESULT_MESSAGES;
use benilla_ui::script::{TabardCall, TabardHost, UiScript, TABARD_COUNTS, TABARD_CREATION_COST};
use bevy::ecs::system::SystemParam;

use crate::net::{ClientCommand, EnteredWorldMessage, NetCommands, ObjectStore, SelfPlayer};
use crate::ui_guild::GuildState;
use crate::ui_script::{SweepApp, SweepLeg, UiFeed};
use crate::ui_session::{LatchApp, LatchedWindow, NpcWindow};

/// The open designer: its vendor (`[0xbdcee8]`), the save-in-flight latch (`[0xc4d780]`), and
/// the open and lines owed to the next feed. The close is the core's ([`close_core`]).
#[derive(Resource, Default)]
pub(crate) struct TabardOpen {
    vendor: Option<u64>,
    save_pending: bool,
    open_event: bool,
    pending_events: u32,
    lines: Vec<&'static str>,
}

impl TabardOpen {
    /// The designer's vendor, the cell `[0xbdcee8]`; `None` when it is closed.
    pub(crate) fn npc(&self) -> Option<u64> {
        self.vendor
    }

    /// An open designer on `vendor`, its `OPEN_TABARD_FRAME` owed, for a test that seats it
    /// directly.
    #[cfg(test)]
    pub(crate) fn open(&mut self, vendor: u64) {
        self.vendor = Some(vendor);
        self.open_event = true;
    }

    /// `MSG_SAVE_GUILD_EMBLEM` in (`0x5e70f0`), true on success; a failure re-fires
    /// `TABARD_SAVE_PENDING`.
    pub(crate) fn apply_result(&mut self, result: u32) -> bool {
        let Some(row) = usize::try_from(result)
            .ok()
            .and_then(|i| GUILD_EMBLEM_RESULT_MESSAGES.get(i))
        else {
            return false;
        };
        self.save_pending = false;
        if let Some(key) = row {
            self.lines.push(key);
        }
        if result != 0 {
            self.pending_events += 1;
        }
        result == 0
    }
}

/// `CloseTabardCreation` (`0x4f5900` → `0x4f58a0`) is the core itself.
impl LatchedWindow for TabardOpen {
    const WINDOW: NpcWindow = NpcWindow::Tabard;

    fn cell(&self) -> Option<u64> {
        self.vendor
    }

    fn dispatched(world: &mut World) {
        close_core(world);
    }
}

/// `MSG_TABARDVENDOR_ACTIVATE` in (`0x5e70c0` → `0x4f5840`): the opener writes the cell first
/// (`0x4f5845`) and registers on the latch after (`0x4f5882`), so an activate from the vendor
/// whose designer is already open closes it through `0x493310`, cell and all, and the designer
/// reopens holding the latch with its cell clear; then `OPEN_TABARD_FRAME` (`0x4f588d`).
pub(crate) fn open_tabard(world: &mut World, vendor: u64) {
    world.resource_mut::<TabardOpen>().vendor = Some(vendor);
    crate::ui_session::register(world, vendor);
    world.resource_mut::<TabardOpen>().open_event = true;
}

/// The close core `0x4f58a0` as `0x493310` and the leave-world sweep enter it: with a vendor
/// stored, the preview ends (`0x5e0830`), the latch is released (`0x4f58da`), the cell cleared,
/// then `CLOSE_TABARD_FRAME` (`0x4f58f8`). An open not yet told goes first, as the reference fired
/// it at the open. `CloseTabardCreation` runs it inside its call
/// ([`crate::script_calls::ScriptCallHost`]).
pub(crate) fn close_core(world: &mut World) {
    let Some(vendor) = world.resource::<TabardOpen>().vendor else {
        return;
    };
    crate::ui_session::release(world, vendor);
    let opened = {
        let mut open = world.resource_mut::<TabardOpen>();
        open.vendor = None;
        std::mem::take(&mut open.open_event)
    };
    crate::ui_session::note_closed(world, NpcWindow::Tabard);
    crate::ui_session::fire(world, |script| {
        if opened {
            script.fire_event("OPEN_TABARD_FRAME", vec![]);
        }
        script.fire_event("CLOSE_TABARD_FRAME", vec![]);
    });
}

/// The design our own body wears while the frame is open.
#[derive(Resource, Default)]
pub(crate) struct TabardDesign(Option<[i32; 5]>);

impl TabardDesign {
    pub(crate) fn preview(&self) -> Option<GuildEmblem> {
        self.0.map(|d| GuildEmblem {
            emblem_style: d[0],
            emblem_color: d[1],
            border_style: d[2],
            border_color: d[3],
            background_color: d[4],
        })
    }
}

/// The save's fourteen checks in the reference's order (`0x5e03f0`), each a `UI_ERROR_MESSAGE`
/// key: the ten range checks share one, and the money check is the client's own
/// `ERR_NOT_ENOUGH_MONEY` (0x25), not the server's.
pub(crate) fn preflight(
    design: [i32; 5],
    record: Option<[i32; 5]>,
    guild_rank: u32,
    purse: u32,
) -> Result<[u32; 5], &'static str> {
    if design.iter().any(|v| *v < 0) {
        return Err("ERR_GUILDEMBLEM_INVALID_TABARD_COLORS");
    }
    if design
        .iter()
        .zip(TABARD_COUNTS)
        .any(|(v, count)| *v >= count)
    {
        return Err("ERR_GUILDEMBLEM_INVALID_TABARD_COLORS");
    }
    let Some(record) = record else {
        return Err("ERR_GUILDEMBLEM_NOGUILD");
    };
    if record == design {
        return Err("ERR_GUILDEMBLEM_SAME");
    }
    if guild_rank != 0 {
        return Err("ERR_GUILDEMBLEM_NOTGUILDMASTER");
    }
    if purse < TABARD_CREATION_COST {
        return Err("ERR_NOT_ENOUGH_MONEY");
    }
    Ok(design.map(|v| v as u32))
}

fn feed_tabard(
    mut vm: crate::script_calls::VmWith<ResMut<'static, TabardOpen>>,
    mut design: ResMut<TabardDesign>,
    guilds: Res<GuildState>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    mut last_host: Local<crate::ui_script::VmMemo<Option<TabardHost>>>,
    mut sink: crate::ui_action::MessageSink,
) {
    let mut lines = Vec::new();
    vm.feed(|script, mut open, fires| {
        let Some(script) = script else {
            return;
        };
        // Our guild's cached record and the latch, pushed on change.
        let guild_id = self_q.iter().next().map_or(0, |s| s.0.player_guild_id());
        let host = TabardHost {
            guild_record: guilds.own_emblem_record(guild_id),
            save_pending: open.save_pending,
        };
        let last = last_host.get(script);
        if *last != Some(host) {
            *last = Some(host);
            script.set_tabard_host(host);
        }

        // The events, in the reference's order.
        if std::mem::take(&mut open.open_event) {
            fires.post("OPEN_TABARD_FRAME", vec![]);
        }
        for _ in 0..std::mem::take(&mut open.pending_events) {
            fires.post("TABARD_SAVE_PENDING", vec![]);
        }
        // `TABARD_CANSAVE_CHANGED` is `RefreshGuildTag`'s (`0x5e0902`), raised by its callers
        // ([`crate::field_watch::guild_tag`]); a guild record landing or dropped raises nothing.

        lines = open
            .lines
            .drain(..)
            .filter_map(|key| crate::ui_action::keyed_line(script, key))
            .collect();

        // The body wears the pane's design while the designer is open;
        // `TabardModel:SetUnit("player")` mirrors that body.
        let shown = open.vendor.is_some() && script.frame_visible("TabardFrame");
        let current = shown.then(|| script.tabard_design()).flatten();
        if design.0 != current {
            design.0 = current;
        }
    });
    if !lines.is_empty() {
        vm.run(|script| {
            if let Some(script) = script {
                crate::ui_action::show_messages(script, &mut sink, "ui_tabard", lines);
            }
        });
    }
}

/// The designer's latch and what its save reads and sends through: the engine half of the tabard
/// calls ([`crate::script_calls::ScriptCallHost`]).
#[derive(SystemParam)]
pub(crate) struct TabardSteps<'w, 's> {
    /// Absent in a world with no designer, which drops the calls.
    open: Option<ResMut<'w, TabardOpen>>,
    guilds: Option<Res<'w, GuildState>>,
    self_q: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
    net: Res<'w, NetCommands>,
    sink: crate::ui_action::MessageSink<'w>,
}

impl TabardSteps<'_, '_> {
    /// The designer's cell `[0xbdcee8]`, which its close core tests (`0x4f58a0`-`0x4f58ab`).
    pub(crate) fn vendor(&self) -> Option<u64> {
        self.open.as_deref().and_then(TabardOpen::npc)
    }

    /// The close core's clear of the cell (`0x4f58df`-`0x4f58e9`), after its release: whether an
    /// open was still to be told.
    pub(crate) fn clear_vendor(&mut self) -> bool {
        let Some(open) = self.open.as_deref_mut() else {
            return false;
        };
        open.vendor = None;
        std::mem::take(&mut open.open_event)
    }

    /// `Save()` (`0x502bd0` → `0x5e03f0`) inside its call: the checks, whose refusal shows its
    /// line there, else the send, the latch and `TABARD_SAVE_PENDING` (`0x5e05eb`-`0x5e05fc`).
    /// `CloseTabardCreation`'s core needs the latch, so the host runs it.
    pub(crate) fn apply(&mut self, script: &mut UiScript, call: TabardCall) {
        let TabardCall::Save(design) = call else {
            return;
        };
        let (Some(open), Some(guilds)) = (self.open.as_deref_mut(), self.guilds.as_deref()) else {
            debug!("ui_tabard: no designer in this world; {call:?} dropped");
            return;
        };
        // The reference sends the global interaction target (`0x502a60` reads `[0xb4e2d0]`),
        // which the open set to this vendor: the same guid.
        let Some(vendor) = open.vendor else {
            return;
        };
        let store = self.self_q.iter().next();
        let guild_id = store.map_or(0, |s| s.0.player_guild_id());
        let record = guilds.own_emblem_record(guild_id);
        let rank = store.map_or(0, |s| s.0.player_guild_rank());
        let purse = store.and_then(|s| s.0.player_money()).unwrap_or(0);
        match preflight(design, record, rank, purse) {
            Err(key) => {
                if let Some(line) = crate::ui_action::keyed_line(script, key) {
                    crate::ui_action::show_now(script, &mut self.sink, "ui_tabard", line);
                }
            }
            Ok(design) => {
                let _ = self
                    .net
                    .0
                    .send(ClientCommand::SaveGuildEmblem { vendor, design });
                // The latch, then the event (`0x5e05f5`, `0x5e05fc`): a handler calling
                // `CanSaveTabardNow()` already sees nil.
                open.save_pending = true;
                script.set_tabard_host(TabardHost {
                    guild_record: record,
                    save_pending: true,
                });
                script.fire_event("TABARD_SAVE_PENDING", vec![]);
            }
        }
    }
}

fn reset_on_world_enter(
    mut entered: MessageReader<EnteredWorldMessage>,
    mut open: ResMut<TabardOpen>,
    mut design: ResMut<TabardDesign>,
) {
    if entered.read().next().is_none() {
        return;
    }
    // The leave-world teardown ran the close core (`0x490b02`); a fresh world starts clean.
    *open = TabardOpen::default();
    design.0 = None;
}

/// The tabard vendor's packet handlers.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::TabardOpen;
    use crate::net::NetHandlerApp;
    use crate::ui_guild::GuildState;

    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::TabardVendorActivate, on_activate)
            .net_handler(K::SaveGuildEmblemResult, on_save_result);
    }

    fn on_activate(In(ev): In<SessionEvent>, mut commands: Commands) {
        if let SessionEvent::TabardVendorActivate(vendor) = ev {
            commands.queue(move |world: &mut World| super::open_tabard(world, vendor));
        }
    }

    /// A saved emblem evicts our guild's cached record (`0x5e715f`), with no event or packet; the
    /// next query re-fetches it.
    fn on_save_result(
        In(ev): In<SessionEvent>,
        mut tabard: ResMut<TabardOpen>,
        mut guild: ResMut<GuildState>,
    ) {
        if let SessionEvent::SaveGuildEmblemResult(result) = ev {
            if tabard.apply_result(result) {
                guild.evict_own_identity();
            }
        }
    }
}

pub(crate) struct TabardUiPlugin;

impl Plugin for TabardUiPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<TabardOpen>()
            .init_resource::<TabardDesign>()
            .add_latched_window::<TabardOpen>()
            // `0x490b02` → `0x4f5830` → `0x4f58a0`, the core.
            .add_sweep_leg(SweepLeg::Tabard, close_core)
            .add_systems(
                Update,
                (
                    // Moves what the save's call reads, on the drain's session edge: after the
                    // drain and before the pass resumes.
                    reset_on_world_enter
                        .after(benilla_world::schedule::WorldStage::Net)
                        .before(crate::vm_pass::VmPass::Resume),
                    feed_tabard
                        .in_set(UiFeed)
                        .in_set(crate::ui_session::NpcWindowFeed),
                ),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fourteen_checks_run_in_the_references_order() {
        let rec = Some([1, 2, 3, 4, 5]);
        assert_eq!(
            preflight([-1, 0, 0, 0, 0], rec, 0, 200_000),
            Err("ERR_GUILDEMBLEM_INVALID_TABARD_COLORS")
        );
        assert_eq!(
            preflight([170, 0, 0, 0, 0], rec, 0, 200_000),
            Err("ERR_GUILDEMBLEM_INVALID_TABARD_COLORS")
        );
        assert_eq!(
            preflight([0, 0, 0, 0, 51], rec, 0, 200_000),
            Err("ERR_GUILDEMBLEM_INVALID_TABARD_COLORS")
        );
        assert_eq!(
            preflight([0; 5], None, 0, 200_000),
            Err("ERR_GUILDEMBLEM_NOGUILD")
        );
        assert_eq!(
            preflight([1, 2, 3, 4, 5], rec, 0, 200_000),
            Err("ERR_GUILDEMBLEM_SAME")
        );
        assert_eq!(
            preflight([0; 5], rec, 1, 200_000),
            Err("ERR_GUILDEMBLEM_NOTGUILDMASTER")
        );
        assert_eq!(
            preflight([0; 5], rec, 0, 99_999),
            Err("ERR_NOT_ENOUGH_MONEY")
        );
        assert_eq!(
            preflight([169, 16, 5, 16, 50], rec, 0, 100_000),
            Ok([169, 16, 5, 16, 50])
        );
        // An undesigned record (-1s) is a cached record: the same check compares and passes.
        assert_eq!(preflight([0; 5], Some([-1; 5]), 0, 100_000), Ok([0; 5]));
    }

    #[test]
    fn the_reply_clears_the_latch_shows_its_row_and_refires_pending_on_failure() {
        let mut t = TabardOpen {
            save_pending: true,
            ..Default::default()
        };
        assert!(t.apply_result(0), "success");
        assert!(!t.save_pending);
        assert_eq!(t.lines, vec!["ERR_GUILDEMBLEM_SUCCESS"]);
        assert_eq!(t.pending_events, 0);
        t.save_pending = true;
        assert!(!t.apply_result(3));
        assert_eq!(t.lines.last(), Some(&"ERR_GUILDEMBLEM_NOTGUILDMASTER"));
        assert_eq!(t.pending_events, 1);
        t.save_pending = true;
        assert!(!t.apply_result(5), "the sentinel row shows nothing");
        assert_eq!(t.lines.len(), 2);
        assert!(!t.save_pending);
        t.save_pending = true;
        assert!(
            !t.apply_result(6),
            "past the table: ignored, latch untouched"
        );
        assert!(t.save_pending);
    }

    /// `TABARD_CANSAVE_CHANGED` is `RefreshGuildTag`'s alone (`0x5e0902`), from its three callers:
    /// a guild record landing, ours or another's, and the emblem save's drop of ours (`0x5e715f`)
    /// signal nothing.
    #[test]
    fn a_landed_or_dropped_guild_record_signals_no_tabard_change() {
        use crate::ui_script::sweep_test::{seen_in, spied_vm};
        let mut app = App::new();
        app.init_resource::<TabardOpen>()
            .init_resource::<TabardDesign>()
            .init_resource::<GuildState>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .add_systems(Update, feed_tabard)
            .insert_non_send_resource(spied_vm(&["TABARD_CANSAVE_CHANGED"]));
        app.update();
        let mut guild = app.world_mut().resource_mut::<GuildState>();
        guild.join_for_test(5);
        guild.land_for_test(5, "Probe Guild");
        app.update();
        app.world_mut()
            .resource_mut::<GuildState>()
            .land_for_test(6, "Strangers");
        app.update();
        app.world_mut()
            .resource_mut::<GuildState>()
            .evict_own_identity();
        app.update();
        assert!(seen_in(&app).is_empty(), "{:?}", seen_in(&app));
    }

    /// The core fires only with a vendor stored, twice while the designer holds the latch, and
    /// tells an open not yet fed first.
    #[test]
    fn the_close_core_fires_only_with_a_vendor_stored() {
        use crate::ui_script::sweep_test::{seen_in, spied_vm};
        const VENDOR: u64 = 0xF130_0000_0000_0042;
        let mut app = App::new();
        app.init_resource::<TabardOpen>()
            .add_latched_window::<TabardOpen>()
            .insert_non_send_resource(spied_vm(&["OPEN_TABARD_FRAME", "CLOSE_TABARD_FRAME"]));
        close_core(app.world_mut());
        assert!(seen_in(&app).is_empty(), "no vendor, no close");

        open_tabard(app.world_mut(), VENDOR);
        close_core(app.world_mut());
        assert_eq!(
            seen_in(&app),
            [
                "OPEN_TABARD_FRAME",
                "CLOSE_TABARD_FRAME",
                "CLOSE_TABARD_FRAME"
            ]
        );
        assert_eq!(app.world().resource::<TabardOpen>().npc(), None);
        close_core(app.world_mut());
        assert_eq!(seen_in(&app).len(), 3, "a second close is silent");
    }

    /// `Save` (`0x502bd0` → `0x5e03f0`) runs the checks, sends `MSG_SAVE_GUILD_EMBLEM`, sets the
    /// latch and signals `TABARD_SAVE_PENDING` before it returns (`0x5e05eb`-`0x5e05fc`), and a
    /// refusal shows its line there.
    #[test]
    fn a_save_checks_sends_and_signals_inside_its_call() {
        use crate::net::{ClientCommand, NetCommands, ObjectStore};
        use crate::script_calls::with_hosted_vm;
        use benilla_ui::script::{TabardHost, UnitState};
        const VENDOR: u64 = 0xF130_0000_0000_0042;
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
        let (tx, rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        let world = app.world_mut();
        world.spawn((
            crate::net::SelfPlayer,
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (191, 5),
                (192, 0),
                (1176, 200_000),
            ])),
        ));
        {
            let mut guild = world.resource_mut::<GuildState>();
            guild.join_for_test(5);
            guild.land_for_test(5, "Probe Guild");
        }
        world.resource_mut::<TabardOpen>().open(VENDOR);
        let mut script = UiScript::new().expect("a VM");
        script.set_unit(
            "player",
            Some(UnitState {
                exists: true,
                ..Default::default()
            }),
        );
        script.set_tabard_host(TabardHost {
            guild_record: Some([1, 2, 3, 4, 5]),
            save_pending: false,
        });
        script
            .run(
                r#"HEARD = ""
                ERR_NOT_ENOUGH_MONEY = "Not enough money."
                local f = CreateFrame("Frame")
                f:RegisterEvent("TABARD_SAVE_PENDING")
                f:RegisterEvent("UI_ERROR_MESSAGE")
                f:SetScript("OnEvent", function()
                    HEARD = HEARD .. event .. "(" .. tostring(arg1) .. ") "
                end)
                TM = CreateFrame("TabardModel", "TM")
                TM:InitializeTabardColors()
                TM:CycleVariation(1, 1)"#,
            )
            .expect("the designer seeds");
        world.insert_non_send_resource(script);
        let run = |app: &mut App, lua: &str| {
            with_hosted_vm(app.world_mut(), |vm| vm.expect("a VM").run(lua))
                .expect("the chunk runs");
        };
        let eval = |app: &mut App, lua: &str| -> String {
            with_hosted_vm(app.world_mut(), |vm| vm.expect("a VM").eval::<String>(lua))
                .expect("the chunk runs")
        };
        let sent = |rx: &crossbeam_channel::Receiver<ClientCommand>| -> Vec<String> {
            rx.try_iter()
                .map(|c| format!("{c:?}"))
                .filter(|c| !c.starts_with("GuildQuery"))
                .collect()
        };
        run(&mut app, "TM:Save() SEEN = HEARD HEARD = ''");
        assert_eq!(eval(&mut app, "return SEEN"), "TABARD_SAVE_PENDING(nil) ");
        assert_eq!(
            sent(&rx),
            [format!(
                "SaveGuildEmblem {{ vendor: {VENDOR}, design: [2, 2, 3, 4, 5] }}"
            )]
        );
        assert!(app.world().resource::<TabardOpen>().save_pending);

        // The purse below the price: the client's own `ERR_NOT_ENOUGH_MONEY`, in the call.
        app.world_mut().resource_mut::<TabardOpen>().save_pending = false;
        let mut me = app
            .world_mut()
            .query_filtered::<&mut ObjectStore, With<crate::net::SelfPlayer>>();
        me.single_mut(app.world_mut()).expect("our store").0.merge(
            benilla_protocol::ObjectFields::from_pairs(&[(1176, 99_999)]),
        );
        run(&mut app, "TM:Save() SEEN = HEARD HEARD = ''");
        assert_eq!(
            eval(&mut app, "return SEEN"),
            "UI_ERROR_MESSAGE(Not enough money.) "
        );
        assert!(sent(&rx).is_empty());
    }

    /// An activate from the vendor whose designer is open writes the cell before it registers,
    /// so `0x493310` finds the designer and closes it, cell and all: it reopens holding the latch
    /// with no cell.
    #[test]
    fn a_second_activate_from_the_open_vendor_closes_the_designer_cell_and_all() {
        const VENDOR: u64 = 0xF130_0000_0000_0042;
        let mut app = App::new();
        app.init_resource::<TabardOpen>()
            .add_latched_window::<TabardOpen>();
        open_tabard(app.world_mut(), VENDOR);
        open_tabard(app.world_mut(), VENDOR);
        assert_eq!(app.world().resource::<TabardOpen>().npc(), None);
        assert_eq!(
            app.world()
                .resource::<crate::ui_session::InteractLatch>()
                .guid(),
            Some(VENDOR)
        );
        assert!(app.world().resource::<TabardOpen>().open_event);
    }
}
