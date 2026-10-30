//! The `AUTO_JOIN_GUILD_CHANNEL` cascade (`0x49ea90`), the one place the client joins or leaves
//! `GuildRecruitment - City` on its own. On AUTO (`[0x843608]`, the default) a guilded player
//! leaves it (`0x49ee70`) and an unguilded one in a capital joins it (`0x49eb70`), with no
//! membership check; otherwise it arms the retry `[0xb6e5e4]`, which the zone walk's tail spends
//! (`0x49a6a4`-`0x49a6b2`). Once joined, the zone walk owns it as it owns Trade. It runs on the
//! plan the host resolves as it runs ([`RecruitmentLive`]), where the reference runs it:
//! `SetGuildRecruitmentMode(1)` inside the verb (`0x49ea7b`), our create's `PostInit` (`0x5dec1e`)
//! and the `PLAYER_GUILDID` watcher (`0x5e279a`) inside the update packet, each behind the guild
//! tag ([`crate::field_watch::FieldNotice::RecruitmentCascade`]), and the walk's tail.
//! Deviation: no re-run when a remote player streams in, because the reference's re-run only
//! repeats the join, which vmangos swallows, or the leave, whose "Not on channel" notice nothing
//! prints.

use bevy::prelude::*;

use benilla_ui::script::RecruitmentPlan;

use crate::area::AreaTableRes;
use crate::net::{ObjectStore, SelfPlayer};

use super::channels::{city_word, AREA_FLAG_CAPITAL};
use super::edit::ChannelState;

/// The cascade's retry: the one-shot `[0xb6e5e4]` a run that cannot act arms (`0x49eaf5`,
/// `0x49eb33`) and the walk's tail spends (`0x49a6ac`), engine state that outlives a reload.
#[derive(Resource, Default)]
pub(crate) struct GuildRecruitmentCascade {
    pending: bool,
}

impl GuildRecruitmentCascade {
    /// One run of `0x49ea90`, with the host lent: nothing off AUTO (`0x49ea96`), else the run on
    /// the plan as it stands, which arms the retry when it cannot act.
    pub(crate) fn run(&mut self, script: &mut benilla_ui::script::UiScript) {
        if script.guild_recruitment_mode() == 1 && !script.run_guild_recruitment_cascade() {
            self.pending = true;
        }
    }

    /// The walk's tail (`0x49a6a4`-`0x49a6b2`): a retry armed, by a run or by a
    /// `SetGuildRecruitmentMode(1)` whose cascade could not act, is cleared and run.
    pub(crate) fn tail(&mut self, script: &mut benilla_ui::script::UiScript) {
        let owed = script.take_guild_recruitment_cascade();
        if std::mem::take(&mut self.pending) || owed {
            self.run(script);
        }
    }

    /// Session end: forget the retry.
    pub(super) fn clear_session(&mut self) {
        self.pending = false;
    }

    #[cfg(test)]
    pub(crate) fn arm_for_test(&mut self) {
        self.pending = true;
    }

    #[cfg(test)]
    pub(crate) fn armed_for_test(&self) -> bool {
        self.pending
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

/// The plan `0x49ea90` acts on, the latch taken as AUTO: the verdict, then for an acting verdict
/// the name, composed against the "City" row whatever the zone (`0x49f140`).
fn plan(
    guild_id: u32,
    channels: &ChannelState,
    zone: Option<u32>,
    areas: Option<&AreaTableRes>,
) -> RecruitmentPlan {
    // Wait for the chat-cache restore, which seats the latch and the mask.
    if channels.zone_mask.is_none() {
        return RecruitmentPlan::Retry;
    }
    // The zone row: unresolvable ⇒ the retry stays armed (`0x49eaf5`).
    let Some(areas) = areas else {
        return RecruitmentPlan::Retry;
    };
    let Some(row) = zone.and_then(|id| areas.0.get(id)) else {
        return RecruitmentPlan::Retry;
    };
    let action = cascade(true, guild_id, row.flags & AREA_FLAG_CAPITAL != 0);
    if action == Some(Cascade::Deferred) {
        return RecruitmentPlan::Retry; // `0x49eb33`: the retry stays armed
    }
    // With no row or city word there is no name to send; the run is consumed, not retried.
    let target = channels
        .channels
        .rows()
        .iter()
        .find(|r| r.is_guild_recruitment())
        .and_then(|row| {
            let city = city_word(&areas.0).filter(|w| !w.is_empty())?;
            Some((row, row.joinable_name(city)))
        });
    let Some((row, name)) = target else {
        return RecruitmentPlan::Unnamed;
    };
    match action {
        Some(Cascade::Leave) => RecruitmentPlan::Leave(name),
        _ => RecruitmentPlan::Join {
            name,
            shortcut: row.shortcut.clone(),
            id: row.id,
        },
    }
}

/// The cascade's reads as it runs, the host's half of [`benilla_ui::script::RecruitmentPlanQuery`]:
/// our guild id (`0x49eb01`), the zone row (`0x49ead7`) and the channels, as they stand. Absent in
/// a harness without chat.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct RecruitmentLive<'w, 's> {
    me: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
    channels: Option<Res<'w, ChannelState>>,
    committed: Option<Res<'w, crate::area::CommittedZone>>,
    areas: Option<Res<'w, AreaTableRes>>,
}

impl crate::script_calls::live::LiveFamily for RecruitmentLive<'_, '_> {
    fn answer(&mut self, ask: &mut benilla_ui::script::live::LiveAsk<'_>) -> bool {
        ask.answer(|_: &benilla_ui::script::RecruitmentPlanQuery| {
            let (channels, committed) = (self.channels.as_deref()?, self.committed.as_deref()?);
            // With no player object the cascade returns owing nothing (`0x49eac3`).
            Some(match self.me.iter().next() {
                Some(store) => plan(
                    store.0.player_guild_id(),
                    channels,
                    committed.0,
                    self.areas.as_deref(),
                ),
                None => RecruitmentPlan::NoPlayer,
            })
        })
    }
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

    /// The cascade's leave (`0x49eb17` → `0x49ee70`) clears the mask bit (`0x49f10a`), sends, and
    /// strips no window: the strip matches the full name verbatim (`0x49f017`), which none holds.
    #[test]
    fn the_cascades_leave_keeps_the_windows_entry() {
        use benilla_ui::script::UiScript;
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
        channels.claim_slot("GuildRecruitment - City", GUILD_RECRUITMENT);
        let (tx, rx) = crossbeam_channel::unbounded();

        let packet = super::super::input::apply_channel_command(
            benilla_ui::script::ChannelCommand::Leave {
                name: "GuildRecruitment - City".into(),
            },
            &mut channels,
            &mut script,
            &super::super::ChatLog::default(),
            &mut |_, key| panic!("a leave refuses nothing: {key}"),
        );
        if let Some(packet) = packet {
            let _ = tx.send(packet);
        }

        assert!(
            matches!(rx.try_recv(), Ok(crate::net::ClientCommand::LeaveChannel { name }) if name == "GuildRecruitment - City"),
            "the CMSG_LEAVE_CHANNEL goes out"
        );
        assert_eq!(channels.zone_mask, Some(0), "the mask bit clears");
        assert_eq!(
            script.chat_window_looks()[0].channels,
            vec![("GuildRecruitment".to_string(), GUILD_RECRUITMENT)],
            "window 1 still carries the entry the join registered"
        );
        assert_eq!(
            script.guild_recruitment_mode(),
            1,
            "the cascade passes no reset flag"
        );
    }
}
