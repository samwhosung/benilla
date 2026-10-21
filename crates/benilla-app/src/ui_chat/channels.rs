//! The zone-channel auto-join, the reference's `ZoneChannelRefresh 0x49a210`. The server joins
//! nobody (vmangos `Player::UpdateLocalChannels` is empty, `Objects/Player.cpp:5121`), so the
//! client joins every [`ChannelState::zone_mask`] row composed for the current zone, and on each
//! zone change leaves and re-joins the zone-named ones, LEAVE then JOIN per slot.
//!
//! The join's `CHAT_MSG_CHANNEL_NOTICE` is also one of the events that trigger AceEvent-2.0's
//! `AceEvent_FullyInitialized`, which many Ace2 addons wait for.

use bevy::prelude::*;

use benilla_assets::LockRecover;
use benilla_formats::ChatChannelsCatalog;

use crate::area::AreaTableRes;
use crate::net::{ClientCommand, NetCommands};

use super::edit::{ChannelState, SlotState};
use super::recruitment::GuildRecruitmentCascade;

/// `AreaTable.dbc` flag `0x08`, the walk's city gate (`0x49a3c1`), set on the six capitals;
/// vmangos names it `AREA_FLAG_SLAVE_CAPITAL`, "Allow trade channel" (`DBCEnums.h:58`).
const AREA_FLAG_TRADE_CHANNEL: u32 = 0x08;

/// `AreaTable.dbc` flag `0x100`, a capital, on the six capitals: a city-named row's `%s` is the
/// city word only in an area carrying it (`0x49a2df`, `0x49a4c1`, `0x49eccc`), and the guild
/// recruitment cascade joins only there (`0x49eb2e`). vmangos names it `AREA_FLAG_CAPITAL`.
pub(super) const AREA_FLAG_CAPITAL: u32 = 0x100;

/// `AreaTable.dbc` flag `0x200`: the one row (id 3459, "City"), cached at load (`0xb4e4f0`,
/// written at `0x4985fd`), whose localized name the city-named channels take.
const AREA_FLAG_CITY_NAME_ROW: u32 = 0x200;

/// The city word, as the client's load-time scan finds it; `None` when no row carries `0x200`,
/// where a city-named row takes the zone's name (`0x49a2eb`).
pub(crate) fn city_word(areas: &benilla_formats::AreaTableCatalog) -> Option<&str> {
    areas
        .rows()
        .find(|r| r.flags & AREA_FLAG_CITY_NAME_ROW != 0)
        .map(|r| r.name.as_str())
}

/// The walk's own state; the joined list is not copied here, as the reference's pass 1 walks the
/// slot array itself ([`ChannelState::joined`]).
#[derive(Resource, Default)]
pub(crate) struct ZoneChannelWalk {
    /// The top-level zone the last walk ran for; `None` walks from scratch.
    at: Option<u32>,
    /// The zone the walk may act on this frame, the reference's `ds:0xb4e314`, read by the walk
    /// (`0x49a243`) and the guild-recruitment cascade (`0x49ead7`); `None` while a gate holds.
    pub(super) zone_id: Option<u32>,
    /// A character session is live: without it the logout frame, still `InWorld` with `at` just
    /// cleared, would join the zone being left.
    live: bool,
}

impl ZoneChannelWalk {
    /// Walk again at the next settled frame, whatever the zone: the cache loader's call
    /// (`0x499a22`), which runs the deferred joins it just seated.
    pub(super) fn rewalk(&mut self) {
        self.at = None;
    }

    /// Forget the session: the next world entry re-joins from nothing.
    fn clear_session(&mut self) {
        self.at = None;
        self.zone_id = None;
        self.live = false;
    }
}

/// End the channel session: vmangos leaves every channel with the session
/// (`Player::CleanupChannels`, `Objects/Player.cpp:5107`), so the next one starts in none. The
/// mask stays, as it belongs to the character's file.
fn end_channel_session(
    script: Option<&mut benilla_ui::script::UiScript>,
    channels: &mut ChannelState,
    walk: &mut ZoneChannelWalk,
    cascade: &mut GuildRecruitmentCascade,
) {
    walk.clear_session();
    cascade.clear_session();
    channels.joined.clear();
    channels.deferred_joins.clear();
    if let Some(script) = script {
        script.set_joined_channels(Vec::new());
    }
}

/// Seed a fresh VM's joined-channel mirror, which is otherwise pushed only on a join or a leave, so
/// a `/reload` would leave `GetChannelName` and `/N` empty until the next one.
pub(super) fn seed_channels(
    mut vm: crate::script_calls::VmWith<Res<'static, ChannelState>>,
    mut seeded: Local<crate::ui_script::VmMemo<bool>>,
) {
    vm.turn(|script, channels| {
        let Some(script) = script else {
            return;
        };
        if seeded.claim(script) {
            channels.mirror_into(script);
        }
    })
}

/// The session-end edge: a confirmed `/logout` back to the glue screens (`OnExit(InWorld)`).
pub(super) fn end_session_channels(
    mut vm: crate::script_calls::VmWith<ResMut<'static, ChannelState>>,
    mut walk: ResMut<ZoneChannelWalk>,
    mut cascade: ResMut<GuildRecruitmentCascade>,
) {
    vm.turn(|script, mut channels| {
        end_channel_session(script, &mut channels, &mut walk, &mut cascade);
    })
}

/// The other session end, a dead socket: a recoverable drop never leaves `InWorld`, but the
/// reconnect gets a fresh server-side `Player` in no channels, so it clears the same state.
pub(super) fn end_session_channels_on_disconnect(
    mut vm: crate::script_calls::VmWith<ResMut<'static, ChannelState>>,
    mut walk: ResMut<ZoneChannelWalk>,
    mut cascade: ResMut<GuildRecruitmentCascade>,
    mut disconnects: MessageReader<crate::net::DisconnectedMessage>,
) {
    if disconnects.read().next().is_none() {
        return;
    }
    vm.turn(|script, mut channels| {
        end_channel_session(script, &mut channels, &mut walk, &mut cascade);
    })
}

/// Startup: read `ChatChannels.dbc`; without an install nothing auto-joins and arg7 stays 0.
pub(super) fn load_chat_channels(
    mut channels: ResMut<ChannelState>,
    assets: Option<Res<benilla_assets::WorldAssets>>,
) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_chat_channels_catalog(&mut chain)
    };
    match loaded {
        Ok(cat) => {
            info!("chat: {} ChatChannels rows", cat.rows().len());
            channels.channels = cat;
        }
        Err(e) => warn!("chat: ChatChannels.dbc failed to load — no zone channels: {e:#}"),
    }
}

/// The name `row` composes to in `zone_name`, an area with `area_flags`: the pattern with the
/// city word for a city-named row (`0x20`) in a capital (`0x100`) when a city row exists, else
/// with the zone's name (`0x49a2d0`-`0x49a2f6`, `0x49a4af`-`0x49a4d8`, `0x49ecb6`-`0x49ece0`).
/// `None` when that word is empty, where the join and leave bodies send nothing for any row
/// (`0x49ece3`-`0x49ecea`, `0x49efca`-`0x49efd5`) and the walk does not run (`0x49a235`).
///
/// Deviation: in a capital with a blank city word the walk composes `"Trade - "` (it tests the
/// row pointer, not the string, `0x49a2e4`-`0x49a2ed`); we decline, because a wrong channel name
/// is joined silently and for good.
fn compose(
    row: &benilla_formats::ChatChannelRow,
    zone_name: &str,
    area_flags: u32,
    city_word: Option<&str>,
) -> Option<String> {
    let word = match city_word {
        Some(city) if row.takes_city_name() && area_flags & AREA_FLAG_CAPITAL != 0 => city,
        _ => zone_name,
    };
    (!word.is_empty()).then(|| row.joinable_name(word))
}

