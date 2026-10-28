//! The world broadcasts that print in chat: each queued on the chat lane at its packet, and resolved
//! from its wire ids against the VM's strings, the catalogs, the joined channels and the player's
//! zone where it fires, at the packet's place.
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
//! `SMSG_TRANSFER_ABORTED` goes the same way, its line a `GlobalStrings` text by reason
//! ([`transfer_abort_line`]).

use bevy::prelude::*;

use benilla_assets::{LockRecover, WorldAssets};
use benilla_formats::{chat_channel_flags as chan_flags, ServerMessagesCatalog};

use crate::area::AreaTableRes;

use super::edit::ChannelState;
use super::event::{ChatEvent, ChatEventKind};

/// `ServerMessages.dbc`, read once at startup: the shutdown and restart sentences.
#[derive(Resource)]
pub(crate) struct ServerMessages(pub(crate) ServerMessagesCatalog);

/// One world broadcast as the wire sent it, on the chat lane until it fires ([`resolve`]).
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
    /// `SMSG_TRANSFER_ABORTED`: the reason byte its handler reads (`0x4019aa`).
    TransferAborted { reason: u8 },
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

/// The joined channels a defense broadcast about `subject_zone` reaches, in slot order: the slots
/// in state 0 (`0x49dd5a`-`0x49dd63`, `0x49deaa`-`0x49deb3`) whose ChannelID names a `DEFENSE`
/// row (`0x49dd69`-`0x49dd94`), a `ZONE_DEP` one only when the zone is the player's
/// (`0x49dd9c`-`0x49dda4`). With `player_zone` unknown only non-`ZONE_DEP` rows pass, as with the
/// reference's unset `[0xb6e5d0]`.
pub(crate) fn defense_targets(
    channels: &ChannelState,
    subject_zone: u32,
    player_zone: Option<u32>,
) -> Vec<String> {
    channels
        .confirmed_slots()
        .filter(|(_, slot)| {
            let Some(row) = channels.channels.row(slot.id) else {
                return false; // a custom channel: no DBC row, no defense flag
            };
            if row.flags & chan_flags::DEFENSE == 0 {
                return false;
            }
            row.flags & chan_flags::ZONE_DEP == 0 || player_zone == Some(subject_zone)
        })
        .map(|(_, slot)| slot.name.clone())
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

/// What a broadcast's resolve reads beside the VM and the joined channels: the area and message
/// catalogs and the area under the player.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct BroadcastReads<'w> {
    areas: Option<Res<'w, AreaTableRes>>,
    messages: Option<Res<'w, ServerMessages>>,
    area: benilla_world::world_point::AreaUnderfoot<'w>,
}

/// One line a broadcast prints.
pub(super) enum BroadcastLine {
    /// A ready line the display latch holds (`0x49cae0`): a defence broadcast's.
    Display(ChatEvent),
    /// A system line shown at once.
    Event(ChatEvent),
}

