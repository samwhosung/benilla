//! The world broadcasts — the three that become chat lines, and the resolve pass they need first.
//!
//! `SMSG_ZONE_UNDER_ATTACK`, `SMSG_DEFENSE_MESSAGE` and `SMSG_SERVER_MESSAGE` arrive as ids and
//! fills, never as finished lines: an `AreaTable.dbc` id to name, a `ServerMessages.dbc` row to
//! format, and — for the two defense broadcasts — **the joined-channel list to walk**. That last one
//! is why they cannot ride [`super::feed`]'s drain: the walk needs the AreaTable catalog and the
//! zone under the player, which the chat drain does not hold. So the wire arms park them on
//! [`super::ChatLog`]'s second queue and this module's system resolves each into an ordinary
//! [`ChatEvent`], one frame earlier in the same schedule.
//!
//! ## The defense broadcasts are CHANNEL lines, not system lines
//!
//! This is the part a reimplementation gets wrong, and the reference is unambiguous about it. Both
//! handlers (`0x49dcc0` for ZONE_UNDER_ATTACK, `0x49de30` for DEFENSE_MESSAGE) end in the same
//! loop over the client's own joined-channel array (`[0xb4fe04]`, count `[0xb4fe00]`, stride
//! `0xa0`), delivering the
//! text once per surviving channel as chat type **`0xE` = `CHAT_MSG_CHANNEL`** with a NULL sender
//! and language `0`. `0x49de04` and `0x49df4c` are the **only two sites in the whole image** that
//! hand the composer a literal chat type `0xE`: engine-composed channel chat exists for these two
//! opcodes and nothing else. A channel survives when:
//!
//! - its slot is taken — `[e+0x00]` is the **1-based slot number**, `0` meaning free, not a
//!   liveness flag — and its join state is `joined` (`[e+0x9c] == 0`);
//! - its `ChatChannels.dbc` row exists and carries `DEFENSE` (`0x10000`) — `0x49dd94`
//!   `test ecx,0x10000`. In the shipped table that is exactly rows 22 (LocalDefense) and 23
//!   (WorldDefense);
//! - and, **if** the row also carries `ZONE_DEP` (`0x2` — LocalDefense but not WorldDefense), the
//!   broadcast's zone equals the player's current zone (`0x49dda4`
//!   `cmp ecx, ds:0xb6e5d0`).
//!
//! So a character who has joined neither channel sees **nothing at all**, and that is the faithful
//! answer rather than a dropped packet. The whole test is [`defense_targets`].
//!
//! **In practice that means LocalDefense alone**, and it is worth writing down because it later
//! reads as a bug: only rows 1, 2 and 22 carry `INITIAL`, so the auto-join walk never joins
//! **WorldDefense** — the reference does not either (its own `chat-cache.txt` persists
//! `ZONECHANNELS 0x200003`, General + Trade + LocalDefense). vmangos sends `SMSG_DEFENSE_MESSAGE`
//! to the whole *map*, but the client's filter narrows it to players standing in the broadcast's
//! zone unless they typed `/join WorldDefense` themselves. An Eastern Plaguelands tower capture
//! reaching only players in the Eastern Plaguelands is the mechanism working.
//!
//! **The zone compared is the PARENT.** Both handlers resolve the packet's id through
//! `AreaTable.dbc` and replace it with the row's parent (`[rec+0x8]`) when that is nonzero
//! (`0x49dd2b`, `0x49de80`) — so "Sentinel Hill is under attack!" is matched against *Westfall*, and
//! a player standing anywhere in Westfall hears it. ZONE_UNDER_ATTACK's *text* still names the
//! packet's own area, because the name is read off the row **before** the remap (`0x49dd05`).
//!
//! **A missing row is not the same refusal in the two handlers**, and that difference is
//! deliberate on the client's side: ZONE_UNDER_ATTACK bails outright (`0x49dcdc`/`0x49dce8`/
//! `0x49dcf9` all jump to the epilogue — with no row there is no name, so there is no line),
//! while DEFENSE_MESSAGE keeps its wire text and simply skips the remap (`0x49de8a`), leaving the
//! raw zone id as the comparison subject.
//!
//! ## `SMSG_SERVER_MESSAGE` is the ordinary one
//!
//! One `CHAT_MSG_SYSTEM` line (chat type `0xA`, `0x49e047`), composed by
//! [`benilla_formats::ServerMessagesCatalog::compose`] — the DBC row is the format string, the
//! packet's text is its `%s`, and a type with no row falls back to the client's own `"[%d]: %s"`.
//!
//! ## `SMSG_CHAT_RESTRICTED` is a system line too, and that is not the obvious answer
//!
//! Its arm reads nothing off the wire — `0x5e4a09` is `push 0x1c3; call 0x496720`, three
//! instructions — so the whole packet is "show message 451". benilla already models
//! `CGGameUI::DisplayError 0x496720` and its message registry
//! ([`benilla_ui::messages::MsgKind`]); what this packet needed was the two facts that registry
//! entry carries, and both are VERIFIED here:
//!
//! - **451 is `ERR_CHAT_RESTRICTED`.** The string `"ERR_CHAT_RESTRICTED"` (`0x83f6d4`) has exactly
//!   ONE pointer anywhere in the image, at `0x4882f1`, inside the table's initializer — and the
//!   record it is written into is `0xb4d7d4`, which is `0xb4b498 + 451 × 20` on the nose.
//! - **Its surface is the chat window, not the red toast.** The registrar `0x488410` stores its
//!   second argument at `[rec+0x04]`, and that call site pushes **`0`** — the arm `0x496822`, which
//!   ends `call 0x49a870` with `edx = [rec+0x10]`. That field is `esi` throughout the initializer,
//!   loaded once at `0x484cac` as **`0xA` = `CHAT_MSG_SYSTEM`**. (Its neighbours in the same block
//!   — `ERR_MAIL_REACHED_CAP`, `ERR_INVALID_RAID_TARGET` — push `2`, the red line, which is what
//!   makes the `0` here a deliberate difference rather than a default.)
//!
//! So a trial account's whisper cap prints "Trial accounts cannot send unlimited tells…" into chat.
//! It is a dead letter against vmangos, which has no trial accounts and never sends it — but the
//! reference is the spec, and an opcode we drop is one we cannot notice arriving.

