//! The world broadcasts that print in chat, resolved from their wire ids once the catalogs, the
//! joined channels and the player's zone are at hand, a pass before the chat drain.
//!
//! The two defense broadcasts (handlers `0x49dcc0`, `0x49de30`) print as senderless
//! `CHAT_MSG_CHANNEL` lines in language 0, once per joined channel whose `ChatChannels.dbc` row
//! has `DEFENSE` (`0x10000`, `0x49dd94`); a `ZONE_DEP` (`0x2`) row also needs the broadcast's zone
//! to be the player's (`0x49dda4`). Both compare the area's parent zone (`0x49dd2b`, `0x49de80`),
//! while ZONE_UNDER_ATTACK's text names the area itself (`0x49dd05`). With no `AreaTable.dbc` row
//! ZONE_UNDER_ATTACK prints nothing (`0x49dcdc`) and DEFENSE_MESSAGE compares the raw id
//! (`0x49de8a`). vmangos sends DEFENSE_MESSAGE to the whole map (`Map.cpp:1868`), but only rows
//! 1, 2 and 22 carry `INITIAL`, so the auto-join never joins WorldDefense and in practice only
//! LocalDefense, in the broadcast's zone, hears them.
//!
//! `SMSG_SERVER_MESSAGE` is one `CHAT_MSG_SYSTEM` line (`0x49e047`). `SMSG_CHAT_RESTRICTED`
//! carries nothing and is `DisplayError` 451 (`0x5e4a09`, `0x496720`), `ERR_CHAT_RESTRICTED`, a
//! chat row, not a red toast (`0x496822`, `0x484cac`); vmangos never sends it.

use bevy::prelude::*;

use benilla_assets::{LockRecover, WorldAssets};
use benilla_formats::{chat_channel_flags as chan_flags, ServerMessagesCatalog};

use crate::area::AreaTableRes;

use super::edit::ChannelState;
use super::event::{ChatEvent, ChatEventKind};
use super::feed::ChatLog;

/// `ServerMessages.dbc`, read once at startup: the shutdown and restart sentences.
#[derive(Resource)]
pub(crate) struct ServerMessages(pub(crate) ServerMessagesCatalog);

/// One parked world broadcast, as the wire sent it, awaiting the catalogs and the channel walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Broadcast {
    /// `SMSG_ZONE_UNDER_ATTACK`: the attacked area's `AreaTable.dbc` id.
    ZoneUnderAttack { area_id: u32 },
    /// `SMSG_DEFENSE_MESSAGE`: the zone it is about, and the server's text.
    Defense { zone_id: u32, text: String },
    /// `SMSG_SERVER_MESSAGE`: a `ServerMessages.dbc` row id and the text for its `%s`.
    Server { message_type: u32, text: String },
    /// `SMSG_CHAT_RESTRICTED`, which carries nothing.
    ChatRestricted,
}

/// Load `ServerMessages.dbc`; it must run after `benilla_assets::AssetSet::Open`, or it finds no
/// patch chain and loads nothing.
pub(super) fn load_server_messages(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_server_messages_catalog(&mut chain)
    };
    match loaded {
        // The row count tells an empty load from a healthy one.
        Ok(cat) => {
            info!("chat: {} ServerMessages rows", cat.len());
            commands.insert_resource(ServerMessages(cat));
        }
        Err(e) => warn!("chat: ServerMessages.dbc failed to load: {e:#}"),
    }
}

/// The joined channels a defense broadcast about `subject_zone` reaches, in slot order. With
/// `player_zone` unknown only non-`ZONE_DEP` rows pass, as with the reference's unset
/// `[0xb6e5d0]`. Every slot counts whatever its state; the reference takes only state 0
/// (`[e+0x9c]`, `0x49dd5a`), skipping pending, renamed and suspended slots.
pub(crate) fn defense_targets(
    channels: &ChannelState,
    subject_zone: u32,
    player_zone: Option<u32>,
) -> Vec<String> {
    channels
        .iter_names()
        .filter(|name| {
            let Some(row) = channels.channels.row_for_name(name) else {
                return false; // a custom channel: no DBC row, no defense flag
            };
            if row.flags & chan_flags::DEFENSE == 0 {
                return false;
            }
            row.flags & chan_flags::ZONE_DEP == 0 || player_zone == Some(subject_zone)
        })
        .map(str::to_string)
        .collect()
}