/// The lines `item` prints, in its handler's order, resolved where it fires on the chat lane, as
/// the handlers compose them (`0x49dcc0`, `0x49de30`, `0x49e047`, `0x5e4a09`, `0x4019a0`): the
/// format strings off the VM, so with none it prints nothing.
pub(super) fn resolve(
    item: &Broadcast,
    script: &benilla_ui::script::UiScript,
    channels: &ChannelState,
    reads: &BroadcastReads,
) -> Vec<BroadcastLine> {
    let BroadcastReads {
        areas,
        messages,
        area,
    } = reads;
    // The player's zone, the reference's `[0xb6e5d0]`; `top_zone` is the single-hop parent on
    // 5875 data.
    let player_zone = areas
        .as_ref()
        .and_then(|a| area.area().and_then(|leaf| a.0.top_zone(leaf)));
    match item {
        Broadcast::ZoneUnderAttack { area_id } => {
            // No catalog, template or row: with no name there is no line.
            let Some((areas, fmt)) = areas
                .as_ref()
                .zip(super::combat::global_string(script, "ZONE_UNDER_ATTACK"))
            else {
                return Vec::new();
            };
            let (Some(name), Some(zone)) = (areas.0.name(*area_id), parent_zone(areas, *area_id))
            else {
                debug!("chat: ZONE_UNDER_ATTACK for unknown area {area_id}");
                return Vec::new();
            };
            let text = zone_under_attack_line(&fmt, name);
            channel_lines(channels, zone, player_zone, &text)
        }
        Broadcast::Defense { zone_id, text } => {
            // The wire text stands on its own, so an unresolvable zone only costs the remap.
            let zone = areas
                .as_ref()
                .and_then(|a| parent_zone(a, *zone_id))
                .unwrap_or(*zone_id);
            channel_lines(channels, zone, player_zone, text)
        }
        // Its row is a chat row (tested below), so it is written as a system line here.
        // Deviation: a missing global prints nothing, where the reference prints an empty line
        // (`0x703c02`), because benilla's other `DisplayError` routes do the same.
        Broadcast::ChatRestricted => super::combat::global_string(script, "ERR_CHAT_RESTRICTED")
            .map(|text| BroadcastLine::Event(ChatEvent::text_only(ChatEventKind::System, text)))
            .into_iter()
            .collect(),
        Broadcast::TransferAborted { reason } => {
            let get = |key: &str| {
                script
                    .lua()
                    .globals()
                    .get::<Option<String>>(key)
                    .ok()
                    .flatten()
            };
            transfer_abort_line(*reason, get)
                .map(|text| BroadcastLine::Event(ChatEvent::text_only(ChatEventKind::System, text)))
                .into_iter()
                .collect()
        }
        Broadcast::Server { message_type, text } => {
            let Some(messages) = messages.as_ref() else {
                debug!("chat: SMSG_SERVER_MESSAGE {message_type} dropped: no catalog");
                return Vec::new();
            };
            vec![BroadcastLine::Event(ChatEvent::text_only(
                ChatEventKind::System,
                messages.0.compose(*message_type, text),
            ))]
        }
    }
}

/// `SMSG_TRANSFER_ABORTED`'s system line (`0x4019a0`): reasons 1, 2, 3 and 5 name a string by the
/// jump table at `0x401a20`, read through `get` and shown as `CHAT_MSG_SYSTEM` (`0x4019f8`); 4 and
/// any other reason show nothing (`0x401a04`). A string the VM lacks is an empty line, as
/// `GetText` (`0x703bf0`) answers one.
pub(crate) fn transfer_abort_line(
    reason: u8,
    get: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    let key = match reason {
        1 => "TRANSFER_ABORT_MAX_PLAYERS",
        2 => "TRANSFER_ABORT_NOT_FOUND",
        3 => "TRANSFER_ABORT_TOO_MANY_INSTANCES",
        5 => "TRANSFER_ABORT_ZONE_IN_COMBAT",
        _ => return None,
    };
    Some(get(key).unwrap_or_default())
}