use bevy::prelude::*;

use benilla_assets::{LockRecover, WorldAssets};
use benilla_formats::{chat_channel_flags as chan_flags, ServerMessagesCatalog};

use crate::area::AreaTableRes;

use super::edit::ChannelState;
use super::event::{ChatEvent, ChatEventKind};
use super::feed::ChatLog;

/// `ServerMessages.dbc`, read once at Startup — the five shutdown/restart sentences.
#[derive(Resource)]
pub(crate) struct ServerMessages(pub(crate) ServerMessagesCatalog);

/// One parked world broadcast, awaiting the catalogs and the joined-channel walk.
///
/// Kept as the *wire* facts rather than a composed line for the reason the module doc gives: the
/// resolve needs resources the packet arm does not hold, and half-resolving at the arm would put
/// the AreaTable lookup in one place and the channel walk in another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Broadcast {
    /// `SMSG_ZONE_UNDER_ATTACK` — the `AreaTable.dbc` id of the area under attack.
    ZoneUnderAttack { area_id: u32 },
    /// `SMSG_DEFENSE_MESSAGE` — the zone the broadcast is about, and the server's own text.
    Defense { zone_id: u32, text: String },
    /// `SMSG_SERVER_MESSAGE` — the `ServerMessages.dbc` row id and the text filling its `%s`.
    Server { message_type: u32, text: String },
    /// `SMSG_CHAT_RESTRICTED` — no payload at all. Parked here rather than on
    /// [`crate::ui_action::UiErrorKeys`] because its registry entry names the **chat** surface, and
    /// that queue's drain only knows the two `UIErrorsFrame` arms; the key still resolves through
    /// the VM's own `GlobalStrings.lua`, like every other `DisplayError` tenant.
    ChatRestricted,
}

/// Load `ServerMessages.dbc`. `.after(benilla_assets::AssetSet::Open)` at the call site is
/// load-bearing for the reason [`super::channels::load_chat_channels`] records: without it this
/// runs before the patch chain exists, takes its `None` arm and silently loads nothing.
pub(super) fn load_server_messages(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_server_messages_catalog(&mut chain)
    };
    match loaded {
        // Logged like its two neighbours, and for their reason: a silently-empty load is the exact
        // failure `load_chat_channels`'s comment describes, and a count in the run log is the only
        // thing that distinguishes it from a healthy one.
        Ok(cat) => {
            info!("chat: {} ServerMessages rows", cat.len());
            commands.insert_resource(ServerMessages(cat));
        }
        Err(e) => warn!("chat: ServerMessages.dbc failed to load: {e:#}"),
    }
}

