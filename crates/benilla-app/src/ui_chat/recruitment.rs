//! The `AUTO_JOIN_GUILD_CHANNEL` cascade (`0x49ea90`), the one place the client joins or leaves
//! `GuildRecruitment - City` on its own. On AUTO (`[0x843608]`, the default) a guilded player
//! leaves it (`0x49ee70`) and an unguilded one in a capital joins it (`0x49eb70`), with no
//! membership check; otherwise a retry waits for the next zone change. Once joined, the zone walk
//! owns it as it owns Trade. Triggers: `SetGuildRecruitmentMode(1)` and the `PLAYER_GUILDID`
//! watcher (`0x5e2770`), whose first sight is the player create (`0x5dec1e`). Deviation: no re-run
//! when a remote player streams in, because the reference's re-run only repeats the join, which
//! vmangos swallows, or the leave, whose "Not on channel" notice nothing prints.

use bevy::prelude::*;

use benilla_ui::script::UiScript;

use crate::area::AreaTableRes;
use crate::net::{ClientCommand, NetCommands, ObjectStore, SelfPlayer};

use super::channels::{city_word, ZoneChannelWalk};
use super::edit::ChannelState;

/// vmangos `AREA_FLAG_CAPITAL`, the cascade's capital test (`0x49eb2e`); the walk tests `0x8`,
/// which marks the same six rows in 1.12 data.
const AREA_FLAG_CAPITAL: u32 = 0x100;

/// The cascade's own state: whether a run is owed, and the guild id its watcher last saw.
#[derive(Resource, Default)]
pub(crate) struct GuildRecruitmentCascade {
    /// A run is owed: the one-shot `[0xb6e5e4]` and the tail-jump from `0x49ea70` in one flag,
    /// kept while the cascade cannot act.
    pending: bool,
    /// The last `PLAYER_GUILDID` seen; `None` makes the first sight a change, the create trigger.
    last_guild_id: Option<u32>,
}

impl GuildRecruitmentCascade {
    /// A trigger fired: `SetGuildRecruitmentMode(1)`, or the watcher.
    pub(super) fn request(&mut self) {
        self.pending = true;
    }

    /// The watcher: note the guild id, and request a run when it moved or is first seen.
    pub(super) fn observe_guild_id(&mut self, guild_id: u32) -> bool {
        if self.last_guild_id == Some(guild_id) {
            return false;
        }
        self.last_guild_id = Some(guild_id);
        self.pending = true;
        true
    }

    /// Session end: forget the owed run and the last guild id.
    pub(super) fn clear_session(&mut self) {
        self.pending = false;
        self.last_guild_id = None;
    }
}

/// What `0x49ea90` does once the player and the zone row have resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cascade {
    /// Guilded: leave `GuildRecruitment - City` (`0x49eb0e`-`0x49eb2b`).
    Leave,
    /// Unguilded, in a capital: join `GuildRecruitment` (`0x49eb2b`-`0x49eb64`).
    Join,
    /// Unguilded, not in a capital: arm the retry (`0x49eb33`), no packet.
    Deferred,
}

/// `0x49ea90`'s verdict: `None` when the latch is not AUTO (`0x49ea96`), not even a retry. The
/// guild test comes first (`0x49eb0c`), so a guilded player leaves from anywhere.
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

/// The cascade, chained after the walk: the reference runs it at the walk's tail, on a fresh zone.
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
    // `SetGuildRecruitmentMode(1)` runs it even when the value did not move (`0x49ea70`).
    if script.take_guild_recruitment_cascade() {
        state.request();
    }
    // Only while the avatar is streamed: a despawned body has not "left the guild".
    let Some(guild_id) = self_q.iter().next().map(|s| s.0.player_guild_id()) else {
        return;
    };
    state.observe_guild_id(guild_id);
    if !state.pending {
        return;
    }
    // Wait for the chat-cache restore, which seats the latch and the mask.
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
    // The name is composed against the "City" row whatever the zone (`0x49f140`); with no row or
    // city word the run is consumed, not retried.
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
            // `0x49eb70`: the slot (`0x49b980`, by name), window 1's list, then the send, always.
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
    // Both acting arms fire it (`0x49eb21`, `0x49eb5f`); chat frames re-read channels on it.
    script.fire_event("UPDATE_CHAT_WINDOWS", vec![]);
    state.pending = false;
}

/// Leave by the full name (`0x49ee70`): the packet and the mask bit (`0x49f10a`/`0x49f11a`). No
/// window loses its entry: the strip matches that name verbatim (`0x49f017`), which none holds.
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
