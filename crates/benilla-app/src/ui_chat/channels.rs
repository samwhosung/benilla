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

/// `AreaTable.dbc` flag `0x200`: the one row (id 3459, "City"), cached at load (`0xb4e4f0`,
/// written at `0x4985fd`), whose localized name the city-named channels take.
const AREA_FLAG_CITY_NAME_ROW: u32 = 0x200;

/// The city word, as the client's load-time scan finds it; `None` skips the city-named rows.
pub(crate) fn city_word(areas: &benilla_formats::AreaTableCatalog) -> Option<&str> {
    areas
        .rows()
        .find(|r| r.flags & AREA_FLAG_CITY_NAME_ROW != 0)
        .map(|r| r.name.as_str())
        .filter(|n| !n.is_empty())
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
    if let Some(script) = script {
        script.set_joined_channels(Vec::new());
    }
}

/// Seed a fresh VM's joined-channel mirror, which is otherwise pushed only on a join or a leave, so
/// a `/reload` would leave `GetChannelName` and `/N` empty until the next one.
pub(super) fn seed_channels(
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    channels: Res<ChannelState>,
    mut seeded: Local<crate::ui_script::VmMemo<bool>>,
) {
    let Some(mut script) = script else {
        return;
    };
    if seeded.claim(&script) {
        script.set_joined_channels(channels.names());
    }
}

/// The session-end edge: a confirmed `/logout` back to the glue screens (`OnExit(InWorld)`).
pub(super) fn end_session_channels(
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    mut channels: ResMut<ChannelState>,
    mut walk: ResMut<ZoneChannelWalk>,
    mut cascade: ResMut<GuildRecruitmentCascade>,
) {
    end_channel_session(
        script.map(NonSendMut::into_inner),
        &mut channels,
        &mut walk,
        &mut cascade,
    );
}

/// The other session end, a dead socket: a recoverable drop never leaves `InWorld`, but the
/// reconnect gets a fresh server-side `Player` in no channels, so it clears the same state.
pub(super) fn end_session_channels_on_disconnect(
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    mut channels: ResMut<ChannelState>,
    mut walk: ResMut<ZoneChannelWalk>,
    mut cascade: ResMut<GuildRecruitmentCascade>,
    mut disconnects: MessageReader<crate::net::DisconnectedMessage>,
) {
    if disconnects.read().next().is_none() {
        return;
    }
    end_channel_session(
        script.map(NonSendMut::into_inner),
        &mut channels,
        &mut walk,
        &mut cascade,
    );
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

/// The name `row` composes to in `zone_name`; `None` when there is nothing to substitute. A
/// city-named row takes the city word in every zone here; the reference takes it only in an area
/// with flag `0x100` and the zone name elsewhere (`0x49a2dc`).
///
/// Deviation: with a blank city word the reference composes `"Trade - "` (its capital arm checks
/// the row pointer, not the string, `0x49a2d0`-`0x49a2f6`); we decline, because a wrong channel
/// name is joined silently and for good.
fn compose(
    row: &benilla_formats::ChatChannelRow,
    zone_name: &str,
    city_word: Option<&str>,
) -> Option<String> {
    if row.is_zone_dependent() && zone_name.is_empty() {
        return None;
    }
    if row.takes_city_name() && city_word.is_none() {
        return None;
    }
    Some(row.joinable_name(zone_name, city_word.unwrap_or_default()))
}

/// The names the walk registers on a character with no slots, in table order: the reference
/// registers a slot (`0x49a50d`) before its city gate (`0x49a512`), so a city-only row takes its
/// number even outside a capital. The `N.` of a channel line is that slot, never the ChannelID.
#[cfg(test)]
pub(crate) fn tracked_channels(
    catalog: &ChatChannelsCatalog,
    zone_mask: u32,
    zone_name: &str,
    city_word: Option<&str>,
) -> Vec<String> {
    catalog
        .rows()
        .iter()
        .filter(|r| zone_mask & super::edit::zone_bit(r.id) != 0)
        .filter_map(|r| compose(r, zone_name, city_word))
        .collect()
}

/// [`tracked_channels`] with the city gate applied: the names the walk joins. `in_city` gates the
/// city-only rows (`0x10`); `city_word` fills a city-named row's (`0x20`) `%s`.
#[cfg(test)]
pub(crate) fn wanted_channels(
    catalog: &ChatChannelsCatalog,
    zone_mask: u32,
    zone_name: &str,
    in_city: bool,
    city_word: Option<&str>,
) -> Vec<String> {
    catalog
        .rows()
        .iter()
        .filter(|r| zone_mask & super::edit::zone_bit(r.id) != 0)
        .filter(|r| in_city || !r.is_city_only())
        .filter_map(|r| compose(r, zone_name, city_word))
        .collect()
}

/// One step of a walk, in wire order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WalkStep {
    /// `CMSG_LEAVE_CHANNEL` of the old name (`0x49a367`), from a confirmed slot only (`0x49a35e`).
    Leave(String),
    /// `CMSG_JOIN_CHANNEL` (`0x49a3e1` in pass 1, `0x49a56e` in pass 2).
    Join(String),
    /// `0x49bc50`: renamed in place at send time ([`ChannelState::rename_slot`]).
    Rename { old: String, new: String },
    /// `0x49b980`: a slot claimed for an untracked row, before the city gate.
    Register(String),
    /// `0x49bcf0`: state 3, the row stopped applying; the record and its number survive.
    Suspend(String),
    /// The state-3 bypass with the name unchanged: `0x49bc50` writes `(old == 3) ? 1 : 2`, so the
    /// slot confirms with a plain `YOU_JOINED`.
    Confirm(String),
}