/// The names the walk registers on a character with no slots, in table order: the reference
/// registers a slot (`0x49a50d`) before its city gate (`0x49a512`), so a city-only row takes its
/// number even outside a capital. The `N.` of a channel line is that slot, never the ChannelID.
#[cfg(test)]
pub(crate) fn tracked_channels(
    catalog: &ChatChannelsCatalog,
    zone_mask: u32,
    zone_name: &str,
    area_flags: u32,
    city_word: Option<&str>,
) -> Vec<String> {
    if zone_name.is_empty() {
        return Vec::new(); // the walk's own bail (`0x49a235`)
    }
    catalog
        .rows()
        .iter()
        .filter(|r| zone_mask & super::edit::zone_bit(r.id) != 0)
        .filter_map(|r| compose(r, zone_name, area_flags, city_word))
        .collect()
}

/// [`tracked_channels`] with the city gate applied: the names the walk joins. A city-only row
/// (`0x10`) is joined only in an area with `0x8` (`0x49a512`-`0x49a51f`).
#[cfg(test)]
pub(crate) fn wanted_channels(
    catalog: &ChatChannelsCatalog,
    zone_mask: u32,
    zone_name: &str,
    area_flags: u32,
    city_word: Option<&str>,
) -> Vec<String> {
    if zone_name.is_empty() {
        return Vec::new(); // the walk's own bail (`0x49a235`)
    }
    let in_city = area_flags & AREA_FLAG_TRADE_CHANNEL != 0;
    catalog
        .rows()
        .iter()
        .filter(|r| zone_mask & super::edit::zone_bit(r.id) != 0)
        .filter(|r| in_city || !r.is_city_only())
        .filter_map(|r| compose(r, zone_name, area_flags, city_word))
        .collect()
}

/// One step of a walk, in wire order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WalkStep {
    /// `CMSG_LEAVE_CHANNEL` of the old name (`0x49a367`), from a confirmed slot only (`0x49a35e`).
    Leave(String),
    /// `CMSG_JOIN_CHANNEL` (`0x49a3e1` in pass 1, `0x49a536` in pass 2, `0x49a5e2` in pass 3).
    Join(String),
    /// `0x49bc50`: renamed in place at send time ([`ChannelState::rename_slot`]).
    Rename { old: String, new: String },
    /// `0x49b980`: a slot claimed with its ChannelID, before the city gate (`0x49a50d`), or with
    /// id 0 for a deferred join (`0x49a5da`).
    Register { name: String, id: u32 },
    /// `0x49bcf0`: state 3, the row stopped applying; the record and its number survive.
    Suspend(String),
}

/// The walk: `ZoneChannelRefresh 0x49a210`'s three passes, for a player in `zone_name`, an area
/// with `area_flags`, each step written to the slots as it is taken, so a row reads the table as
/// the steps before it left it: General's register can reuse a freed record and overwrite its id
/// (`0x49b9b0`, `0x49ba01`), which LocalDefense's by-id lookup then no longer finds. Nothing at all
/// with no zone text (`0x49a235`, `0x49a23d`). The steps come back in wire order.
///
/// - Pass 1 (`0x49a284`) takes each slot with a number and a ChannelID naming a row
///   (`0x49a295`-`0x49a2ca`), in slot order, and composes its name for this zone whatever the mask
///   says. An unchanged name is left alone unless the slot is suspended (`0x49a31c`, `0x49a337`).
///   Otherwise a confirmed slot leaves (`0x49a35e`), then a city-only row (`0x10`) in an area
///   without `0x8` suspends (`0x49a3b8`-`0x49a3cb`, a no-op on a suspended slot) and any other is
///   renamed and re-joined (`0x49a3dc`-`0x49a410`).
/// - Pass 2 (`0x49a468`) takes each row whose mask bit is set (`0x49a494`), that no record
///   carries by id, a freed one included (`0x49bef0`, `0x49a4a2`), and whose composed name no slot
///   holds (`0x49be90`), in DBC order: it registers the slot (`0x49a50d`), then suspends it or
///   joins by the same city gate (`0x49a512`-`0x49a577`).
/// - Pass 3 (`0x49a5b4`) registers and joins each deferred name with id 0 (`0x49a5da`, `0x49a5e2`).
///
/// A registration with all ten slots taken shows its error and the join still goes out
/// (`0x49a50d`, `0x49a5da` → `0x49b9c5`).
pub(crate) fn run_walk(
    channels: &mut ChannelState,
    zone_name: &str,
    area_flags: u32,
    city_word: Option<&str>,
    errors: &crate::ui_action::UiErrorKeys,
) -> Vec<WalkStep> {
    use WalkStep as W;
    if zone_name.is_empty() {
        return Vec::new();
    }
    let in_city = area_flags & AREA_FLAG_TRADE_CHANNEL != 0;
    let eligible = |row: &benilla_formats::ChatChannelRow| in_city || !row.is_city_only();
    let mut steps = Vec::new();
    let mut take = |channels: &mut ChannelState, step: WalkStep| {
        write_slot(&step, channels, errors);
        steps.push(step);
    };

    // Pass 1.
    for i in 0..channels.joined.len() {
        let Some(slot) = channels.joined[i].held().cloned() else {
            continue;
        };
        let Some(row) = channels.channels.row(slot.id).cloned() else {
            continue; // a custom channel: not the walk's
        };
        let old = slot.name;
        let Some(new) = compose(&row, zone_name, area_flags, city_word) else {
            continue; // the deviation in `compose`: no name to join
        };
        if slot.state != SlotState::Suspended && new.eq_ignore_ascii_case(&old) {
            continue;
        }
        if slot.state == SlotState::Confirmed {
            take(channels, W::Leave(old.clone()));
        }
        if eligible(&row) {
            take(
                channels,
                W::Rename {
                    old,
                    new: new.clone(),
                },
            );
            take(channels, W::Join(new));
        } else if slot.state != SlotState::Suspended {
            take(channels, W::Suspend(old));
        }
    }

    // Pass 2.
    for i in 0..channels.channels.rows().len() {
        let row = channels.channels.rows()[i].clone();
        if !channels.zone_row_wanted(row.id) || channels.carries_row(row.id) {
            continue;
        }
        let Some(name) = compose(&row, zone_name, area_flags, city_word) else {
            continue;
        };
        if channels.number_of(&name).is_some() {
            continue;
        }
        take(
            channels,
            W::Register {
                name: name.clone(),
                id: row.id,
            },
        );
        if eligible(&row) {
            take(channels, W::Join(name));
        } else {
            take(channels, W::Suspend(name));
        }
    }

    // Pass 3.
    for name in channels.deferred_joins.clone() {
        take(
            channels,
            W::Register {
                name: name.clone(),
                id: 0,
            },
        );
        take(channels, W::Join(name));
    }
    steps
}

