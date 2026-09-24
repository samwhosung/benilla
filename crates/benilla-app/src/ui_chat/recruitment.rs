//! The `AUTO_JOIN_GUILD_CHANNEL` cascade — the reference's `0x49ea90`, the thing
//! `SetGuildRecruitmentMode(1)` tail-jumps into and the one place the client joins or leaves
//! `GuildRecruitment - City` on its own (decision 2144).
//!
//! **What it is for.** `GuildRecruitment` is the channel *unguilded* players sit in to be found.
//! Its `ChatChannels.dbc` row carries no `INITIAL` bit, so the zone walk never seeds it; instead
//! the option's latch (`[0x843608]`, `1` = AUTO, the shipped default) drives a cascade that reads
//! the local player's own `PLAYER_GUILDID` and acts on the wire:
//!
//! - **guilded** ⇒ leave-by-name `"GuildRecruitment - City"` (`0x49eb17` → `0x49ee70`):
//!   `CMSG_LEAVE_CHANNEL` and its `ZONECHANNELS` bit cleared, then `UPDATE_CHAT_WINDOWS`. **No
//!   already-a-member check.** The window entry the join registered **stays**: the leave's strip
//!   key is the full name, which no window carries (`0x49f017`).
//! - **unguilded, in a capital** (`AreaTable Flags & 0x100`) ⇒ join `"GuildRecruitment"`
//!   (`0x49eb55` → `0x49eb70`): a slot, the composed name into chat window 1's list,
//!   `CMSG_JOIN_CHANNEL`, then `UPDATE_CHAT_WINDOWS`. **No already-joined check.**
//! - **otherwise** — no resolvable zone row, or unguilded outside a capital ⇒ the one-shot
//!   `[0xb6e5e4]` is armed and `ZoneChannelRefresh`'s tail consumes it on the next walk
//!   (`0x49a6a4`–`0x49a6b2`).
//!
//! Once the join confirms, the server's `YOU_JOINED` ORs bit 24 into the mask (`0x49bbaf`), and
//! from then on **the zone walk owns the channel exactly like Trade** — registered before the
//! gate, suspended outside a capital, re-joined on the way back in through the state-3 bypass
//! ([`super::channels::plan_walk`]). The cascade is the *entry*, not the upkeep.
//!
//! **Triggers** (`0x49ea90`'s closed caller census): `SetGuildRecruitmentMode(1)`; the
//! chat-cache loader seating `AUTO`; the local player's `PLAYER_GUILDID` field-change watcher
//! (`0x5e2770`); and every `CGPlayer` create (`0x5dec1e`) — which includes the local player's own
//! at login. Ours: the Lua verb's ask, and a watcher over the local player's guild id whose first
//! sight of it *is* the create. The loader's call is subsumed (the watcher fires after the seat,
//! never before, because the walk's zone — which the cascade waits on — resolves only after the
//! world-entry load). **The per-*remote*-player re-fire is deliberately not modelled**: on the
//! bytes a guilded player re-sends the LEAVE every time another player streams in, which vmangos
//! answers "Not on channel" — a notice the stock `ChatFrame_OnEvent` then drops, because no slot
//! and no window carries the channel (`found == 0`). Invisible on screen, a packet per player on
//! the wire, and a quirk of where the hook sits rather than a behaviour; recorded in 2144.
//!
//! **Retry, generalised.** The reference arms a one-shot and the walk's tail consumes it on the
//! next zone change. Ours keeps the request pending and re-evaluates on every frame the zone
//! resolves; the observable is the same — it acts on the first zone change into a capital — and
//! it does not depend on the walk happening to run.

use bevy::prelude::*;

use benilla_ui::script::UiScript;

use crate::area::AreaTableRes;
use crate::net::{ClientCommand, NetCommands, ObjectStore, SelfPlayer};

use super::channels::{city_word, ZoneChannelWalk};
use super::edit::ChannelState;

/// `AreaTable.dbc` `Flags & 0x100` — vmangos `AREA_FLAG_CAPITAL`. The cascade's own capital test
/// (`0x49eb2e test ch,1`): a *different* bit from the walk's eligibility gate (`0x8`), read here
/// as its own flag because the client reads it as one. In the 1.12.1 data both sit on exactly the
/// same six rows.
const AREA_FLAG_CAPITAL: u32 = 0x100;

/// The cascade's own state: whether a run is owed, and the guild id its watcher last saw.
#[derive(Resource, Default)]
pub(crate) struct GuildRecruitmentCascade {
    /// A run is owed — the reference's `[0xb6e5e4]` one-shot and the tail-jump out of `0x49ea70`,
    /// folded into one flag: set by every trigger, cleared when the cascade acts or finds the latch
    /// off, **kept** while it cannot act (module doc, "Retry, generalised").
    pending: bool,
    /// The local player's `PLAYER_GUILDID` as last observed — the field-change watcher
    /// `0x5e2770`. `None` until the player streams in, so the first observation is a change: that
    /// is the reference's player-create trigger (`0x5dec1e`), which is what runs the cascade at
    /// login.
    last_guild_id: Option<u32>,
}

