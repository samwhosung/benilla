//! The guild tabard designer, the app half of the stock `TabardFrame.xml`. Only
//! `MSG_TABARDVENDOR_ACTIVATE` opens it (`0x5e70c0` → `0x4f5840`); no Lua verb does. Our own body
//! wears the design over a forced tabard geoset (`[cc+0xc]`, `0x47a610`).
//!
//! `CLOSE_TABARD_FRAME` fires once per close; the reference fires it twice on the Lua route, a
//! re-entrancy (`0x4f58a0 → 0x4931c0 → 0x493310 → 0x4934a0`) the stock `HideUIPanel` absorbs.

use bevy::prelude::*;

use benilla_formats::GuildEmblem;
use benilla_protocol::messages::GUILD_EMBLEM_RESULT_MESSAGES;
use benilla_ui::script::{TabardHost, TabardIntent, UiScript, TABARD_COUNTS, TABARD_CREATION_COST};

use crate::net::{ClientCommand, EnteredWorldMessage, NetCommands, ObjectStore, SelfPlayer};
use crate::portrait::PaperDollBooth;
use crate::ui_guild::GuildState;
use crate::ui_script::{UiFeed, UiInput};
use crate::ui_session::{close_npc_session_out_of_range, NpcSession};

/// The open designer: its vendor (`[0xbdcee8]`), the save-in-flight latch (`[0xc4d780]`), and
/// the events and lines owed to the next feed.
#[derive(Resource, Default)]
pub(crate) struct TabardOpen {
    vendor: Option<u64>,
    save_pending: bool,
    open_event: bool,
    close_event: bool,
    pending_events: u32,
    lines: Vec<&'static str>,
}

impl TabardOpen {
    /// `MSG_TABARDVENDOR_ACTIVATE` in (`0x4f5840`).
    pub(crate) fn open(&mut self, vendor: u64) {
        self.vendor = Some(vendor);
        self.open_event = true;
    }

    /// The close core (`0x4f58a0`) that `CloseTabardCreation()`, the walk-away leash and the
    /// world leave share; it fires only with a vendor stored.
    fn close_core(&mut self) {
        if self.vendor.take().is_some() {
            self.close_event = true;
        }
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

impl NpcSession for TabardOpen {
    fn npc(&self) -> Option<u64> {
        self.vendor
    }
    fn close(&mut self) {
        self.close_core();
    }
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
    script: Option<NonSendMut<UiScript>>,
    mut open: ResMut<TabardOpen>,
    mut design: ResMut<TabardDesign>,
    mut booth: ResMut<PaperDollBooth>,
    guilds: Res<GuildState>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    commands: Res<NetCommands>,
    mut last_host: Local<crate::ui_script::VmMemo<Option<TabardHost>>>,
    mut last_identity: Local<crate::ui_script::VmMemo<Option<u64>>>,
    mut sink: crate::ui_action::MessageSink,
) {
    let Some(mut script) = script else {
        return;
    };
    // Our guild's cached record (a miss sends the query) and the latch, pushed on change.
    let guild_id = self_q.iter().next().map_or(0, |s| s.0.player_guild_id());
    let host = TabardHost {
        guild_record: guilds.own_emblem_record(guild_id, &commands),
        save_pending: open.save_pending,
    };
    let last = last_host.get(&script);
    if *last != Some(host) {
        *last = Some(host);
        script.set_tabard_host(host);
    }

    // The events, in the reference's order.
    if std::mem::take(&mut open.open_event) {
        script.fire_event("OPEN_TABARD_FRAME", vec![]);
    }
    if std::mem::take(&mut open.close_event) {
        script.fire_event("CLOSE_TABARD_FRAME", vec![]);
    }
    for _ in 0..std::mem::take(&mut open.pending_events) {
        script.fire_event("TABARD_SAVE_PENDING", vec![]);
    }
    // `TABARD_CANSAVE_CHANGED` fires when a guild record arrives (`0x5e08e0`); here, when our
    // identity cache moves.
    let gen = guilds.identity_generation();
    let last_gen = last_identity.get(&script);
    if last_gen.is_some() && *last_gen != Some(gen) {
        script.fire_event("TABARD_CANSAVE_CHANGED", vec![]);
    }
    *last_gen = Some(gen);

    let lines: Vec<_> = open
        .lines
        .drain(..)
        .filter_map(|key| crate::ui_action::keyed_line(&script, key))
        .collect();
    crate::ui_action::show_messages(&mut script, &mut sink, "ui_tabard", lines);

    // The body wears the pane's design while the designer is open; the pane's yaw turns the booth.
    let shown = open.vendor.is_some() && script.frame_visible("TabardFrame");
    let current = shown.then(|| script.tabard_design()).flatten();
    if design.0 != current {
        design.0 = current;
    }
    if shown {
        let yaw = script.model_pane_facing("TabardModel");
        if booth.yaw != yaw {
            booth.yaw = yaw;
        }
    }
}

fn drain_tabard(
    script: Option<NonSendMut<UiScript>>,
    mut open: ResMut<TabardOpen>,
    guilds: Res<GuildState>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    commands: Res<NetCommands>,
    mut sink: crate::ui_action::MessageSink,
) {
    let Some(mut script) = script else {
        return;
    };
    for intent in script.take_tabard_intents() {
        match intent {
            TabardIntent::Close => open.close_core(),
            TabardIntent::Save(design) => {
                // The reference sends the global interaction target (`0x502a60` reads
                // `[0xb4e2d0]`), which the open set to this vendor: the same guid.
                let Some(vendor) = open.vendor else {
                    continue;
                };
                let store = self_q.iter().next();
                let guild_id = store.map_or(0, |s| s.0.player_guild_id());
                let record = guilds.own_emblem_record(guild_id, &commands);
                let rank = store.map_or(0, |s| s.0.player_guild_rank());
                let purse = store.and_then(|s| s.0.player_money()).unwrap_or(0);
                match preflight(design, record, rank, purse) {
                    Err(key) => {
                        if let Some(line) = crate::ui_action::keyed_line(&script, key) {
                            crate::ui_action::show_messages(
                                &mut script,
                                &mut sink,
                                "ui_tabard",
                                [line],
                            );
                        }
                    }
                    Ok(design) => {
                        let _ = commands
                            .0
                            .send(ClientCommand::SaveGuildEmblem { vendor, design });
                        // The latch, then the event (`0x5e05f5`, `0x5e05fc`): a handler
                        // calling `CanSaveTabardNow()` already sees nil.
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

    fn on_activate(In(ev): In<SessionEvent>, mut tabard: ResMut<TabardOpen>) {
        if let SessionEvent::TabardVendorActivate(vendor) = ev {
            tabard.open(vendor);
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
            .add_systems(
                Update,
                (
                    reset_on_world_enter.before(feed_tabard),
                    close_npc_session_out_of_range::<TabardOpen>.before(feed_tabard),
                    feed_tabard.in_set(UiFeed),
                    drain_tabard.after(UiInput),
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

    #[test]
    fn the_close_core_fires_only_with_a_vendor_stored() {
        let mut t = TabardOpen::default();
        t.close_core();
        assert!(!t.close_event);
        t.open(0xF130_0000_0000_0042);
        assert!(t.open_event);
        t.close_core();
        assert!(t.close_event && t.vendor.is_none());
        t.close_event = false;
        t.close_core();
        assert!(!t.close_event, "a second close is silent");
    }
}