/// The walk as a plan: `ZoneChannelRefresh 0x49a210`'s two passes, for a player in `zone_name`.
/// Pass 1 (`0x49a284`) composes each held slot, in slot order, for this zone whatever the mask
/// says. A moved name leaves if confirmed (`0x49a35e`), then the city gate (`0x49a3b8`) suspends
/// it or renames and re-joins it; a suspended slot the gate admits re-joins (the state-3 bypass,
/// `0x49a31c`). A row that stopped applying under an unchanged name, or no longer composes, leaves
/// and suspends here, where the reference skips any other unchanged name (`0x49a337`): its
/// city-named rows move to the zone name outside a capital ([`compose`]). Pass 2 (`0x49a468`)
/// registers the unslotted rows whose mask bit is set (`0x49a494`), in DBC order, before the gate.
pub(crate) fn plan_walk(
    channels: &ChannelState,
    zone_name: &str,
    in_city: bool,
    city_word: Option<&str>,
) -> Vec<WalkStep> {
    use WalkStep as W;
    let catalog = &channels.channels;
    let eligible = |row: &benilla_formats::ChatChannelRow| in_city || !row.is_city_only();
    let mut steps = Vec::new();
    let mut tracked_rows: Vec<u32> = Vec::new();

    // Pass 1.
    for slot in channels.joined.iter().flatten() {
        let Some(row) = catalog.row_for_name(&slot.name) else {
            continue; // a custom channel: not the walk's
        };
        tracked_rows.push(row.id);
        let old = slot.name.as_str();
        match compose(row, zone_name, city_word) {
            Some(new) if new.eq_ignore_ascii_case(old) => {
                if slot.state == SlotState::Suspended {
                    if eligible(row) {
                        steps.push(W::Confirm(old.to_string()));
                        steps.push(W::Join(old.to_string()));
                    }
                } else if !eligible(row) {
                    if slot.state == SlotState::Joined {
                        steps.push(W::Leave(old.to_string()));
                    }
                    steps.push(W::Suspend(old.to_string()));
                }
            }
            Some(new) => {
                if slot.state == SlotState::Joined {
                    steps.push(W::Leave(old.to_string()));
                }
                if eligible(row) {
                    steps.push(W::Rename {
                        old: old.to_string(),
                        new: new.clone(),
                    });
                    steps.push(W::Join(new));
                } else {
                    steps.push(W::Suspend(old.to_string()));
                }
            }
            None => {
                if slot.state == SlotState::Joined {
                    steps.push(W::Leave(old.to_string()));
                }
                if slot.state != SlotState::Suspended {
                    steps.push(W::Suspend(old.to_string()));
                }
            }
        }
    }

    // Pass 2.
    for row in catalog.rows() {
        if !channels.zone_row_wanted(row.id) || tracked_rows.contains(&row.id) {
            continue;
        }
        let Some(name) = compose(row, zone_name, city_word) else {
            continue;
        };
        steps.push(W::Register(name.clone()));
        if eligible(row) {
            steps.push(W::Join(name));
        } else {
            steps.push(W::Suspend(name));
        }
    }
    steps
}