/// One step's slot write: the rename `0x49bc50`, the allocator `0x49b980`, the suspend `0x49bcf0`.
fn write_slot(
    step: &WalkStep,
    channels: &mut ChannelState,
    errors: &crate::ui_action::UiErrorKeys,
) {
    match step {
        WalkStep::Leave(_) | WalkStep::Join(_) => {}
        WalkStep::Rename { old, new } => {
            if let Some(n) = channels.rename_slot(old, new) {
                debug!("chat: zone channel slot {n} renamed {old:?} → {new:?}");
            }
        }
        WalkStep::Register { name, id } => {
            if let Some(n) = channels.claim_or_refuse(name, *id, errors) {
                debug!("chat: channel {name:?} registered as slot {n}");
            }
        }
        WalkStep::Suspend(name) => {
            if let Some(n) = channels.suspend_slot(name) {
                debug!("chat: zone channel slot {n} ({name:?}) suspended — not eligible here");
            }
        }
    }
}

/// One walk for a player in `zone_name` ([`run_walk`]), its packets sent in order, then every
/// slot it renamed, registered or suspended pushed into the VM's mirror, so `GetChannelName(n)`
/// and a `SendChatMessage` to a number read the new slots from this frame on. The deferred joins
/// are spent (`0x49a692`-`0x49a69e`).
pub(super) fn walk_zone(
    channels: &mut ChannelState,
    zone_name: &str,
    area_flags: u32,
    city_word: Option<&str>,
    (commands, errors): (&NetCommands, &crate::ui_action::UiErrorKeys),
    script: Option<&mut benilla_ui::script::UiScript>,
) {
    let steps = run_walk(channels, zone_name, area_flags, city_word, errors);
    if !zone_name.is_empty() {
        channels.deferred_joins.clear();
    }
    for step in &steps {
        let cmd = match step {
            WalkStep::Leave(name) => {
                debug!("chat: leaving zone channel {name:?}");
                ClientCommand::LeaveChannel { name: name.clone() }
            }
            WalkStep::Join(name) => {
                debug!("chat: joining zone channel {name:?}");
                ClientCommand::JoinChannel {
                    name: name.clone(),
                    password: String::new(),
                }
            }
            _ => continue,
        };
        let _ = commands.0.send(cmd);
    }
    if let (false, Some(script)) = (steps.is_empty(), script) {
        channels.mirror_into(script);
    }
}

/// Whether the zone under the player is final: an active avatar done settling, benilla's stand-in
/// for the loading screen the reference walks after.
fn zone_is_settled(player: Option<&crate::player::Player>) -> bool {
    player.is_some_and(|p| p.active && !p.settling)
}

/// The walk, re-run whenever the zone changes. It composes from the real zone name, the
/// `GetRealZoneText` cache (`0xb4b404`), never the display text, which the indoor override
/// (`0x67e670`) renames.
///
/// The reference walks in `UpdateZoneText 0x494780` on each zone change (`0x494931`) and once at
/// world entry; this polls the same inputs every frame. It waits while the world arrives
/// (`Player::settling`), as the reference walks after its loading screen and our leaf area moves
/// while terrain streams in, and for the chat cache to seat the mask, as the reference bails on
/// its ready flag (`0x49a219`) until the cache loader calls the walk (`0x499a22`).
pub(super) fn auto_join_zone_channels(
    commands: Res<NetCommands>,
    // The slots beside the VM: the walk runs no Lua, so it is an engine turn.
    mut vm: crate::script_calls::VmWith<ResMut<'static, ChannelState>>,
    areas: Option<Res<AreaTableRes>>,
    world: benilla_world::world_point::WorldPoint,
    // One parameter for clippy's argument limit: the body's settle and a cinematic's view.
    body: (
        Option<Res<crate::player::Player>>,
        Option<Res<crate::cinematic::Cinematic>>,
    ),
    mut walk: ResMut<ZoneChannelWalk>,
    mut entered: MessageReader<crate::net::EnteredWorldMessage>,
    mut catalog_fed: Local<crate::ui_script::VmMemo<Option<(String, u32)>>>,
    errors: Res<crate::ui_action::UiErrorKeys>,
) {
    vm.turn(|script, mut channels| {
        let (player, cinematic) = (&body.0, &body.1);
        // World entry arms the walk; the session-end edges disarm it.
        if entered.read().next().is_some() {
            walk.live = true;
        }

        // A cinematic suppresses the rejoin: two of the six reads of the reference's cinematic cell
        // `[0xb4e310]`, `0x49491c` (the zone-text update) and `0x5ff565` (an `UPDATEFLAGS` reflex),
        // skip `ZoneChannelRefresh` (`0x49a210`) while one runs, and `EndCinematic` calls it at
        // `0x48f1d0`. A race intro flies the streaming focus far off a body that has not moved.
        let cinematic_running = cinematic.as_deref().is_some_and(|c| c.is_playing());
        let areas = areas.as_deref();

        // `None` with no session, during a cinematic, while the world arrives, or with no area
        // answer.
        let zone_id = (walk.live && !cinematic_running && zone_is_settled(player.as_deref()))
            .then(|| {
                let areas = areas?;
                world.area().and_then(|leaf| areas.0.top_zone(leaf))
            })
            .flatten();
        walk.zone_id = zone_id;
        let zone = zone_id
            .and_then(|id| areas?.0.get(id))
            .map(|row| (row.name.clone(), row.flags));

        // The VM's catalog is fed before those gates: to its verbs an empty catalog means "no such
        // built-in channel", which files `General` as a custom channel and joins one of that name.
        // Zone-less, a zone row carries `resolved: None`, the reference's nil leg (`0x4a10d9`,
        // `0x49ece5`).
        let mut script = script;
        if !channels.channels.is_empty() {
            if let Some(script) = script.as_mut() {
                let at = zone.clone().unwrap_or_default();
                let fed = catalog_fed.get(script);
                if fed.as_ref() != Some(&at) {
                    script.set_zone_channel_catalog(zone_channel_catalog(
                        &channels.channels,
                        &at.0,
                        at.1,
                        areas.and_then(|a| city_word(&a.0)),
                    ));
                    *fed = Some(at);
                }
            }
        }

        let (Some(zone_id), Some((zone_name, area_flags)), Some(areas)) = (zone_id, zone, areas)
        else {
            return; // nothing to walk against yet
        };
        if channels.zone_mask.is_none() {
            return; // the mask is not seated: the reference's ready-flag bail
        }
        if walk.at == Some(zone_id) {
            return; // same zone as last frame
        }

        walk_zone(
            &mut channels,
            &zone_name,
            area_flags,
            city_word(&areas.0),
            (&commands, &errors),
            script,
        );
        walk.at = Some(zone_id);
    })
}

/// Feed the VM the zone-less catalog before any interface file runs: world entry runs FrameXML
/// and the addons in one call, ahead of the walk's first tick.
pub(crate) fn seed_zone_channel_catalog(
    world: &mut World,
    script: &mut benilla_ui::script::UiScript,
) {
    let Some(channels) = world.get_resource::<ChannelState>() else {
        return;
    };
    if channels.channels.is_empty() {
        return; // no `ChatChannels.dbc`: nothing to feed, as in the walk
    }
    let city = world
        .get_resource::<AreaTableRes>()
        .and_then(|a| city_word(&a.0));
    script.set_zone_channel_catalog(zone_channel_catalog(&channels.channels, "", 0, city));
}