/// The joined channels a defense broadcast about `subject_zone` reaches, in slot order.
///
/// The reference's per-entry test, transcribed (module doc): live + joined, a `ChatChannels.dbc`
/// row carrying `DEFENSE`, and — only for a `ZONE_DEP` row — the zone match. `player_zone` is
/// `None` before the world under the player has streamed, which fails the zone-dependent arm and
/// leaves the global one (WorldDefense) intact: the same split the reference gets from a
/// not-yet-written `[0xb6e5d0]`.
///
/// benilla's joined list holds names, not records, so `+0x9c`'s join state has no representative
/// here — an entry in [`ChannelState::joined`] is one the server confirmed with YOU_JOINED, which
/// is the state the reference's `== 0` test selects. The suspended state (`3`) is unmodeled on
/// both sides of that list (see [`super::feed::deliver`]'s note).
pub(crate) fn defense_targets(
    channels: &ChannelState,
    subject_zone: u32,
    player_zone: Option<u32>,
) -> Vec<String> {
    channels
        .iter_names()
        .filter(|name| {
            let Some(row) = channels.channels.row_for_name(name) else {
                return false; // a custom channel — no DBC row, no defense flag
            };
            if row.flags & chan_flags::DEFENSE == 0 {
                return false;
            }
            row.flags & chan_flags::ZONE_DEP == 0 || player_zone == Some(subject_zone)
        })
        .map(str::to_string)
        .collect()
}

/// The `ZONE_UNDER_ATTACK` line — FrameXML's own template with the area's name in its `%s`.
///
/// `replacen(.., 1)` because the reference's `snprintf` at `0x49dd26` takes **one** vararg: a
/// localization carrying a second `%s` would fill it with stack garbage there, and filling only the
/// first is the closest honest thing (the [`ServerMessagesCatalog::compose`] argument exactly).
///
/// The template is `"|cffffff00%s is under attack!|r"` in the shipped enUS strings, but it is never
/// quoted here: the engine reads it out of the VM's globals (`FrameScript_GetText`, `0x49dd14`),
/// which is what keeps a localized install localized.
fn zone_under_attack_line(template: &str, area_name: &str) -> String {
    template.replacen("%s", area_name, 1)
}

/// The parent zone a broadcast's area id resolves to — `[rec+0x8]` when nonzero, else the id
/// itself (`0x49dd2b`/`0x49de80`). `None` when the table has no such row, which the two callers
/// answer differently (module doc).
fn parent_zone(areas: &AreaTableRes, id: u32) -> Option<u32> {
    let row = areas.0.get(id)?;
    Some(if row.zone_id == 0 { id } else { row.zone_id })
}

