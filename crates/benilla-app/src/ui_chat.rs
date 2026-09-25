//! The chat system: [`event`] is the internal `CHAT_MSG_*` currency, [`feed`] turns every source
//! into events with names resolved once, [`frames`] routes them through the `ChatFrame_OnEvent`
//! composer to the docked windows, and [`input`] is the outbound side over [`commands`]' table of
//! every `/command`, built from the reference's own alias strings.

use bevy::prelude::*;

use crate::ui_script::{UiFeed, UiInput};

#[cfg(test)]
mod ace_gate_tests;
/// `/afk` and `/dnd`: the optimistic AFK mirror `[0xb6e5cc]` and the implicit clear that every
/// other chat send and every movement press carries.
mod away;
/// The world broadcasts: `SMSG_ZONE_UNDER_ATTACK`/`_DEFENSE_MESSAGE`/`_SERVER_MESSAGE`.
mod broadcast;
mod channels;
/// The combat log's chat lines: classification, chat type and each sentence's GlobalString key.
pub(crate) mod combat;
pub(crate) mod commands;
/// The edit box's send path: the chat-type token passed to `SendChatMessage` and its wire kind.
/// `pub(crate)` for `ui_script::chat_tests`, which joins the stock file's token to this resolve.
pub(crate) mod edit;
mod event;
mod feed;
mod frames;
/// The idle handler: the 5-minute auto-sit and auto-AFK, and the 30-minute camp.
pub(crate) mod idle;
mod input;
/// The language gate: the exemptions and fluency lookup behind the chat garble.
mod language;
/// `LoggingChat`/`LoggingCombat`: the two log files `/chatlog` and `/combatlog` toggle.
mod logging;
mod net;
/// The `AUTO_JOIN_GUILD_CHANNEL` cascade, the one place the client joins or leaves
/// `GuildRecruitment - City` on its own.
mod recruitment;
/// The chat windows' saved look: tab tint, alpha and font size, read at login, saved at logout.
pub(crate) mod settings;
#[cfg(test)]
mod tests;

pub(crate) use away::AfkMirror;
pub(crate) use broadcast::Broadcast;
/// The zone-channel catalog's seed, called from the world-entry UI load: addons read the catalog
/// at file scope, before an `Update` push would land.
pub(crate) use channels::seed_zone_channel_catalog;
/// The joined-channel roster and the `ChatChannels.dbc` catalog.
pub(crate) use edit::ChannelState;
/// Test-only: `ui_script::chat_tests` checks every fired name against the live `ChatTypeInfo`.
#[cfg(test)]
pub(crate) use event::event_name;
pub(crate) use event::{default_color, ChatEvent, ChatEventKind};
pub(crate) use feed::ChatLog;
/// The per-character chat cache's restore, called from the world-entry UI load: its two events
/// must precede the session's first chat line and `PLAYER_LOGIN`.
pub(crate) use settings::restore_chat_looks;

pub(crate) struct UiChatPlugin;