impl GuildRecruitmentCascade {
    /// A trigger fired: `SetGuildRecruitmentMode(1)`, or the watcher.
    pub(super) fn request(&mut self) {
        self.pending = true;
    }

    /// The watcher: note the local player's guild id, answering whether it moved (the first sight
    /// counts). A move requests a run.
    pub(super) fn observe_guild_id(&mut self, guild_id: u32) -> bool {
        if self.last_guild_id == Some(guild_id) {
            return false;
        }
        self.last_guild_id = Some(guild_id);
        self.pending = true;
        true
    }

    /// Session end: the next login's first sight of the guild id must be a change again, and a
    /// run owed to a character who logged out is not owed to the next one.
    pub(super) fn clear_session(&mut self) {
        self.pending = false;
        self.last_guild_id = None;
    }
}

/// What `0x49ea90` does once the player and the zone row have resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cascade {
    /// Guilded: leave `GuildRecruitment - City` (`0x49eb0e`–`0x49eb2b`).
    Leave,
    /// Unguilded, in a capital: join `GuildRecruitment` (`0x49eb2b`–`0x49eb64`).
    Join,
    /// Unguilded, not in a capital: arm the retry (`0x49eb33`), no packet.
    Deferred,
}

/// `0x49ea90`'s verdict, pure. `None` = the latch is not AUTO (`0x49ea96`): nothing at all, the
/// retry not even armed. The guild test comes **before** the capital test (`0x49eb0c`), so a
/// guilded player leaves from anywhere.
pub(crate) fn cascade(auto: bool, guild_id: u32, in_capital: bool) -> Option<Cascade> {
    if !auto {
        return None;
    }
    if guild_id != 0 {
        return Some(Cascade::Leave);
    }
    Some(if in_capital {
        Cascade::Join
    } else {
        Cascade::Deferred
    })
}

/// The cascade as a system — chained **after** the walk, which is where the reference runs it
/// (the tail of `ZoneChannelRefresh`) and where the zone it reads has just been resolved.
pub(super) fn guild_recruitment_cascade(
    script: Option<NonSendMut<UiScript>>,
    mut state: ResMut<GuildRecruitmentCascade>,
    mut channels: ResMut<ChannelState>,
    walk: Res<ZoneChannelWalk>,
    areas: Option<Res<AreaTableRes>>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else { return };
    // Trigger: `SetGuildRecruitmentMode(1)` — `0x49ea70`'s tail-jump, whether or not the value
    // moved (the reference's store is unconditional and the jump keys on the new value alone).
    if script.take_guild_recruitment_cascade() {
        state.request();
    }
    // Trigger: the local player's guild id, first sight included. Only while the avatar is
    // streamed — a frame between the logout despawn and the teardown must not read as "left the
    // guild" (`ui_guild::feed` keeps the same rule).
    let Some(guild_id) = self_q.iter().next().map(|s| s.0.player_guild_id()) else {
        return;
    };
    state.observe_guild_id(guild_id);
    if !state.pending {
        return;
    }
    // The latch and the mask are seated by the same chat-cache restore; before it, the latch is
    // the boot value and not the character's. The walk holds on the same condition.
    if channels.zone_mask.is_none() {
        return;
    }
    // The zone row: unresolvable ⇒ the retry stays armed (`0x49eaf5`).
    let Some(areas) = areas.as_deref() else {
        return;
    };
    let Some(row) = walk.zone_id.and_then(|id| areas.0.get(id)) else {
        return;
    };
    let auto = script.guild_recruitment_mode() == 1;
    let verdict = cascade(auto, guild_id, row.flags & AREA_FLAG_CAPITAL != 0);
    let Some(action) = verdict else {
        state.pending = false; // STANDARD: nothing, not even the retry
        return;
    };
    if action == Cascade::Deferred {
        return; // `0x49eb33`: the retry stays armed
    }
    // The row and its composed name: `0x49f140` composes `Name_lang` against the `"City"` row
    // regardless of the zone (the join side is handed the bare shortcut and `0x49eb70` composes
    // it again the same way). No such row, or no city word (the walk's own guard against a
    // half-formed name): nothing to name, consumed rather than retried.
    let target = channels
        .channels
        .rows()
        .iter()
        .find(|r| r.is_guild_recruitment())
        .cloned()
        .and_then(|row| {
            let name = row.joinable_name("", city_word(&areas.0)?);
            Some((row, name))
        });
    let Some((row, name)) = target else {
        state.pending = false;
        return;
    };
    match action {
        Cascade::Leave => {
            info!("chat: guild recruitment cascade — guilded, leaving {name:?}");
            cascade_leave(&mut channels, &commands, name);
        }
        Cascade::Join => {
            info!("chat: guild recruitment cascade — unguilded in a capital, joining {name:?}");
            // `0x49eb70`: the slot (`0x49b980`, de-duplicated by name), window 1's list
            // (`frameIdx = 0`), then the send — unconditional, so a repeat re-sends.
            match channels.claim_slot(&name) {
                Some(n) => {
                    debug!("chat: {name:?} registered as slot {n}");
                    script.set_joined_channels(channels.names());
                }
                None => warn!(
                    "chat: no free slot for {name:?} — all {} are taken",
                    super::edit::MAX_CHANNELS
                ),
            }
            script.register_chat_window_channel(0, &row.shortcut, row.id);
            let _ = commands.0.send(ClientCommand::JoinChannel {
                name,
                password: String::new(),
            });
        }
        Cascade::Deferred => {}
    }
    // Both acting arms fire it (`0x49eb21`, `0x49eb5f`): the stock `ChatFrame_OnEvent` re-reads
    // every window's channel list on this event and nothing else does.
    script.fire_event("UPDATE_CHAT_WINDOWS", vec![]);
    state.pending = false;
}