/// Apply a plan in order; answers whether a slot name moved, the VM mirror's cue.
fn apply_walk(steps: &[WalkStep], channels: &mut ChannelState, commands: &NetCommands) -> bool {
    let mut names_moved = false;
    for step in steps {
        match step {
            WalkStep::Leave(name) => {
                debug!("chat: leaving zone channel {name:?}");
                let _ = commands
                    .0
                    .send(ClientCommand::LeaveChannel { name: name.clone() });
            }
            WalkStep::Join(name) => {
                debug!("chat: joining zone channel {name:?}");
                let _ = commands.0.send(ClientCommand::JoinChannel {
                    name: name.clone(),
                    password: String::new(),
                });
            }
            WalkStep::Rename { old, new } => {
                if let Some(n) = channels.rename_slot(old, new) {
                    debug!("chat: zone channel slot {n} renamed {old:?} → {new:?}");
                    names_moved = true;
                }
            }
            WalkStep::Register(name) => match channels.claim_slot(name) {
                Some(n) => {
                    debug!("chat: zone channel {name:?} registered as slot {n}");
                    names_moved = true;
                }
                None => warn!(
                    "chat: no free slot for zone channel {name:?} — all {} are taken",
                    super::edit::MAX_CHANNELS
                ),
            },
            WalkStep::Suspend(name) => {
                if let Some(n) = channels.suspend_slot(name) {
                    debug!("chat: zone channel slot {n} ({name:?}) suspended — not eligible here");
                }
            }
            WalkStep::Confirm(name) => channels.confirm_slot(name),
        }
    }
    names_moved
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
    mut channels: ResMut<ChannelState>,
    areas: Option<Res<AreaTableRes>>,
    world: benilla_world::world_point::WorldPoint,
    // One parameter for clippy's argument limit: the body's settle and a cinematic's view.
    body: (
        Option<Res<crate::player::Player>>,
        Option<Res<crate::cinematic::Cinematic>>,
    ),
    mut walk: ResMut<ZoneChannelWalk>,
    mut entered: MessageReader<crate::net::EnteredWorldMessage>,
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    mut catalog_fed: Local<crate::ui_script::VmMemo<Option<(String, bool)>>>,
) {
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

    // `None` with no session, during a cinematic, while the world arrives, or with no area answer.
    let zone_id = (walk.live && !cinematic_running && zone_is_settled(player.as_deref()))
        .then(|| {
            let areas = areas?;
            world.area().and_then(|leaf| areas.0.top_zone(leaf))
        })
        .flatten();
    walk.zone_id = zone_id;
    let zone = zone_id
        .and_then(|id| areas?.0.get(id))
        .map(|row| (row.name.clone(), row.flags & AREA_FLAG_TRADE_CHANNEL != 0));

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

    let (Some(zone_id), Some((zone_name, in_city)), Some(areas)) = (zone_id, zone, areas) else {
        return; // nothing to walk against yet
    };
    if channels.zone_mask.is_none() {
        return; // the mask is not seated: the reference's ready-flag bail
    }
    if walk.at == Some(zone_id) {
        return; // same zone as last frame
    }

    let steps = plan_walk(&channels, &zone_name, in_city, city_word(&areas.0));
    if apply_walk(&steps, &mut channels, &commands) {
        if let Some(script) = script.as_mut() {
            // `GetChannelName(n)` answers the new name from this frame on, as in the reference.
            script.set_joined_channels(channels.names());
        }
    }
    walk.at = Some(zone_id);
}

/// Feed the VM the zone-less catalog before any interface file runs: world entry runs FrameXML,
/// the addons and `PLAYER_LOGIN` in one call, ahead of the walk's first tick.
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
    script.set_zone_channel_catalog(zone_channel_catalog(&channels.channels, "", false, city));
}