/// The VM's `ZONE_UNDER_ATTACK` string (`0x49dd14`) with the area's name in its first `%s`: the
/// reference's `snprintf` (`0x49dd26`) passes one argument.
fn zone_under_attack_line(template: &str, area_name: &str) -> String {
    template.replacen("%s", area_name, 1)
}

/// The area's parent zone, or the area itself when it has none (`0x49dd2b`, `0x49de80`).
fn parent_zone(areas: &AreaTableRes, id: u32) -> Option<u32> {
    let row = areas.0.get(id)?;
    Some(if row.zone_id == 0 { id } else { row.zone_id })
}

/// Resolve the parked broadcasts into chat events, before [`super::feed::feed_chat`] so each
/// lands the frame it decodes.
pub(super) fn feed_broadcasts(
    mut log: ResMut<ChatLog>,
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    channels: Res<ChannelState>,
    areas: Option<Res<AreaTableRes>>,
    messages: Option<Res<ServerMessages>>,
    world: benilla_world::world_point::WorldPoint,
) {
    if log.broadcasts_pending() == 0 {
        return;
    }
    // The format string lives in the VM, so the queue waits for one; it clears at session end.
    let Some(script) = script else { return };
    // The player's zone, the reference's `[0xb6e5d0]`; `top_zone` is the single-hop parent on
    // 5875 data.
    let player_zone = areas
        .as_ref()
        .and_then(|a| world.area().and_then(|leaf| a.0.top_zone(leaf)));
    for item in log.take_broadcasts() {
        match item {
            Broadcast::ZoneUnderAttack { area_id } => {
                // No catalog, template or row: with no name there is no line.
                let Some((areas, fmt)) = areas
                    .as_ref()
                    .zip(super::combat::global_string(&script, "ZONE_UNDER_ATTACK"))
                else {
                    continue;
                };
                let (Some(name), Some(zone)) = (areas.0.name(area_id), parent_zone(areas, area_id))
                else {
                    debug!("chat: ZONE_UNDER_ATTACK for unknown area {area_id}");
                    continue;
                };
                let text = zone_under_attack_line(&fmt, name);
                push_channel_lines(&mut log, &channels, zone, player_zone, &text);
            }
            Broadcast::Defense { zone_id, text } => {
                // The wire text stands on its own, so an unresolvable zone only costs the remap.
                let zone = areas
                    .as_ref()
                    .and_then(|a| parent_zone(a, zone_id))
                    .unwrap_or(zone_id);
                push_channel_lines(&mut log, &channels, zone, player_zone, &text);
            }
            Broadcast::ChatRestricted => {
                // Its row is a chat row (tested below), so it is written as a system line here.
                // Deviation: a missing global prints nothing, where the reference prints an empty
                // line (`0x703c02`), because benilla's other `DisplayError` routes do the same.
                if let Some(text) = super::combat::global_string(&script, "ERR_CHAT_RESTRICTED") {
                    log.push_event(ChatEvent::text_only(ChatEventKind::System, text));
                }
            }
            Broadcast::Server { message_type, text } => {
                let Some(messages) = messages.as_ref() else {
                    debug!("chat: SMSG_SERVER_MESSAGE {message_type} dropped: no catalog");
                    continue;
                };
                log.push_event(ChatEvent::text_only(
                    ChatEventKind::System,
                    messages.0.compose(message_type, &text),
                ));
            }
        }
    }
}

/// One `CHAT_MSG_CHANNEL` line per defense channel reached, with no sender or language: the
/// handlers push NULL for both (`0x49ddf5`/`0x49ddf7`), hence the bare colon of
/// `[LocalDefense] : text`.
fn push_channel_lines(
    log: &mut ChatLog,
    channels: &ChannelState,
    zone: u32,
    player_zone: Option<u32>,
    text: &str,
) {
    for channel in defense_targets(channels, zone, player_zone) {
        log.push_event(ChatEvent {
            kind: Some(ChatEventKind::Channel),
            text: text.to_string(),
            channel,
            ..Default::default()
        });
    }
}

#[cfg(test)]
mod tests {
    /// `ERR_CHAT_RESTRICTED` (451) is a chat row, so the arm above writes a system line.
    #[test]
    fn chat_restricted_is_a_chat_row() {
        use benilla_ui::messages::{by_key, kind_of, MsgKind};
        assert_eq!(by_key("ERR_CHAT_RESTRICTED").expect("a real row").id, 451);
        assert_eq!(kind_of("ERR_CHAT_RESTRICTED"), MsgKind::Chat);
    }