/// Resolve every parked broadcast into chat events. Runs before [`super::feed::feed_chat`] so a
/// broadcast lands the same frame it decodes, like every other chat source.
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
    // The `ZONE_UNDER_ATTACK` format string is FrameXML's, read out of the VM's own globals — so
    // there is nothing to resolve until the VM exists. Park rather than drop: the queue is cleared
    // at session end like the rest of the log.
    let Some(script) = script else { return };
    // The zone under the player — the reference's `[0xb6e5d0]`. `top_zone` is `crate::area`'s and
    // `channels`' own answer to "which zone am I in", and equals the single-hop parent on 5875 data.
    let player_zone = areas
        .as_ref()
        .and_then(|a| world.area().and_then(|leaf| a.0.top_zone(leaf)));
    for item in log.take_broadcasts() {
        match item {
            Broadcast::ZoneUnderAttack { area_id } => {
                // No catalog and no row are the same refusal here: with no name there is no line.
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
                // The system line is written out here rather than routed through
                // `ui_action::show_messages`, and that is not an oversight: of this feed's four
                // arms only this one is a `DisplayError` tenant at all — the two defence
                // broadcasts are engine-composed CHANNEL lines and `SMSG_SERVER_MESSAGE` composes
                // from `ServerMessages.dbc`, so three of the four have no message record to read a
                // surface from. The claim that this one's record says "chat" is checked instead,
                // in `chat_restricted_is_a_chat_row` below.
                //
                // An absent key shows nothing — this crate's uniform GlobalStrings stance, and a
                // deliberate half-step short of the reference: `0x496720` suppresses on the message
                // record's own key field being NULL or empty (`0x4967c5`), but `FrameScript_GetText`
                // returns `""` rather than NULL for a missing global (`0x703c02` seeds it), so a
                // broken `GlobalStrings.lua` would make the
                // reference print an EMPTY line here rather than none. Showing nothing is what every
                // other tenant of this route does; diverging in one module would be the
                // inconsistency.
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

/// One `CHAT_MSG_CHANNEL` line per surviving defense channel — the reference's loop body.
///
/// `sender` and `language` stay empty: the handlers push NULL for both (`0x49ddf5`/`0x49ddf7`), so
/// arg2 and arg3 are `""` and `ChatFrame_OnEvent` renders `[LocalDefense] : <text>` — the bare
/// colon is the reference's own output for a senderless channel line, not a gap here.
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
    /// `ERR_CHAT_RESTRICTED` (id 451) carries kind 0, so the arm above is right to write a chat
    /// system line and not a red toast — the one hand-set surface left in this feed, checked
    /// rather than trusted (decision 1770).
    #[test]
    fn chat_restricted_is_a_chat_row() {
        use benilla_ui::messages::{by_key, kind_of, MsgKind};
        assert_eq!(by_key("ERR_CHAT_RESTRICTED").expect("a real row").id, 451);
        assert_eq!(kind_of("ERR_CHAT_RESTRICTED"), MsgKind::Chat);
    }

    use super::*;
    use benilla_formats::{ChatChannelRow, ChatChannelsCatalog};

    /// The three shipped rows that matter here, as the real DBC carries them (asserted against the
    /// file by `benilla_formats::chat_channels`'s own test).
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

    /// The whole selection in one: General is skipped (no DEFENSE bit), WorldDefense always fires,
    /// LocalDefense only when the broadcast is about the zone the player is standing in.
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
        // Standing in Elwynn: the local channel we hold is Elwynn's, so a Westfall alarm reaches
        // only the global one.
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

    /// Joined nothing that carries the flag ⇒ the broadcast is silent. This is the faithful
    /// answer, and the reason a "why did nothing print?" report is not automatically a bug.
    #[test]
    fn a_player_in_no_defense_channel_hears_nothing() {
        let s = state(&["General - Westfall"]);
        assert!(defense_targets(&s, WESTFALL, Some(WESTFALL)).is_empty());
    }

    /// A custom channel has no DBC row, so it can never carry the flag — and asking must not
    /// panic on the missing row.
    #[test]
    fn a_custom_channel_is_never_a_defense_channel() {
        let s = state(&["mydefense"]);
        assert!(defense_targets(&s, WESTFALL, Some(WESTFALL)).is_empty());
    }

    /// Before the world under the player has streamed there is no zone to compare, which fails the
    /// zone-dependent arm and leaves the global one — not a dropped broadcast.
    #[test]
    fn an_unknown_player_zone_still_reaches_world_defense() {
        let s = state(&["LocalDefense - Westfall", "WorldDefense"]);
        assert_eq!(defense_targets(&s, WESTFALL, None), vec!["WorldDefense"]);
    }

    /// Freed slots are holes in the list ([`ChannelState::free_slot`] clears in place so the
    /// numbers above do not move); the walk skips them rather than stopping at the first.
    #[test]
    fn a_freed_slot_does_not_end_the_walk() {
        let mut s = state(&["LocalDefense - Westfall", "WorldDefense"]);
        s.joined[0] = None;
        assert_eq!(
            defense_targets(&s, WESTFALL, Some(WESTFALL)),
            vec!["WorldDefense"]
        );
    }

    /// The template's `%s` takes the **area**'s name, not the zone's — the subject is Sentinel Hill
    /// even though the channel test ran against Westfall. The color codes ride through untouched:
    /// they are the reference's own, and the renderer eats them downstream.
    #[test]
    fn the_line_fills_the_frame_xml_template_with_the_area_name() {
        assert_eq!(
            zone_under_attack_line("|cffffff00%s is under attack!|r", "Sentinel Hill"),
            "|cffffff00Sentinel Hill is under attack!|r"
        );
    }

    /// **The rendered line has a bare colon, and that is the reference's output, not a gap.**
    /// `ChatFrame_OnEvent` (l.1449-1452) formats `CHAT_CHANNEL_GET` = `"%s: "` with `pflag..arg2`,
    /// and the two handlers push a NULL sender — so the `%s` is empty and the colon is left
    /// hanging. Pinned here because it looks exactly like a bug, and re-"fixing" it would be a
    /// divergence.
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