/// The cascade's LEAVE arm — leave-by-name `0x49ee70` over the composed name
/// `"GuildRecruitment - City"`: the packet and the mask bit (`0x49f10a`/`0x49f11a`).
///
/// **No window strip.** `0x49ee70` does strip every window's list, but keyed on the DBC Shortcut
/// only when its *argument* matched a shortcut — else on the argument verbatim (`0x49eff6` /
/// `0x49f017`). The full name matches no shortcut, and the join arm registered window 1's entry
/// under the Shortcut `"GuildRecruitment"`, so the scan misses in every window and the entry
/// survives. Stripping by the Shortcut here removed an entry the reference keeps.
fn cascade_leave(channels: &mut ChannelState, commands: &NetCommands, name: String) {
    let _ = commands
        .0
        .send(ClientCommand::LeaveChannel { name: name.clone() });
    channels.note_zone_channel_left(&name);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four legs of `0x49ea90`, and the order of its two tests.
    #[test]
    fn the_cascade_reads_the_latch_then_the_guild_then_the_capital() {
        assert_eq!(
            cascade(false, 0, true),
            None,
            "STANDARD: nothing, not even the retry"
        );
        assert_eq!(cascade(false, 7, true), None);
        assert_eq!(
            cascade(true, 7, false),
            Some(Cascade::Leave),
            "guilded leaves from anywhere — the guild test precedes the capital test"
        );
        assert_eq!(cascade(true, 7, true), Some(Cascade::Leave));
        assert_eq!(cascade(true, 0, true), Some(Cascade::Join));
        assert_eq!(cascade(true, 0, false), Some(Cascade::Deferred));
    }

    /// **The cascade's own leave cannot strip its window entry** (`0x49f017`).
    /// The join arm registered window 1's entry
    /// under the Shortcut `"GuildRecruitment"`; the leave's strip key is its argument, the full
    /// `"GuildRecruitment - City"`, which matches no DBC shortcut and so is used verbatim — and
    /// equals no window entry. The packet goes out and the mask bit clears; the entry stays.
    #[test]
    fn the_cascades_leave_keeps_the_windows_entry() {
        const GUILD_RECRUITMENT: u32 = 25;
        let mut script = UiScript::new().expect("a VM");
        assert!(script.register_chat_window_channel(0, "GuildRecruitment", GUILD_RECRUITMENT));
        let mut channels = ChannelState {
            channels: benilla_formats::ChatChannelsCatalog::from_rows(vec![
                benilla_formats::ChatChannelRow {
                    id: GUILD_RECRUITMENT,
                    flags: 0x32,
                    pattern: "GuildRecruitment - %s".into(),
                    shortcut: "GuildRecruitment".into(),
                },
            ]),
            zone_mask: Some(1 << (GUILD_RECRUITMENT - 1)),
            ..Default::default()
        };
        channels.claim_slot("GuildRecruitment - City");
        let (tx, rx) = crossbeam_channel::unbounded();

        cascade_leave(
            &mut channels,
            &NetCommands(tx),
            "GuildRecruitment - City".into(),
        );

        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::LeaveChannel { name }) if name == "GuildRecruitment - City"),
            "the CMSG_LEAVE_CHANNEL goes out"
        );
        assert_eq!(channels.zone_mask, Some(0), "the mask bit clears");
        assert_eq!(
            script.chat_window_looks()[0].channels,
            vec![("GuildRecruitment".to_string(), GUILD_RECRUITMENT)],
            "window 1 still carries the entry the join registered"
        );
    }

    /// The watcher: the first sight of the guild id is a change (the player-create trigger), a
    /// repeat is not, a move is, and the session end forgets — so the next login's first sight is
    /// a change again.
    #[test]
    fn the_watcher_fires_on_first_sight_and_on_change() {
        let mut c = GuildRecruitmentCascade::default();
        assert!(!c.pending);
        assert!(c.observe_guild_id(0), "first sight, even of 'no guild'");
        assert!(c.pending);
        c.pending = false;
        assert!(!c.observe_guild_id(0), "unchanged");
        assert!(!c.pending);
        assert!(c.observe_guild_id(42), "joined a guild");
        assert!(c.pending);
        c.clear_session();
        assert!(!c.pending);
        assert!(c.observe_guild_id(42), "a new session sees it fresh");
    }
}