impl Plugin for UiChatPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.add_observer(combat::on_cvar);
        app.init_resource::<ChatLog>()
            .init_resource::<away::AfkMirror>()
            .init_resource::<away::AfkMirrorMemo>()
            .init_resource::<idle::LastInput>()
            .init_resource::<frames::ChatWindows>()
            .init_resource::<edit::ChannelState>()
            .init_resource::<channels::ZoneChannelWalk>()
            .init_resource::<recruitment::GuildRecruitmentCascade>()
            .init_resource::<language::ChatLanguages>()
            .init_resource::<combat::CombatLogRanges>()
            // `combat::on_cvar` writes this and the range set: a missing one panics on the first
            // write, which only `scripts/smoke.sh` sees (the unit suites build their own worlds).
            .init_resource::<combat::LogPeriodicSpells>()
            // These DBC loads must follow `AssetSet::Open`: before it there is no chain, and each
            // silently loads nothing.
            .add_systems(
                Startup,
                (
                    channels::load_chat_channels,
                    feed::load_emote_texts,
                    broadcast::load_server_messages,
                )
                    .after(benilla_assets::AssetSet::Open),
            )
            // The slash-command table needs the VM's globals and the emote catalog, both `Startup`.
            .add_systems(PostStartup, commands::build_slash_commands)
            // The language gate's feeds, ahead of the chat drain that reads them.
            .add_systems(
                Update,
                (
                    language::load_language_words,
                    language::feed_language_skills,
                    language::feed_default_language,
                )
                    .before(feed::feed_chat),
            )
            // Descriptor-diff combat-log lines, ahead of the drain so none lands a frame late.
            .add_systems(
                Update,
                (
                    combat::watch::death_lines,
                    combat::watch::aura_lines,
                    combat::watch::pet_loyalty_lines,
                )
                    .before(feed::feed_chat),
            )
            // Before the drain, so a broadcast lands on the frame it decodes.
            .add_systems(
                Update,
                broadcast::feed_broadcasts
                    .in_set(crate::ui_script::UiFeed)
                    .before(feed::feed_chat),
            )
            // Only against the in-game VM: the drain takes the whole queue and fires every line,
            // so a VM with no ChatFrame destroys them. After the unit and guild feeds, as in the
            // reference: login-burst chat is queued (`0x49cae0`) behind the `[0x8435fc]` latch and
            // drains at `0x490974`, after `PLAYER_LOGIN` (`0x490959`) and `PLAYER_ENTERING_WORLD`
            // (`0x49096a`), while `GUILD_MOTD` fires on arrival (`0x5e7288`), so the guild line
            // precedes the welcome.
            .add_systems(
                Update,
                feed::feed_chat
                    .in_set(UiFeed)
                    .after(crate::ui_unit::UnitFeed)
                    .after(crate::ui_guild::GuildFeed)
                    .run_if(crate::ui_script::ingame_ui_up),
            )
            // The last-input stamp `[0xcf0bc8]`, ungated and ahead of the UI pass: the reference
            // stamps raw input before dispatch, so a key the chat box swallows still counts.
            .add_systems(Update, idle::stamp_input.before(UiInput))
            // A fresh VM's joined-channel mirror, before the feed so its first line is numbered.
            .add_systems(
                Update,
                channels::seed_channels
                    .in_set(crate::ui_script::UiFeed)
                    .before(feed::feed_chat),
            )
            // `RequestTimePlayed()` -> `CMSG_PLAYED_TIME`; `SMSG_PLAYED_TIME` -> `TIME_PLAYED_MSG`.
            .add_systems(Update, feed::played_time_bridge.in_set(UiFeed))
            .add_systems(
                Update,
                (
                    // First, so a `/afk` typed this frame reads the descriptor's settled state.
                    away::reconcile_afk_mirror,
                    away::movement_clears_afk,
                    // After the clears, so a press that stamps the clock and drops the flag is
                    // settled before the idle timer reads it.
                    idle::idle_handler,
                    input::drain_chat_input,
                    // After the box's drain, so a `SendChatMessage` or `SendAddonMessage` from a
                    // slash handler it just ran goes out this frame.
                    input::drain_addon_chat_sends,
                    input::drain_addon_message_sends,
                )
                    .chain()
                    .after(UiInput)
                    .in_set(crate::char_select::InWorldGated),
            )
            // The zone-channel auto-join, which vmangos leaves to the client. The disconnect clear
            // runs first, so a same-frame drop cannot leave the walk diffing stale membership.
            // After `AreaAuthoritySet`, since a stale area joins and leaves the wrong channels. The
            // guild-recruitment cascade follows the walk: it is the tail of `ZoneChannelRefresh`.
            .add_systems(
                Update,
                (
                    channels::end_session_channels_on_disconnect,
                    channels::auto_join_zone_channels.in_set(crate::char_select::InWorldGated),
                    recruitment::guild_recruitment_cascade.in_set(crate::char_select::InWorldGated),
                )
                    .chain()
                    .after(benilla_world::terrain_stream::AreaAuthoritySet),
            )
            .add_systems(
                OnExit(crate::char_select::ClientState::InWorld),
                (channels::end_session_channels, end_session_chat),
            );
        settings::plugin(app);
        logging::plugin(app);
    }
}

/// The chat session end: the undrained feed, which outlives the Lua state, and both windows'
/// lines. The Lua state is itself replaced at the session end, as the reference destroys its own
/// at logout.
///
/// Deviation: not run on benilla's same-character reconnect, which the reference lacks (it
/// returns to the login screen), so the window keeps its scrollback there.
fn end_session_chat(
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    mut log: ResMut<ChatLog>,
) {
    end_chat_session(script.map(NonSendMut::into_inner), &mut log);
}

/// [`end_session_chat`]'s body, callable without a `World`.
pub(crate) fn end_chat_session(
    script: Option<&mut benilla_ui::script::UiScript>,
    log: &mut ChatLog,
) {
    *log = ChatLog::default();
    if let Some(script) = script {
        for frame in ["ChatFrame1", "ChatFrame2"] {
            crate::ui_script::run_or_warn(script, &format!("{frame}:Clear()"));
        }
    }
}