    use super::*;
    use benilla_formats::{ChatChannelRow, ChatChannelsCatalog};

    /// Three shipped `ChatChannels.dbc` rows, as `benilla_formats::chat_channels` tests them.
    fn catalog() -> ChatChannelsCatalog {
        ChatChannelsCatalog::from_rows(vec![
            ChatChannelRow {
                id: 1,
                flags: 0x0_0003,
                pattern: "General - %s".into(),
                shortcut: "General".into(),
            },
            ChatChannelRow {
                id: 22,
                flags: 0x1_0003,
                pattern: "LocalDefense - %s".into(),
                shortcut: "LocalDefense".into(),
            },
            ChatChannelRow {
                id: 23,
                flags: 0x1_0004,
                pattern: "WorldDefense".into(),
                shortcut: "WorldDefense".into(),
            },
        ])
    }

    fn state(joined: &[&str]) -> ChannelState {
        ChannelState {
            joined: joined
                .iter()
                .map(|n| Some(crate::ui_chat::edit::ChannelSlot::joined(n)))
                .collect(),
            channels: catalog(),
            ..Default::default()
        }
    }

    const WESTFALL: u32 = 40;
    const ELWYNN: u32 = 12;

    #[test]
    fn only_defense_channels_hear_it_and_local_only_in_zone() {
        let s = state(&[
            "General - Westfall",
            "LocalDefense - Westfall",
            "WorldDefense",
        ]);
        assert_eq!(
            defense_targets(&s, WESTFALL, Some(WESTFALL)),
            vec!["LocalDefense - Westfall", "WorldDefense"]
        );
        // In Elwynn, a Westfall alarm reaches only the global channel.
        let s = state(&[
            "General - Elwynn Forest",
            "LocalDefense - Elwynn Forest",
            "WorldDefense",
        ]);
        assert_eq!(
            defense_targets(&s, WESTFALL, Some(ELWYNN)),
            vec!["WorldDefense"]
        );
    }

    #[test]
    fn a_player_in_no_defense_channel_hears_nothing() {
        let s = state(&["General - Westfall"]);
        assert!(defense_targets(&s, WESTFALL, Some(WESTFALL)).is_empty());
    }

    #[test]
    fn a_custom_channel_is_never_a_defense_channel() {
        let s = state(&["mydefense"]);
        assert!(defense_targets(&s, WESTFALL, Some(WESTFALL)).is_empty());
    }

    #[test]
    fn an_unknown_player_zone_still_reaches_world_defense() {
        let s = state(&["LocalDefense - Westfall", "WorldDefense"]);
        assert_eq!(defense_targets(&s, WESTFALL, None), vec!["WorldDefense"]);
    }

    /// [`ChannelState::free_slot`] clears in place, leaving a hole the walk skips.
    #[test]
    fn a_freed_slot_does_not_end_the_walk() {
        let mut s = state(&["LocalDefense - Westfall", "WorldDefense"]);
        s.joined[0] = None;
        assert_eq!(
            defense_targets(&s, WESTFALL, Some(WESTFALL)),
            vec!["WorldDefense"]
        );
    }

    /// The `%s` takes the area's name, not its zone's; the colour codes pass through.
    #[test]
    fn the_line_fills_the_frame_xml_template_with_the_area_name() {
        assert_eq!(
            zone_under_attack_line("|cffffff00%s is under attack!|r", "Sentinel Hill"),
            "|cffffff00Sentinel Hill is under attack!|r"
        );
    }

    /// The bare colon is the reference's: `ChatFrame_OnEvent` fills `CHAT_CHANNEL_GET` (`"%s: "`)
    /// with the empty sender (`ChatFrame.lua:1453`).
    #[test]
    fn a_defense_line_renders_with_the_references_bare_colon() {
        benilla_formats::wow_data_or_skip!();
        let event = ChatEvent {
            kind: Some(ChatEventKind::Channel),
            text: "|cffffff00Sentinel Hill is under attack!|r".into(),
            // arg4 as `stamp_channel` leaves it once the channel is joined: numbered.
            channel: "3. LocalDefense - Westfall".into(),
            ..Default::default()
        };
        assert_eq!(
            crate::ui_chat::tests::compose(&event, ChatEventKind::Channel, "Common").unwrap(),
            "[3. LocalDefense] : |cffffff00Sentinel Hill is under attack!|r"
        );
    }
}
