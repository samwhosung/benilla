//! The guild tabard designer (decision 1977):
//! the app half of the stock `TabardFrame.xml` — the vendor session, the body preview, the save's
//! pre-flight checks and wire, the reply, and the four events.
//!
//! **The frame opens only from the wire.** `MSG_TABARDVENDOR_ACTIVATE` in (`0x5e70c0` → `0x4f5840`)
//! stores the vendor guid, raises the designer-preview flag on the local player's character
//! component, sets the global interaction target, and fires `OPEN_TABARD_FRAME`; there is no Lua
//! verb that opens it. The stock handler then `SetUnit("player")`s the pane, seeds the design
//! (`InitializeTabardColors`, the engine's) and `ShowUIPanel`s.
//!
//! **The body preview.** The five under design replace the guild's emblem on OUR body, over the
//! tabard geoset forced on with an empty slot (`Equipment::tabard_preview` → the composite and the
//! geoset law; `[cc+0xc]` and `0x47a610` in the reference). The `TabardModel` pane samples the
//! paper-doll booth, which bakes that same body — the reference's pane is a `PlayerModel` of the
//! player, so what the world shows and what the pane shows are one look.
//!
//! **`Save()`** runs the reference's fourteen pre-flight checks in order (`0x5e03f0`), each a
//! `UI_ERROR_MESSAGE` line — none silent — then sends `MSG_SAVE_GUILD_EMBLEM` and fires
//! `TABARD_SAVE_PENDING` with the latch already set. The reply's `u32` clears the latch and shows
//! its table row; a failure fires `TABARD_SAVE_PENDING` again; success evicts the guild record so
//! the next query anywhere re-fetches it (no event, no packet — `0x5e715f`).
//!
//! **Close.** `CloseTabardCreation()` (the OnHide's call), the walk-away leash and the world-leave
//! run one close core: with a vendor stored, drop the preview flag, clear the guid, fire
//! `CLOSE_TABARD_FRAME`; with none, nothing. The reference fires the event twice on the Lua route
//! (a re-entrancy through the interaction-target clear, `0x4f58a0 → 0x4931c0 → 0x493310 →
//! 0x4934a0`) — a shape the stock `HideUIPanel` handler absorbs and this close does not reproduce.

use bevy::prelude::*;

use benilla_formats::GuildEmblem;
use benilla_protocol::messages::GUILD_EMBLEM_RESULT_MESSAGES;
use benilla_ui::script::{TabardHost, TabardIntent, UiScript, TABARD_COUNTS, TABARD_CREATION_COST};

use crate::net::{ClientCommand, EnteredWorldMessage, NetCommands, ObjectStore, SelfPlayer};
use crate::portrait::PaperDollBooth;
use crate::ui_guild::GuildState;
use crate::ui_script::{UiFeed, UiInput};
use crate::ui_session::{close_npc_session_out_of_range, NpcSession};

/// The open designer: the vendor it was opened on (`[0xbdcee8]`), and the save-in-flight latch
/// (`[0xc4d780]`).
#[derive(Resource, Default)]
pub(crate) struct TabardOpen {
    vendor: Option<u64>,
    save_pending: bool,
    /// `OPEN_TABARD_FRAME` owed to the next feed.
    open_event: bool,
    /// `CLOSE_TABARD_FRAME` owed to the next feed.
    close_event: bool,
    /// `TABARD_SAVE_PENDING` fires owed to the next feed.
    pending_events: u32,
    /// The reply's line, keyed, owed to the next feed.
    lines: Vec<&'static str>,
}

impl TabardOpen {
    /// `MSG_TABARDVENDOR_ACTIVATE` in — `0x4f5840`.
    pub(crate) fn open(&mut self, vendor: u64) {
        self.vendor = Some(vendor);
        self.open_event = true;
    }

    /// The close core `0x4f58a0`: only with a vendor stored.
    fn close_core(&mut self) {
        if self.vendor.take().is_some() {
            self.close_event = true;
        }
    }

    /// `MSG_SAVE_GUILD_EMBLEM` in — `0x5e70f0`: the latch clears first; a result past the six-row
    /// table is ignored; the row's line shows unless it is the sentinel; a failure re-fires
    /// `TABARD_SAVE_PENDING`. Returns whether the save SUCCEEDED (the caller evicts the record).
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

/// The five under design while the frame is open — what the local player's body wears
/// (`entities::equipment::resolve`). `None` closes the preview.
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

/// The sender's fourteen rejections in order (`0x5e03f0`), each a `UI_ERROR_MESSAGE` key — the
/// ten range checks share one line; the money check is the client's `ERR_NOT_ENOUGH_MONEY`
/// (0x25), not the server's `ERR_GUILDEMBLEM_NOTENOUGHMONEY`. `Ok` means send.
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
    // The host facts: our guild's cached record (a lazy-cache read — a miss sends the query) and
    // the latch. Pushed on change so the getters read the current state on every call.
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

    // The events, in the order the reference's flow produces them.
    if std::mem::take(&mut open.open_event) {
        script.fire_event("OPEN_TABARD_FRAME", vec![]);
    }
    if std::mem::take(&mut open.close_event) {
        script.fire_event("CLOSE_TABARD_FRAME", vec![]);
    }
    for _ in 0..std::mem::take(&mut open.pending_events) {
        script.fire_event("TABARD_SAVE_PENDING", vec![]);
    }
    // `TABARD_CANSAVE_CHANGED`: the guild-appearance refresh that a guild record's arrival runs
    // on every member in sight (`0x5e08e0`, its second act) — here, our identity cache moving.
    let gen = guilds.identity_generation();
    let last_gen = last_identity.get(&script);
    if last_gen.is_some() && *last_gen != Some(gen) {
        script.fire_event("TABARD_CANSAVE_CHANGED", vec![]);
    }
    *last_gen = Some(gen);

    // The reply's line.
    let lines: Vec<_> = open
        .lines
        .drain(..)
        .filter_map(|key| crate::ui_action::keyed_line(&script, key))
        .collect();
    crate::ui_action::show_messages(&mut script, &mut sink, "ui_tabard", lines);

    // The body preview follows the pane's five while the designer is open; the pane's yaw drives
    // the paper-doll booth it samples.
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
                // The vendor guid is the global UI interaction target, which the open set to
                // this vendor (`0x502a60` reads `[0xb4e2d0]`); ours is the session's stored guid,
                // the same value.
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
                        // `0x5e05f5` then `0x5e05fc`: the latch, then the event — a handler
                        // reading `CanSaveTabardNow()` inside it already sees nil.
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

/// The tabard vendor's packet handlers (in the net handler table since 2312), beside the state
/// they drive.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::TabardOpen;
    use crate::net::NetHandlerApp;
    use crate::ui_guild::GuildState;

    /// Register the two handlers — called from [`super::TabardUiPlugin`].
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

    /// A saved emblem evicts our guild's cached record (`0x5e715f`): the next query anywhere
    /// re-fetches it — no event, no packet.
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
        // An undesigned record (-1s) is a cached record: the SAME check compares and passes.
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