/// Every `ChatChannels.dbc` row as the VM needs it: the id, the Shortcut the verbs match a typed
/// name against (`0x4a10b2`), the name composed here (`None` is the verbs' nil leg), and whether
/// `EnumerateServerChannels 0x4a1790` lists it (a city-only row, `flags & 0x10`, only in a city).
pub(crate) fn zone_channel_catalog(
    catalog: &ChatChannelsCatalog,
    zone_name: &str,
    in_city: bool,
    city_word: Option<&str>,
) -> Vec<benilla_ui::script::ZoneChannelRow> {
    catalog
        .rows()
        .iter()
        .map(|r| benilla_ui::script::ZoneChannelRow {
            id: r.id,
            shortcut: r.shortcut.clone(),
            resolved: compose(r, zone_name, city_word),
            listed: !r.is_city_only() || in_city,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn joined(name: &str) -> Option<crate::ui_chat::edit::ChannelSlot> {
        Some(crate::ui_chat::edit::ChannelSlot::joined(name))
    }

    fn suspended(name: &str) -> Option<crate::ui_chat::edit::ChannelSlot> {
        Some(crate::ui_chat::edit::ChannelSlot {
            name: name.into(),
            state: SlotState::Suspended,
        })
    }

    fn s(v: &str) -> String {
        v.to_string()
    }

    /// WorldDefense, LookingForGroup and GuildRecruitment have no bit in a fresh mask.
    #[test]
    fn an_ordinary_zone_joins_general_and_local_defense() {
        assert_eq!(
            wanted_channels(&catalog(), SEED, "Elwynn Forest", false, CITY),
            vec!["General - Elwynn Forest", "LocalDefense - Elwynn Forest"]
        );
    }

    #[test]
    fn a_capital_adds_the_shared_trade_channel() {
        assert_eq!(
            wanted_channels(&catalog(), SEED, "Stormwind City", true, CITY),
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
        assert!(wanted_channels(&catalog(), SEED, "", false, CITY).is_empty());
        assert!(wanted_channels(&catalog(), SEED, "", true, CITY).is_empty());
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
            wanted_channels(&catalog(), SEED, "Stormwind City", true, None),
            vec!["General - Stormwind City", "LocalDefense - Stormwind City"]
        );
    }

    #[test]
    fn crossing_a_zone_border_renames_the_same_rows() {
        let cat = catalog();
        let felwood = wanted_channels(&cat, SEED, "Felwood", false, CITY);
        let winterspring = wanted_channels(&cat, SEED, "Winterspring", false, CITY);
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
        let steps = plan_walk(&c, "Elwynn Forest", false, CITY);
        assert_eq!(
            steps,
            vec![
                W::Register(s("General - Elwynn Forest")),
                W::Join(s("General - Elwynn Forest")),
                W::Register(s("Trade - City")),
                W::Suspend(s("Trade - City")),
                W::Register(s("LocalDefense - Elwynn Forest")),
                W::Join(s("LocalDefense - Elwynn Forest")),
            ]
        );
        apply_steps(&steps, &mut c);
        assert_eq!(c.number_of("General - Elwynn Forest"), Some(1));
        assert_eq!(c.number_of("Trade - City"), Some(2));
        assert_eq!(c.slot_state("Trade - City"), Some(SlotState::Suspended));
        assert_eq!(c.number_of("LocalDefense - Elwynn Forest"), Some(3));

        let steps = plan_walk(&state(SEED), "Stormwind City", true, CITY);
        assert_eq!(
            steps,
            vec![
                W::Register(s("General - Stormwind City")),
                W::Join(s("General - Stormwind City")),
                W::Register(s("Trade - City")),
                W::Join(s("Trade - City")),
                W::Register(s("LocalDefense - Stormwind City")),
                W::Join(s("LocalDefense - Stormwind City")),
            ]
        );
    }

    /// Apply a plan's slot writes as the system does, without the wire.
    fn apply_steps(steps: &[WalkStep], c: &mut ChannelState) {
        for step in steps {
            match step {
                W::Leave(_) | W::Join(_) => {}
                W::Rename { old, new } => {
                    c.rename_slot(old, new);
                }
                W::Register(name) => {
                    c.claim_slot(name);
                }
                W::Suspend(name) => {
                    c.suspend_slot(name);
                }
                W::Confirm(name) => c.confirm_slot(name),
            }
        }
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
        let steps = plan_walk(&c, "Winterspring", false, CITY);
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
        apply_steps(&steps, &mut c);
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

    /// Trade suspends on the way out (`0x49bcf0`) and re-joins through the state-3 bypass.
    #[test]
    fn a_city_round_trip_keeps_the_slot_and_its_number() {
        let mut c = state(SEED);
        c.joined = vec![
            joined("General - Stormwind City"),
            joined("Trade - City"),
            joined("LocalDefense - Stormwind City"),
        ];

        // Out of the city.
        let steps = plan_walk(&c, "Elwynn Forest", false, CITY);
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
        apply_steps(&steps, &mut c);
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
        let steps = plan_walk(&c, "Stormwind City", true, CITY);
        assert_eq!(
            steps,
            vec![
                W::Leave(s("General - Elwynn Forest")),
                W::Rename {
                    old: s("General - Elwynn Forest"),
                    new: s("General - Stormwind City")
                },
                W::Join(s("General - Stormwind City")),
                W::Confirm(s("Trade - City")),
                W::Join(s("Trade - City")),
                W::Leave(s("LocalDefense - Elwynn Forest")),
                W::Rename {
                    old: s("LocalDefense - Elwynn Forest"),
                    new: s("LocalDefense - Stormwind City")
                },
                W::Join(s("LocalDefense - Stormwind City")),
            ]
        );
        apply_steps(&steps, &mut c);
        assert_eq!(c.slot_state("Trade - City"), Some(SlotState::Joined));
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
        let target = c.leave_target("1").expect("slot 1 is confirmed");
        assert_eq!(target, "General - Elwynn Forest", "the slot's wire name");
        c.note_zone_channel_left(&target);
        assert_eq!(c.zone_mask, Some(SEED & !1), "bit 0 cleared");
        // The server's `YOU_LEFT` frees the slot in place.
        assert_eq!(c.free_slot(&target), Some(1));

        // The next border: General is neither walked (no slot) nor registered (no bit).
        let steps = plan_walk(&c, "Westfall", false, CITY);
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
        let relog = state(SEED & !1);
        let steps = plan_walk(&relog, "Elwynn Forest", false, CITY);
        assert_eq!(
            steps,
            vec![
                W::Register(s("Trade - City")),
                W::Suspend(s("Trade - City")),
                W::Register(s("LocalDefense - Elwynn Forest")),
                W::Join(s("LocalDefense - Elwynn Forest")),
            ],
            "General is gone for good; Trade is now number 1"
        );

        // Until a `/join General` confirms and sets the bit back.
        let mut relog = relog;
        apply_steps(&steps, &mut relog);
        relog.note_zone_channel_joined("General - Elwynn Forest");
        relog.claim_slot("General - Elwynn Forest");
        assert_eq!(relog.zone_mask, Some(SEED));
        let steps = plan_walk(&relog, "Westfall", false, CITY);
        assert!(
            steps.contains(&W::Join(s("General - Westfall"))),
            "walked again from its slot: {steps:?}"
        );
    }

    /// Zero is a real state, every zone channel left; unseated means the file is not read yet.
    #[test]
    fn a_zero_or_unseated_mask_registers_nothing() {
        assert!(plan_walk(&state(0), "Elwynn Forest", false, CITY).is_empty());
        let unseated = ChannelState {
            channels: catalog(),
            zone_mask: None,
            ..Default::default()
        };
        assert!(plan_walk(&unseated, "Elwynn Forest", false, CITY).is_empty());
        assert!(!unseated.zone_row_wanted(1));
    }

    /// Once the cascade's confirmed join sets bit 24, `GuildRecruitment` takes the same city gate
    /// (`0x10`) and city name (`0x20`) as Trade, last in DBC order.
    #[test]
    fn the_guild_recruitment_bit_rides_the_walk_like_trade() {
        let steps = plan_walk(
            &state(SEED | GUILD_RECRUITMENT_BIT),
            "Elwynn Forest",
            false,
            CITY,
        );
        assert_eq!(
            &steps[6..],
            &[
                W::Register(s("GuildRecruitment - City")),
                W::Suspend(s("GuildRecruitment - City")),
            ],
            "registered and suspended out here, number 4"
        );
        let steps = plan_walk(
            &state(SEED | GUILD_RECRUITMENT_BIT),
            "Ironforge",
            true,
            CITY,
        );
        assert_eq!(
            &steps[6..],
            &[
                W::Register(s("GuildRecruitment - City")),
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
        // The cascade's join: a slot, then the notice sets the bit.
        c.claim_slot("GuildRecruitment - City");
        let steps = plan_walk(&c, "Stormwind City", true, CITY);
        assert!(
            steps.is_empty(),
            "same zone, every name unchanged, every row eligible — nothing to do: {steps:?}"
        );
        // Walking out suspends it exactly like Trade.
        let steps = plan_walk(&c, "Elwynn Forest", false, CITY);
        assert!(steps.contains(&W::Leave(s("GuildRecruitment - City"))));
        assert!(steps.contains(&W::Suspend(s("GuildRecruitment - City"))));
        assert!(!steps.iter().any(|st| matches!(st, W::Register(_))));
    }

    /// State 3 sends no LEAVE, and the rename's `(old == 3) ? 1 : 2` lands it on 1, a plain
    /// `YOU_JOINED`.
    #[test]
    fn a_suspended_slot_whose_name_moved_rejoins_without_a_leave() {
        let mut c = state(SEED);
        c.joined = vec![suspended("General - Elwynn Forest")];
        let steps = plan_walk(&c, "Westfall", false, CITY);
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
        apply_steps(&steps, &mut c);
        assert_eq!(
            c.slot_state("General - Westfall"),
            Some(SlotState::Joined),
            "3 → 1, not 2"
        );
    }

    #[test]
    fn leave_target_resolves_a_number_to_a_confirmed_slot_or_to_nothing() {
        let mut c = state(SEED);
        c.joined = vec![
            joined("General - Elwynn Forest"),
            suspended("Trade - City"),
            None,
            joined("MyChan"),
        ];
        assert_eq!(
            c.leave_target("1").as_deref(),
            Some("General - Elwynn Forest")
        );
        assert_eq!(
            c.leave_target("1abc").as_deref(),
            Some("General - Elwynn Forest"),
            "SStrToInt takes the leading digits"
        );
        assert_eq!(
            c.leave_target("2"),
            None,
            "suspended: state 3 fails `0x49be50`, and the whole call is a no-op"
        );
        assert_eq!(c.leave_target("3"), None, "a hole");
        assert_eq!(c.leave_target("9"), None, "out of range");
        assert_eq!(c.leave_target("-1"), None);
        assert_eq!(c.leave_target("0").as_deref(), Some("0"), "0 is a name");
        assert_eq!(
            c.leave_target("").as_deref(),
            Some(""),
            "`/leave` alone sends an empty name"
        );
        assert_eq!(
            c.leave_target("General - Elwynn Forest").as_deref(),
            Some("General - Elwynn Forest"),
            "a composed name passes through"
        );
        assert_eq!(c.leave_target("mychan").as_deref(), Some("mychan"));

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
            tracked_channels(&cat, SEED, "Stormwind City", None),
            wanted_channels(&cat, SEED, "Stormwind City", true, None),
            "no city word: the city-NAMED rows are unresolvable, so neither list carries them"
        );
        assert!(
            tracked_channels(&cat, SEED, "", CITY).is_empty(),
            "no zone: nothing zone-dependent composes, so nothing takes a slot either"
        );
    }

    /// Zone-less is not empty: every row stays, and a zone-dependent one carries `resolved: None`,
    /// the reference's nil leg (`0x4a10d9`, `0x49ece5`), not the custom-channel leg.
    #[test]
    fn a_zoneless_catalog_still_carries_every_row() {
        let rows = zone_channel_catalog(&catalog(), "", false, None);
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
            Some("WorldDefense".to_string()),
            "a row whose name carries no %s resolves with no zone at all"
        );

        // Once a zone is known, the same rows resolve.
        let rows = zone_channel_catalog(&catalog(), "Elwynn Forest", false, CITY);
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
}