/// Every `ChatChannels.dbc` row as the VM needs it: the id, the Shortcut the verbs match a typed
/// name against (`0x4a10b2`), the name the join body composes here ([`compose`]; `None` is the
/// verbs' nil leg), and whether `EnumerateServerChannels 0x4a1790` lists it (a city-only row,
/// `flags & 0x10`, only in a city).
pub(crate) fn zone_channel_catalog(
    catalog: &ChatChannelsCatalog,
    zone_name: &str,
    area_flags: u32,
    city_word: Option<&str>,
) -> Vec<benilla_ui::script::ZoneChannelRow> {
    let in_city = area_flags & AREA_FLAG_TRADE_CHANNEL != 0;
    catalog
        .rows()
        .iter()
        .map(|r| benilla_ui::script::ZoneChannelRow {
            id: r.id,
            shortcut: r.shortcut.clone(),
            resolved: compose(r, zone_name, area_flags, city_word),
            listed: !r.is_city_only() || in_city,
            guild_recruitment: r.is_guild_recruitment(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_chat::edit::SlotRecord;
    use benilla_formats::ChatChannelRow;
    use WalkStep as W;

    /// The shipped table, hand-built so the tests run without an install.
    fn catalog() -> ChatChannelsCatalog {
        ChatChannelsCatalog::from_rows(
            [
                (1, 0x00003, "General - %s", "General"),
                (2, 0x0003B, "Trade - %s", "Trade"),
                (22, 0x10003, "LocalDefense - %s", "LocalDefense"),
                (23, 0x10004, "WorldDefense", "WorldDefense"),
                (24, 0x00000, "LookingForGroup", "LookingForGroup"),
                (25, 0x20032, "GuildRecruitment - %s", "GuildRecruitment"),
            ]
            .into_iter()
            .map(|(id, flags, pattern, shortcut)| ChatChannelRow {
                id,
                flags,
                pattern: pattern.into(),
                shortcut: shortcut.into(),
            })
            .collect(),
        )
    }

    /// The city word, row 3459 of the shipped `AreaTable.dbc`.
    const CITY: Option<&str> = Some("City");

    /// A capital's `AreaTable.dbc` flags (Stormwind City's `0x138`): the walk's city gate `0x8`
    /// and the city word's `0x100`.
    const CAPITAL: u32 = 0x138;
    /// An ordinary zone's: neither.
    const OUTSIDE: u32 = 0;

    /// A fresh character's mask: the DBC's three `INITIAL` rows (seeded at `0x4997fc`).
    const SEED: u32 = 0x0020_0003;
    /// `GuildRecruitment`'s bit, `1 << (25 - 1)`, which the cascade's confirmed join sets.
    const GUILD_RECRUITMENT_BIT: u32 = 1 << 24;

    fn state(mask: u32) -> ChannelState {
        ChannelState {
            channels: catalog(),
            zone_mask: Some(mask),
            ..Default::default()
        }
    }

    /// A confirmed slot carrying the row its name composes from, as the walk or a join stores it.
    fn joined(name: &str) -> SlotRecord {
        crate::ui_chat::edit::ChannelSlot::confirmed(name, catalog().zone_channel_id(name)).into()
    }

    /// `joined(name)` in `state`.
    fn in_state(name: &str, state: SlotState) -> SlotRecord {
        crate::ui_chat::edit::ChannelSlot {
            state,
            ..crate::ui_chat::edit::ChannelSlot::confirmed(name, catalog().zone_channel_id(name))
        }
        .into()
    }

    fn suspended(name: &str) -> SlotRecord {
        in_state(name, SlotState::Suspended)
    }

    fn s(v: &str) -> String {
        v.to_string()
    }

    /// WorldDefense, LookingForGroup and GuildRecruitment have no bit in a fresh mask.
    #[test]
    fn an_ordinary_zone_joins_general_and_local_defense() {
        assert_eq!(
            wanted_channels(&catalog(), SEED, "Elwynn Forest", OUTSIDE, CITY),
            vec!["General - Elwynn Forest", "LocalDefense - Elwynn Forest"]
        );
    }

    #[test]
    fn a_capital_adds_the_shared_trade_channel() {
        assert_eq!(
            wanted_channels(&catalog(), SEED, "Stormwind City", CAPITAL, CITY),
            vec![
                "General - Stormwind City",
                "Trade - City",
                "LocalDefense - Stormwind City",
            ]
        );
    }

    /// "General - " would be a real channel on the server.
    #[test]
    fn an_unknown_zone_joins_nothing_zone_dependent() {
        assert!(wanted_channels(&catalog(), SEED, "", OUTSIDE, CITY).is_empty());
        assert!(wanted_channels(&catalog(), SEED, "", CAPITAL, CITY).is_empty());
    }

    #[test]
    fn the_walk_waits_for_the_world_under_the_player() {
        use crate::player::Player;

        assert!(!zone_is_settled(None), "no avatar: no zone to believe");

        let mut p = Player::default();
        assert!(!zone_is_settled(Some(&p)), "inactive avatar");

        p.active = true;
        p.settling = true;
        assert!(
            !zone_is_settled(Some(&p)),
            "settling — the destination's terrain and WMOs are still arriving, so the leaf area \
             under the body is still moving"
        );

        p.settling = false;
        assert!(zone_is_settled(Some(&p)), "settled: the zone is now final");
    }

    #[test]
    fn leaving_the_world_ends_the_channel_session() {
        let mut channels = state(SEED);
        channels.joined = vec![
            joined("General - Tanaris"),
            joined("LocalDefense - Tanaris"),
        ];
        let mut walk = ZoneChannelWalk {
            at: Some(440),
            zone_id: Some(440),
            live: true,
        };
        let mut cascade = GuildRecruitmentCascade::default();
        cascade.observe_guild_id(7);

        // No VM in a unit test: the mirror leg is the one line this cannot reach.
        end_channel_session(None, &mut channels, &mut walk, &mut cascade);

        assert_eq!(walk.at, None, "the next entry re-walks from scratch");
        assert_eq!(walk.zone_id, None);
        assert!(!walk.live);
        assert!(
            channels.joined.is_empty(),
            "the confirmed list is the server's, and the server just destroyed it — a survivor \
             here is what renumbers the next character's channels"
        );
        assert_eq!(
            channels.zone_mask,
            Some(SEED),
            "the mask is the character's file, not the session's"
        );
        assert!(
            cascade.observe_guild_id(7),
            "the cascade's watcher forgot the guild id, so the next login's first sight of it is \
             a change again — the reference's player-create trigger"
        );
    }

    /// A locale whose sentinel row ships blank: the deviation in `compose`.
    #[test]
    fn a_missing_city_word_skips_the_trade_channel_rather_than_half_naming_it() {
        assert_eq!(
            wanted_channels(&catalog(), SEED, "Stormwind City", CAPITAL, Some("")),
            vec!["General - Stormwind City", "LocalDefense - Stormwind City"]
        );
    }

    #[test]
    fn crossing_a_zone_border_renames_the_same_rows() {
        let cat = catalog();
        let felwood = wanted_channels(&cat, SEED, "Felwood", OUTSIDE, CITY);
        let winterspring = wanted_channels(&cat, SEED, "Winterspring", OUTSIDE, CITY);
        assert_ne!(felwood, winterspring, "the names differ");
        let ids = |v: &[String]| -> Vec<u32> { v.iter().map(|n| cat.zone_channel_id(n)).collect() };
        assert_eq!(
            ids(&felwood),
            ids(&winterspring),
            "…but they are the same DBC rows: [1, 22] either side of the border"
        );
        assert_eq!(ids(&felwood), vec![1, 22]);
    }

    #[test]
    fn a_first_walk_registers_every_mask_row_before_the_gate() {
        let mut c = state(SEED);
        let steps = walk(&mut c, "Elwynn Forest", OUTSIDE, CITY);
        assert_eq!(
            steps,
            vec![
                W::Register {
                    name: s("General - Elwynn Forest"),
                    id: 1
                },
                W::Join(s("General - Elwynn Forest")),
                W::Register {
                    name: s("Trade - Elwynn Forest"),
                    id: 2
                },
                W::Suspend(s("Trade - Elwynn Forest")),
                W::Register {
                    name: s("LocalDefense - Elwynn Forest"),
                    id: 22
                },
                W::Join(s("LocalDefense - Elwynn Forest")),
            ]
        );
        assert_eq!(c.number_of("General - Elwynn Forest"), Some(1));
        assert_eq!(c.number_of("Trade - Elwynn Forest"), Some(2));
        assert_eq!(
            c.slot_state("Trade - Elwynn Forest"),
            Some(SlotState::Suspended)
        );
        assert_eq!(c.number_of("LocalDefense - Elwynn Forest"), Some(3));

        let steps = plan_walk(&state(SEED), "Stormwind City", CAPITAL, CITY);
        assert_eq!(
            steps,
            vec![
                W::Register {
                    name: s("General - Stormwind City"),
                    id: 1
                },
                W::Join(s("General - Stormwind City")),
                W::Register {
                    name: s("Trade - City"),
                    id: 2
                },
                W::Join(s("Trade - City")),
                W::Register {
                    name: s("LocalDefense - Stormwind City"),
                    id: 22
                },
                W::Join(s("LocalDefense - Stormwind City")),
            ]
        );
    }

    /// The walk on `c`, its slots written as it goes, without the wire.
    fn walk(c: &mut ChannelState, zone: &str, flags: u32, city: Option<&str>) -> Vec<WalkStep> {
        run_walk(
            c,
            zone,
            flags,
            city,
            &crate::ui_action::UiErrorKeys::default(),
        )
    }

    /// What the walk would do on `c`, run on a copy.
    fn plan_walk(c: &ChannelState, zone: &str, flags: u32, city: Option<&str>) -> Vec<WalkStep> {
        walk(&mut c.clone(), zone, flags, city)
    }

    /// The suspended Trade slot between the two is walked too, and stays as it is.
    #[test]
    fn a_border_crossing_is_leave_rename_join_per_slot_in_slot_order() {
        let mut c = state(SEED);
        c.joined = vec![
            joined("General - Felwood"),
            suspended("Trade - City"),
            joined("LocalDefense - Felwood"),
        ];
        let steps = walk(&mut c, "Winterspring", OUTSIDE, CITY);
        assert_eq!(
            steps,
            vec![
                W::Leave(s("General - Felwood")),
                W::Rename {
                    old: s("General - Felwood"),
                    new: s("General - Winterspring")
                },
                W::Join(s("General - Winterspring")),
                W::Leave(s("LocalDefense - Felwood")),
                W::Rename {
                    old: s("LocalDefense - Felwood"),
                    new: s("LocalDefense - Winterspring")
                },
                W::Join(s("LocalDefense - Winterspring")),
            ]
        );
        assert_eq!(
            c.number_of("General - Winterspring"),
            Some(1),
            "renamed in place"
        );
        assert_eq!(
            c.slot_state("General - Winterspring"),
            Some(SlotState::Renamed)
        );
        assert_eq!(c.number_of("Trade - City"), Some(2), "untouched");
        assert_eq!(c.number_of("LocalDefense - Winterspring"), Some(3));
    }

    /// Trade suspends on the way out (`0x49bcf0`) under its old name, and back in a suspended slot
    /// skips the name test (`0x49a31c`) and is renamed to itself and re-joined (`0x49a3dc`).
    #[test]
    fn a_city_round_trip_keeps_the_slot_and_its_number() {
        let mut c = state(SEED);
        c.joined = vec![
            joined("General - Stormwind City"),
            joined("Trade - City"),
            joined("LocalDefense - Stormwind City"),
        ];

        // Out of the city.
        let steps = walk(&mut c, "Elwynn Forest", OUTSIDE, CITY);
        assert_eq!(
            steps,
            vec![
                W::Leave(s("General - Stormwind City")),
                W::Rename {
                    old: s("General - Stormwind City"),
                    new: s("General - Elwynn Forest")
                },
                W::Join(s("General - Elwynn Forest")),
                W::Leave(s("Trade - City")),
                W::Suspend(s("Trade - City")),
                W::Leave(s("LocalDefense - Stormwind City")),
                W::Rename {
                    old: s("LocalDefense - Stormwind City"),
                    new: s("LocalDefense - Elwynn Forest")
                },
                W::Join(s("LocalDefense - Elwynn Forest")),
            ]
        );
        assert_eq!(c.slot_state("Trade - City"), Some(SlotState::Suspended));
        assert_eq!(c.number_of("Trade - City"), Some(2));
        assert_eq!(
            c.number_of("LocalDefense - Elwynn Forest"),
            Some(3),
            "and nothing above it renumbered — the slot went quiet, it did not go away"
        );
        // The notices confirm the two renamed rows.
        c.confirm_slot("General - Elwynn Forest");
        c.confirm_slot("LocalDefense - Elwynn Forest");

        // Back in: the bypass joins and clears the suspension, with no LEAVE.
        let steps = walk(&mut c, "Stormwind City", CAPITAL, CITY);
        assert_eq!(
            steps,
            vec![
                W::Leave(s("General - Elwynn Forest")),
                W::Rename {
                    old: s("General - Elwynn Forest"),
                    new: s("General - Stormwind City")
                },
                W::Join(s("General - Stormwind City")),
                W::Rename {
                    old: s("Trade - City"),
                    new: s("Trade - City")
                },
                W::Join(s("Trade - City")),
                W::Leave(s("LocalDefense - Elwynn Forest")),
                W::Rename {
                    old: s("LocalDefense - Elwynn Forest"),
                    new: s("LocalDefense - Stormwind City")
                },
                W::Join(s("LocalDefense - Stormwind City")),
            ]
        );
        assert_eq!(
            c.slot_state("Trade - City"),
            Some(SlotState::Pending),
            "renamed to itself, 3 → 1, until its notice"
        );
        assert_eq!(c.number_of("Trade - City"), Some(2), "still slot 2");
    }

    /// The explicit leave clears the bit (`0x49f10a`) and `YOU_LEFT` frees the slot (`0x49bbd0`),
    /// so neither pass walks General again (`0x49a494`), this session or the next.
    #[test]
    fn a_left_channel_stays_left_this_session_and_the_next() {
        let mut c = state(SEED);
        c.joined = vec![
            joined("General - Elwynn Forest"),
            suspended("Trade - City"),
            joined("LocalDefense - Elwynn Forest"),
        ];

        // `/leave General`: the stock `SlashCmdList["LEAVE"]` hands `LeaveChannelByName` the bare
        // token and the VM composes it for the zone; `/leave 1` names the slot here.
        let target = c.channel_key("1").expect("slot 1 is confirmed");
        assert_eq!(target, "General - Elwynn Forest", "the slot's wire name");
        c.note_zone_channel_left(&target);
        assert_eq!(c.zone_mask, Some(SEED & !1), "bit 0 cleared");
        // The server's `YOU_LEFT` frees the slot in place.
        assert_eq!(c.free_slot(&target), Some(1));

        // The next border: General is neither walked (no slot) nor registered (no bit).
        let steps = plan_walk(&c, "Westfall", OUTSIDE, CITY);
        assert_eq!(
            steps,
            vec![
                W::Leave(s("LocalDefense - Elwynn Forest")),
                W::Rename {
                    old: s("LocalDefense - Elwynn Forest"),
                    new: s("LocalDefense - Westfall")
                },
                W::Join(s("LocalDefense - Westfall")),
            ]
        );

        // The next login: a fresh slot array under the file's mask.
        let mut relog = state(SEED & !1);
        let steps = walk(&mut relog, "Elwynn Forest", OUTSIDE, CITY);
        assert_eq!(
            steps,
            vec![
                W::Register {
                    name: s("Trade - Elwynn Forest"),
                    id: 2
                },
                W::Suspend(s("Trade - Elwynn Forest")),
                W::Register {
                    name: s("LocalDefense - Elwynn Forest"),
                    id: 22
                },
                W::Join(s("LocalDefense - Elwynn Forest")),
            ],
            "General is gone for good; Trade is now number 1"
        );

        // Until a `/join General` confirms and sets the bit back.
        relog.claim_slot("General - Elwynn Forest", 1);
        relog.confirm_slot("General - Elwynn Forest");
        assert_eq!(relog.zone_mask, Some(SEED));
        let steps = plan_walk(&relog, "Westfall", OUTSIDE, CITY);
        assert!(
            steps.contains(&W::Join(s("General - Westfall"))),
            "walked again from its slot: {steps:?}"
        );
    }

    /// Zero is a real state, every zone channel left; unseated means the file is not read yet.
    #[test]
    fn a_zero_or_unseated_mask_registers_nothing() {
        assert!(plan_walk(&state(0), "Elwynn Forest", OUTSIDE, CITY).is_empty());
        let unseated = ChannelState {
            channels: catalog(),
            zone_mask: None,
            ..Default::default()
        };
        assert!(plan_walk(&unseated, "Elwynn Forest", OUTSIDE, CITY).is_empty());
        assert!(!unseated.zone_row_wanted(1));
    }

    /// Once the cascade's confirmed join sets bit 24, `GuildRecruitment` takes the same city gate
    /// (`0x10`) and city name (`0x20`, in a capital) as Trade, last in DBC order.
    #[test]
    fn the_guild_recruitment_bit_rides_the_walk_like_trade() {
        let steps = plan_walk(
            &state(SEED | GUILD_RECRUITMENT_BIT),
            "Elwynn Forest",
            OUTSIDE,
            CITY,
        );
        assert_eq!(
            &steps[6..],
            &[
                W::Register {
                    name: s("GuildRecruitment - Elwynn Forest"),
                    id: 25
                },
                W::Suspend(s("GuildRecruitment - Elwynn Forest")),
            ],
            "registered and suspended out here under the zone's name, number 4"
        );
        let steps = plan_walk(
            &state(SEED | GUILD_RECRUITMENT_BIT),
            "Ironforge",
            CAPITAL,
            CITY,
        );
        assert_eq!(
            &steps[6..],
            &[
                W::Register {
                    name: s("GuildRecruitment - City"),
                    id: 25
                },
                W::Join(s("GuildRecruitment - City")),
            ]
        );
    }

    /// A `/join General` after a `/leave`, or the cascade's own join: pass 1 is the slot array.
    #[test]
    fn a_channel_joined_outside_the_walk_is_walked_from_its_slot() {
        let mut c = state(SEED | GUILD_RECRUITMENT_BIT);
        c.joined = vec![
            joined("General - Stormwind City"),
            joined("Trade - City"),
            joined("LocalDefense - Stormwind City"),
        ];
        // The cascade's join: a slot, then the notice confirms it and sets the bit.
        c.claim_slot("GuildRecruitment - City", 25);
        c.confirm_slot("GuildRecruitment - City");
        let steps = plan_walk(&c, "Stormwind City", CAPITAL, CITY);
        assert!(
            steps.is_empty(),
            "same zone, every name unchanged, every row eligible — nothing to do: {steps:?}"
        );
        // Walking out suspends it exactly like Trade.
        let steps = plan_walk(&c, "Elwynn Forest", OUTSIDE, CITY);
        assert!(steps.contains(&W::Leave(s("GuildRecruitment - City"))));
        assert!(steps.contains(&W::Suspend(s("GuildRecruitment - City"))));
        assert!(!steps.iter().any(|st| matches!(st, W::Register { .. })));
    }

    /// State 3 sends no LEAVE, and the rename's `(old == 3) ? 1 : 2` lands it on 1, a plain
    /// `YOU_JOINED`.
    #[test]
    fn a_suspended_slot_whose_name_moved_rejoins_without_a_leave() {
        let mut c = state(SEED);
        c.joined = vec![suspended("General - Elwynn Forest")];
        let steps = walk(&mut c, "Westfall", OUTSIDE, CITY);
        assert_eq!(
            &steps[..2],
            &[
                W::Rename {
                    old: s("General - Elwynn Forest"),
                    new: s("General - Westfall")
                },
                W::Join(s("General - Westfall")),
            ]
        );
        assert_eq!(
            c.slot_state("General - Westfall"),
            Some(SlotState::Pending),
            "3 → 1, not 2"
        );
    }

    #[test]
    fn channel_key_resolves_a_number_to_a_confirmed_slot_or_to_nothing() {
        let mut c = state(SEED);
        c.joined = vec![
            joined("General - Elwynn Forest"),
            suspended("Trade - City"),
            SlotRecord::Freed { id: 0 },
            joined("MyChan"),
        ];
        assert_eq!(
            c.channel_key("1").as_deref(),
            Some("General - Elwynn Forest")
        );
        assert_eq!(
            c.channel_key("1abc").as_deref(),
            Some("General - Elwynn Forest"),
            "SStrToInt takes the leading digits"
        );
        assert_eq!(
            c.channel_key("2"),
            None,
            "suspended: state 3 fails `0x49be50`, and the whole call is a no-op"
        );
        assert_eq!(c.channel_key("3"), None, "a hole");
        assert_eq!(c.channel_key("9"), None, "out of range");
        assert_eq!(c.channel_key("-1"), None);
        assert_eq!(c.channel_key("0").as_deref(), Some("0"), "0 is a name");
        assert_eq!(
            c.channel_key("").as_deref(),
            Some(""),
            "`/leave` alone sends an empty name"
        );
        assert_eq!(
            c.channel_key("General - Elwynn Forest").as_deref(),
            Some("General - Elwynn Forest"),
            "a composed name passes through"
        );
        assert_eq!(c.channel_key("mychan").as_deref(), Some("mychan"));
        assert_eq!(c.channel_key("+1").as_deref(), Some("+1"), "no `+`");
        assert_eq!(c.channel_key(" 1").as_deref(), Some(" 1"), "no space skip");
        assert_eq!(
            c.channel_key("4294967297").as_deref(),
            Some("General - Elwynn Forest"),
            "`SStrToInt` wraps: 2^32 + 1 reads 1"
        );

        // The mask clear needs a slot carrying the wire name (`0x49f0f4`).
        c.note_zone_channel_left("General - Nowhere");
        assert_eq!(c.zone_mask, Some(SEED), "no slot carries it: no clear");
        c.note_zone_channel_left("General - Elwynn Forest");
        assert_eq!(c.zone_mask, Some(SEED & !1));
    }

    #[test]
    fn the_slot_set_and_the_join_set_differ_only_by_the_city_gate() {
        let cat = catalog();
        assert_eq!(
            tracked_channels(&cat, SEED, "Stormwind City", CAPITAL, CITY),
            wanted_channels(&cat, SEED, "Stormwind City", CAPITAL, CITY),
            "in a capital the gate passes every row, so the two lists agree"
        );
        assert_eq!(
            tracked_channels(&cat, SEED, "Elwynn Forest", OUTSIDE, CITY),
            [
                "General - Elwynn Forest",
                "Trade - Elwynn Forest",
                "LocalDefense - Elwynn Forest"
            ],
            "outside one Trade takes a slot under the zone's name and is not joined"
        );
        assert!(
            tracked_channels(&cat, SEED, "", OUTSIDE, CITY).is_empty(),
            "no zone: nothing zone-dependent composes, so nothing takes a slot either"
        );
    }

    /// Zone-less is not empty: every row stays, and a zone-dependent one carries `resolved: None`,
    /// the reference's nil leg (`0x4a10d9`, `0x49ece5`), not the custom-channel leg.
    #[test]
    fn a_zoneless_catalog_still_carries_every_row() {
        let rows = zone_channel_catalog(&catalog(), "", OUTSIDE, None);
        assert_eq!(rows.len(), 6, "every DBC row is present, zone or no zone");

        let by = |s: &str| rows.iter().find(|r| r.shortcut == s).unwrap().clone();
        assert_eq!(by("General").id, 1, "…and each carries its ChannelID");
        assert_eq!(
            by("General").resolved,
            None,
            "a zone-dependent row with no zone is UNRESOLVED — the nil leg, not the custom leg"
        );
        assert_eq!(by("Trade").resolved, None);
        assert_eq!(
            by("WorldDefense").resolved,
            None,
            "a row whose name carries no %s is the nil leg too (`0x49ece3`-`0x49ecea`)"
        );

        // Once a zone is known, the same rows resolve.
        let rows = zone_channel_catalog(&catalog(), "Elwynn Forest", OUTSIDE, CITY);
        let by = |s: &str| rows.iter().find(|r| r.shortcut == s).unwrap().clone();
        assert_eq!(
            by("General").resolved,
            Some("General - Elwynn Forest".to_string())
        );
        assert!(
            !by("Trade").listed,
            "…and Trade is city-only, so it is not listed out here"
        );
    }

    /// arg7: a composed name maps back to its ChannelID by vmangos's substring match
    /// (`DBCStores.cpp:531`); a custom one to 0.
    #[test]
    fn composed_names_carry_their_channel_id_back() {
        let cat = catalog();
        assert_eq!(cat.zone_channel_id("General - Elwynn Forest"), 1);
        assert_eq!(cat.zone_channel_id("Trade - City"), 2);
        assert_eq!(cat.zone_channel_id("LocalDefense - Durotar"), 22);
        assert_eq!(cat.zone_channel_id("World"), 0);
    }

    fn reg(name: &str, id: u32) -> WalkStep {
        W::Register { name: s(name), id }
    }

    fn pending(name: &str) -> SlotRecord {
        in_state(name, SlotState::Pending)
    }

    /// Outside a capital a city-named row takes the zone's name: the `%s` is the city word only for
    /// a row with `0x20` in an area with `0x100` (`0x49a4af`-`0x49a4d8`), so pass 2 registers
    /// `Trade - Elwynn Forest` (`0x49a50d`) before the city gate suspends it (`0x49a527`).
    #[test]
    fn outside_a_capital_trade_takes_the_zones_name() {
        assert_eq!(
            plan_walk(&state(SEED), "Elwynn Forest", OUTSIDE, CITY),
            vec![
                reg("General - Elwynn Forest", 1),
                W::Join(s("General - Elwynn Forest")),
                reg("Trade - Elwynn Forest", 2),
                W::Suspend(s("Trade - Elwynn Forest")),
                reg("LocalDefense - Elwynn Forest", 22),
                W::Join(s("LocalDefense - Elwynn Forest")),
            ]
        );
        // The trade gate (`0x8`) without the city word's bit (`0x100`) joins Trade by the zone.
        assert_eq!(
            plan_walk(&state(0b10), "Somewhere", 0x8, CITY),
            vec![reg("Trade - Somewhere", 2), W::Join(s("Trade - Somewhere"))]
        );
    }

    /// Back in a capital, a suspended slot skips the name test (`0x49a31c`) and is renamed in place
    /// (`0x49a3dc`), which writes state 1, not 2 (`0x49bcd3`), so it is pending until its notice.
    #[test]
    fn a_suspended_trade_slot_is_renamed_into_a_capital_and_waits_for_its_notice() {
        let mut c = state(0b10);
        c.joined = vec![suspended("Trade - Elwynn Forest")];
        let steps = walk(&mut c, "Stormwind City", CAPITAL, CITY);
        assert_eq!(
            steps,
            vec![
                W::Rename {
                    old: s("Trade - Elwynn Forest"),
                    new: s("Trade - City")
                },
                W::Join(s("Trade - City")),
            ]
        );
        assert_eq!(c.slot_state("Trade - City"), Some(SlotState::Pending));

        // Suspended under the capital's own name, the same: renamed to itself.
        let mut c = state(0b10);
        c.joined = vec![suspended("Trade - City")];
        let steps = walk(&mut c, "Ironforge", CAPITAL, CITY);
        assert_eq!(steps.last(), Some(&W::Join(s("Trade - City"))));
        assert_eq!(c.slot_state("Trade - City"), Some(SlotState::Pending));
    }

    /// The walk's LEAVE goes out only from a slot in state 0 (`0x49a35e`): a pending one is renamed
    /// and re-joined without it.
    #[test]
    fn the_walk_leaves_only_from_a_confirmed_slot() {
        let mut c = state(SEED);
        c.joined = vec![pending("General - Elwynn Forest")];
        assert_eq!(
            plan_walk(&c, "Westfall", OUTSIDE, CITY)[..2],
            [
                W::Rename {
                    old: s("General - Elwynn Forest"),
                    new: s("General - Westfall")
                },
                W::Join(s("General - Westfall")),
            ]
        );
    }

    /// A freed slot keeps its ChannelID (`0x49bbd0` zeroes the number and the name, not `+0x94`),
    /// and pass 2 finds a row by id in any record (`0x49bef0` tests no number, `0x49bf10`): a zone
    /// channel the server dropped stays out at the next zone change, until a join reuses its
    /// record and its id (`0x49b9b0`, `0x49ba01`). Pass 1 skips the record (`0x49a295`).
    #[test]
    fn a_dropped_zone_channel_stays_out_until_its_record_is_reused() {
        let mut c = state(SEED);
        for name in ["General - Elwynn Forest", "LocalDefense - Elwynn Forest"] {
            c.claim_slot(name, c.channels.zone_channel_id(name));
            c.confirm_slot(name);
        }
        c.free_slot("General - Elwynn Forest");
        let steps = plan_walk(&c, "Westfall", OUTSIDE, CITY);
        assert!(
            !steps.iter().any(|s| match s {
                W::Join(name) | W::Leave(name) => name.starts_with("General"),
                W::Register { name, .. } => name.starts_with("General"),
                _ => false,
            }),
            "{steps:?}"
        );
        assert!(
            steps.contains(&W::Join(s("LocalDefense - Westfall"))),
            "{steps:?}"
        );

        assert_eq!(
            c.claim_slot("MyChan", 0),
            Some(1),
            "the custom join takes slot 1"
        );
        let steps = plan_walk(&c, "Westfall", OUTSIDE, CITY);
        assert!(
            steps.contains(&W::Join(s("General - Westfall"))),
            "{steps:?}"
        );
    }

    /// Pass 2 looks each row up in the table as the rows before it left it: General's register
    /// reuses the freed record (`0x49b9b0`) and overwrites its id 22 (`0x49ba01`), so
    /// LocalDefense's `0x49bef0` (`0x49a4a2`) then finds no record and it joins too.
    #[test]
    fn pass_two_reads_the_slots_each_earlier_row_wrote() {
        let bit = crate::ui_chat::edit::zone_bit;
        let mut c = state(bit(1) | bit(2) | bit(22));
        c.joined = vec![
            joined("MyChan"),
            suspended("Trade - City"),
            SlotRecord::Freed { id: 22 },
        ];
        let (tx, rx) = crossbeam_channel::unbounded();
        let errors = crate::ui_action::UiErrorKeys::default();
        walk_zone(
            &mut c,
            "Elwynn Forest",
            OUTSIDE,
            CITY,
            (&NetCommands(tx), &errors),
            None,
        );
        let joins: Vec<String> = rx
            .try_iter()
            .filter_map(|c| match c {
                ClientCommand::JoinChannel { name, .. } => Some(name),
                _ => None,
            })
            .collect();
        assert_eq!(
            joins,
            ["General - Elwynn Forest", "LocalDefense - Elwynn Forest"]
        );
        assert_eq!(c.number_of("General - Elwynn Forest"), Some(3));
        assert_eq!(c.number_of("LocalDefense - Elwynn Forest"), Some(4));
    }

    /// The chat cache's `CHANNELS` names are the walk's third pass: registered with id 0
    /// (`0x49a5da`) and joined (`0x49a5e2`-`0x49a62a`) after the zone channels, then the list is
    /// freed (`0x49a692`-`0x49a69e`). With no zone text the walk skips all three passes
    /// (`0x49a235`, `0x49a23d`).
    #[test]
    fn the_login_re_join_is_the_walks_third_pass() {
        let mut c = state(SEED);
        c.deferred_joins = vec![s("MyChan"), s("Other")];
        let steps = plan_walk(&c, "Elwynn Forest", OUTSIDE, CITY);
        assert_eq!(
            steps[6..],
            [
                reg("MyChan", 0),
                W::Join(s("MyChan")),
                reg("Other", 0),
                W::Join(s("Other")),
            ]
        );
        let (tx, rx) = crossbeam_channel::unbounded();
        let errors = crate::ui_action::UiErrorKeys::default();
        walk_zone(
            &mut c,
            "Elwynn Forest",
            OUTSIDE,
            CITY,
            (&NetCommands(tx), &errors),
            None,
        );
        assert_eq!(c.number_of("General - Elwynn Forest"), Some(1));
        assert_eq!(c.number_of("MyChan"), Some(4));
        assert_eq!(c.number_of("Other"), Some(5));
        assert!(c.deferred_joins.is_empty(), "the list is spent");
        assert_eq!(
            rx.try_iter().count(),
            4,
            "General, LocalDefense, MyChan, Other"
        );

        let mut c = state(SEED);
        c.deferred_joins = vec![s("MyChan")];
        assert!(plan_walk(&c, "", OUTSIDE, CITY).is_empty());
    }

    /// Pass 2's allocator (`0x49a50d`) shows `ERR_TOO_MANY_CHAT_CHANNELS` with ten slots taken
    /// (`0x49b9c5`), and the join still goes out (`0x49a536`-`0x49a577`); a suspended row sends
    /// nothing either way.
    #[test]
    fn the_walk_with_ten_slots_taken_shows_too_many_channels() {
        let mut c = state(SEED);
        c.joined = (1..=10)
            .map(|i| crate::ui_chat::edit::ChannelSlot::confirmed(&format!("Chan{i}"), 0).into())
            .collect();
        let (tx, rx) = crossbeam_channel::unbounded();
        let errors = crate::ui_action::UiErrorKeys::default();
        walk_zone(
            &mut c,
            "Elwynn Forest",
            OUTSIDE,
            CITY,
            (&NetCommands(tx), &errors),
            None,
        );
        let keys: Vec<_> = errors.take().into_iter().map(|e| e.key).collect();
        assert_eq!(keys, ["ERR_TOO_MANY_CHAT_CHANNELS"; 3]);
        let joins: Vec<String> = rx
            .try_iter()
            .map(|c| match c {
                ClientCommand::JoinChannel { name, .. } => name,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            joins,
            ["General - Elwynn Forest", "LocalDefense - Elwynn Forest"]
        );
    }

    /// The join and leave bodies resolve a Shortcut only against a non-empty zone text, whatever the
    /// row's pattern (`0x49ece3`-`0x49ecea`, `0x49efca`-`0x49efd5`); the join body's city word needs
    /// the area's `0x100` (`0x49ecc3`-`0x49ecda`).
    #[test]
    fn the_verbs_catalog_composes_as_the_join_body_does() {
        let by = |rows: &[benilla_ui::script::ZoneChannelRow], shortcut: &str| {
            rows.iter()
                .find(|r| r.shortcut == shortcut)
                .unwrap()
                .resolved
                .clone()
        };
        let rows = zone_channel_catalog(&catalog(), "", OUTSIDE, CITY);
        assert_eq!(by(&rows, "WorldDefense"), None, "no zone text: no channel");
        let rows = zone_channel_catalog(&catalog(), "Elwynn Forest", OUTSIDE, CITY);
        assert_eq!(by(&rows, "Trade").as_deref(), Some("Trade - Elwynn Forest"));
        assert_eq!(
            by(&rows, "GuildRecruitment").as_deref(),
            Some("GuildRecruitment - Elwynn Forest")
        );
        let rows = zone_channel_catalog(&catalog(), "Orgrimmar", CAPITAL, CITY);
        assert_eq!(by(&rows, "Trade").as_deref(), Some("Trade - City"));
        assert_eq!(by(&rows, "General").as_deref(), Some("General - Orgrimmar"));
        // No city row at all (`0xb4e4f0` null): the zone's name (`0x49ecd8`).
        let rows = zone_channel_catalog(&catalog(), "Orgrimmar", CAPITAL, None);
        assert_eq!(by(&rows, "Trade").as_deref(), Some("Trade - Orgrimmar"));
    }
}