/// One `CHAT_MSG_CHANNEL` line per defense channel reached, with no sender or language: the
/// handlers push NULL for both (`0x49ddf5`/`0x49ddf7`), hence the bare colon of
/// `[LocalDefense] : text`. The display latch holds them, as it holds a chat line.
fn channel_lines(
    channels: &ChannelState,
    zone: u32,
    player_zone: Option<u32>,
    text: &str,
) -> Vec<BroadcastLine> {
    defense_targets(channels, zone, player_zone)
        .into_iter()
        .map(|channel| {
            BroadcastLine::Display(ChatEvent {
                kind: Some(ChatEventKind::Channel),
                text: text.to_string(),
                channel,
                ..Default::default()
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    /// `0x4019a0` shows reasons 1, 2, 3 and 5 by their jump table (`0x401a20`), 4 lands on the
    /// console line with no text (`0x401a04`), and so does any reason past 5 or 0
    /// (`0x4019b3` `dec eax; cmp eax,4; ja`).
    #[test]
    fn a_transfer_abort_shows_its_reasons_string() {
        let stock = |key: &str| {
            Some(
                match key {
                    "TRANSFER_ABORT_MAX_PLAYERS" => "Transfer Aborted: Instance is full",
                    "TRANSFER_ABORT_NOT_FOUND" => "Transfer Aborted: instance not found",
                    "TRANSFER_ABORT_TOO_MANY_INSTANCES" => {
                        "You have entered too many instances recently."
                    }
                    "TRANSFER_ABORT_ZONE_IN_COMBAT" => {
                        "Unable to zone in while an encounter is in progress."
                    }
                    _ => return None,
                }
                .to_string(),
            )
        };
        let line = |reason| super::transfer_abort_line(reason, stock);
        assert_eq!(
            line(1).as_deref(),
            Some("Transfer Aborted: Instance is full")
        );
        assert_eq!(
            line(2).as_deref(),
            Some("Transfer Aborted: instance not found")
        );
        assert_eq!(
            line(3).as_deref(),
            Some("You have entered too many instances recently.")
        );
        assert_eq!(
            line(5).as_deref(),
            Some("Unable to zone in while an encounter is in progress.")
        );
        for silent in [0, 4, 6, 255] {
            assert_eq!(line(silent), None, "reason {silent}");
        }
        // `GetText` (`0x703bf0`) gives the empty string for a key the strings lack, and the line
        // is added all the same (`0x4019ff`).
        assert_eq!(super::transfer_abort_line(1, |_| None).as_deref(), Some(""));
    }

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

    /// Confirmed slots, each carrying the row its name composes from.
    fn state(joined: &[&str]) -> ChannelState {
        ChannelState {
            joined: joined
                .iter()
                .map(|n| {
                    crate::ui_chat::edit::ChannelSlot::confirmed(n, catalog().zone_channel_id(n))
                        .into()
                })
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
        s.free_slot("LocalDefense - Westfall");
        assert_eq!(
            defense_targets(&s, WESTFALL, Some(WESTFALL)),
            vec!["WorldDefense"]
        );
    }

    /// Both handlers skip a slot whose state is not 0 (`0x49dd5a`-`0x49dd63`, `0x49deaa`-`0x49deb3`):
    /// one the walk registered and the server has not confirmed (1), renamed (2) or suspended (3).
    #[test]
    fn the_defense_broadcasts_skip_every_slot_the_server_has_not_confirmed() {
        use crate::ui_chat::edit::SlotState;
        for state_of in [SlotState::Pending, SlotState::Renamed, SlotState::Suspended] {
            let mut s = state(&["LocalDefense - Westfall", "WorldDefense"]);
            for record in s.joined.iter_mut() {
                if let crate::ui_chat::edit::SlotRecord::Held(slot) = record {
                    slot.state = state_of;
                }
            }
            assert!(
                defense_targets(&s, WESTFALL, Some(WESTFALL)).is_empty(),
                "{state_of:?}"
            );
        }
    }

    /// The handlers read the row through the slot's id (`0x49dd69`-`0x49dd94`): a custom channel
    /// whose name holds a defense row's pattern has id 0.
    #[test]
    fn a_custom_channel_named_like_a_defense_row_is_not_one() {
        let mut s = state(&["WorldDefense"]);
        s.joined[0] = crate::ui_chat::edit::ChannelSlot::confirmed("WorldDefense2", 23).into();
        s.joined
            .push(crate::ui_chat::edit::ChannelSlot::confirmed("WorldDefense3", 0).into());
        assert_eq!(
            defense_targets(&s, WESTFALL, Some(WESTFALL)),
            vec!["WorldDefense2"],
            "the slot that carries row 23 hears it, under whatever name; the custom one does not"
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
