//! The chat system, the reference's own three-stage shape: [`event`] is the
//! internal currency (`CHAT_MSG_*` typed — kinds, groups, the shipped color table); [`feed`] turns
//! every source (wire lines, channel notices, rolls, client-composed loot/system lines) into
//! events with names resolved ask-once; [`frames`] is the router + the `ChatFrame_OnEvent`
//! composer fanning lines across the docked windows; [`input`] is the outbound side (the chat
//! EditBox, the slash grammar, the send-side emote posture gate) over [`commands`]'s table of every
//! `/command` the client answers (built from the reference's own alias strings).
//! This face wires them into [`UiChatPlugin`] and re-exports the crate-facing API (`ChatLog`,
//! `ChatEvent`, `ChatEventKind`).

use bevy::prelude::*;

use crate::ui_script::{UiFeed, UiInput};

#[cfg(test)]
mod ace_gate_tests;
/// `/afk` and `/dnd` — the two commands' asymmetric law, the optimistic AFK mirror `[0xb6e5cc]`,
/// and the implicit clear every other chat send and every movement press carries (2088).
mod away;
/// The world broadcasts (`SMSG_ZONE_UNDER_ATTACK`/`_DEFENSE_MESSAGE`/`_SERVER_MESSAGE`) — the
/// AreaTable/ServerMessages resolve and the joined-defense-channel walk they land on.
mod broadcast;
mod channels;
/// The combat log's chat lines — classification, chat type, and the GlobalString key each
/// combat packet's sentence is built from.
pub(crate) mod combat;
pub(crate) mod commands;
/// The chat edit box's send path — `SendType`, the chat-type TOKEN an addon or the stock
/// `SlashCmdList` passes to `SendChatMessage`, and its map to a wire kind. `pub(crate)` because
/// that token→wire seam is what `ui_script::chat_tests` spans: the stock file emitting the token
/// and this module resolving it are two halves that each pass alone while the join is cut (2082).
pub(crate) mod edit;
mod event;
mod feed;
mod frames;
/// The idle handler — the 5-minute auto-sit / auto-AFK and the 30-minute camp.
pub(crate) mod idle;
mod input;
/// The language gate — the exemptions and the fluency lookup behind the chat garble.
mod language;
/// `LoggingChat`/`LoggingCombat` — the two log files `/chatlog` and `/combatlog` toggle.
mod logging;
mod net;
/// The `AUTO_JOIN_GUILD_CHANNEL` cascade — the one place the client joins or
/// leaves `GuildRecruitment - City` on its own.
mod recruitment;
/// The chat windows' saved look — where the tab menu's tint/alpha/font-size
/// picks are read from at login and written back at logout.
pub(crate) mod settings;
#[cfg(test)]
mod tests;

pub(crate) use away::AfkMirror;
pub(crate) use broadcast::Broadcast;
/// The zone-channel catalog's seed. Called from the world-entry UI load, like the restore below
/// and for the neighbouring reason: the verbs that read it are read at addon file scope, and an
/// `Update` push lands after the whole burst.
pub(crate) use channels::seed_zone_channel_catalog;
/// The joined-channel roster + the `ChatChannels.dbc` catalog. Read outside this module by the
/// world-state readout ([`crate::world_state_ui`]), whose `Type == 1` gate is "has the player
/// joined a zone-dependent defense channel".
pub(crate) use edit::ChannelState;
/// Test-only: `ui_script::chat_tests` checks every name we fire against the live `ChatTypeInfo`
/// table, which lives on that side of the tree. The app itself calls it through `event::` — the
/// router is the only production caller and it is inside this module.
#[cfg(test)]
pub(crate) use event::event_name;
pub(crate) use event::{default_color, ChatEvent, ChatEventKind};
pub(crate) use feed::ChatLog;
/// The per-character chat cache's restore. Called from the world-entry UI load rather than from a
/// system, because the two events it fires have to precede the session's first chat line and
/// `PLAYER_LOGIN` — see [`settings::restore_chat_looks`] and.
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
            // `CombatLogPeriodicSpells`' knob, beside its sibling range set — both are what
            // `combat::on_cvar` writes, so a missing one is not a dormant default but a
            // PANIC on the first write of either row (2303; before that, at startup).
            // 947ba585f registered it only in the `cvar_app()` test helper, and the client
            // stopped booting; the unit suites never noticed because each builds its own
            // world. `scripts/smoke.sh` is the gate that sees this class.
            .init_resource::<combat::LogPeriodicSpells>()
            // `ChatChannels.dbc` — six rows, read once; the auto-join walk and every chat event's
            // arg7 both come out of it. **`.after(AssetSet::Open)` is load-bearing**: without it
            // this runs before the patch chain exists, takes its `assets: Option<Res<_>>` `None`
            // arm, and silently loads nothing — no zone channels, ever, with no error. A live
            // probe is what caught that; no unit test could, because the tests hand the catalog in.
            // `crate::area`'s `AreaTable.dbc` load carries the same ordering for the same reason.
            // `EmotesText.dbc` × `EmotesTextData.dbc` rides the same ordering for
            // the same reason: the sentence tables are read once, off the open patch chain.
            // `ServerMessages.dbc` (the five shutdown/restart sentences) rides the same ordering
            // for the same reason.
            .add_systems(
                Startup,
                (
                    channels::load_chat_channels,
                    feed::load_emote_texts,
                    broadcast::load_server_messages,
                )
                    .after(benilla_assets::AssetSet::Open),
            )
            // The slash-command table, built from the reference's own alias strings
            // once the VM's globals and the emote catalog exist — both are `Startup`, so this runs
            // at the next schedule rather than chasing an ordering constraint into two modules.
            .add_systems(PostStartup, commands::build_slash_commands)
            // Push before the input pass so a line is on screen the same frame it decodes (mirrors
            // the loot/merchant feeds).
            // The language gate's two feeds, both upstream of the chat drain that reads them: the
            // word pool loads once (it retries until the chain is up), and the fluency map
            // rebuilds off the spell book + the self descriptors.
            .add_systems(
                Update,
                (
                    language::load_language_words,
                    language::feed_language_skills,
                    language::feed_default_language,
                )
                    .before(feed::feed_chat),
            )
            // The three combat-log families that are NOT packet-driven (1703): the death reflex,
            // the aura arrival/departure/stack callbacks, and the pet-loyalty byte. Each is a
            // descriptor diff, so each runs before the drain that would otherwise show its line a
            // frame late.
            .add_systems(
                Update,
                (
                    combat::watch::death_lines,
                    combat::watch::aura_lines,
                    combat::watch::pet_loyalty_lines,
                )
                    .before(feed::feed_chat),
            )
            // The world broadcasts' resolve pass — before the drain that renders what it produces,
            // so an alarm or a shutdown countdown lands on the frame it decodes like every other
            // chat source.
            .add_systems(
                Update,
                broadcast::feed_broadcasts
                    .in_set(crate::ui_script::UiFeed)
                    .before(feed::feed_chat),
            )
            // **Never against the boot VM** (B376's second half). `feed_chat` takes the whole
            // queue with `mem::take` and fires each line as a real `CHAT_MSG_*` — so a drain
            // against a VM with no ChatFrame does not defer the lines, it DESTROYS them, with no
            // memo and no error. The window is the one frame `not(ingame_ui_pending)` cannot see
            // (`ui_script::ingame_ui_up`): `apply_net_updates` drains `Connected` and the login
            // burst behind it in one `try_iter` while the state — and with it 1978's park — is
            // still a frame away. Measured: one login in six drained the burst there and ate a
            // line of the realm's own welcome. The early return above `mem::take` is what holds
            // the queue for the frames after it, and this is what holds it for that one.
            //
            // **…and after the world-enter cascade, because that is the reference's own order**.
            // The real client does not print
            // login chat when it arrives either: `[0x8435fc]` is a latch that ships statically
            // `1`, so every `SMSG_MESSAGECHAT` in the login burst is queued into
            // `__AUPENDINGCHAT__` (`0x49db5c`/`0x49db62` → `0x49cae0`) instead of displayed. It is
            // cleared in exactly one place — `0x490974`, INSIDE the world-enter cascade, after
            // `PLAYER_LOGIN` (`0x490959`) and `PLAYER_ENTERING_WORLD` (`0x49096a`) — and the same
            // call drains the queue. `SMSG_GUILD_EVENT` 0x02 has no such latch: it fires
            // `GUILD_MOTD` synchronously in its own handler (`0x5e7288`). So the reference paints
            // the guild line FIRST and the realm's welcome lines after it, inverting the wire
            // order.
            //
            // Our held `ChatLog` queue IS that latch: the early return above `mem::take` is what
            // keeps the login burst waiting. This says when it drains. Without these two edges
            // the order was whatever the scheduler picked between two systems that declared
            // nothing about each other — right today, by luck, exactly as the drain above was
            // safe by luck until it wasn't.
            //
            // The edges are on the two feeds that fire the cascade's events, because that is
            // where `0x490974` sits — DOWNSTREAM of both `0x703e50` calls, not merely inside the
            // cascade. `UnitFeed` carries `PLAYER_ENTERING_WORLD`, `GuildFeed` carries
            // `GUILD_MOTD`. (The reference's drain is one contiguous walk — `0x49d230`, every
            // entry through the live path's own composer `0x49a870` — so the welcome lines land
            // as a block, after those events and before the rest of the cascade.)
            .add_systems(
                Update,
                feed::feed_chat
                    .in_set(UiFeed)
                    .after(crate::ui_unit::UnitFeed)
                    .after(crate::ui_guild::GuildFeed)
                    .run_if(crate::ui_script::ingame_ui_up),
            )
            // The last-input stamp `[0xcf0bc8]`. Deliberately NOT in-world-gated and
            // deliberately ahead of the UI pass: the reference stores it on the raw input bus,
            // before dispatch, so a keystroke the chat box swallows still counts as input.
            .add_systems(Update, idle::stamp_input.before(UiInput))
            // A fresh VM gets the joined-channel mirror re-pushed once — before
            // the feed, so the reload frame's first routed line already renders numbered.
            .add_systems(
                Update,
                channels::seed_channels
                    .in_set(crate::ui_script::UiFeed)
                    .before(feed::feed_chat),
            )
            // RequestTimePlayed() -> CMSG_PLAYED_TIME, and SMSG_PLAYED_TIME -> TIME_PLAYED_MSG.
            // Beside the chat feed because /played is a chat command and the answer prints there
            // too; before the input pass for the same reason feed_chat is.
            .add_systems(Update, feed::played_time_bridge.in_set(UiFeed))
            // The input: open on ENTER (after the UI input pass has set UiKeyboardCapture, so we
            // don't reopen the box that's already eating keys), then drain any submitted line. Both
            // touch the single NonSend VM, so they chain. In-world only: at the
            // character-select glue screen ENTER must not open a chat box behind the overlay.
            .add_systems(
                Update,
                (
                    // The AFK mirror's reconcile runs FIRST in the chain, so a `/afk` typed this
                    // frame reads the descriptor's settled state rather than racing it (2088).
                    away::reconcile_afk_mirror,
                    // …and the four movement clears, which share the CVar and the mirror.
                    away::movement_clears_afk,
                    // The idle handler (2092) — one timer, three legs. After the clears, so a
                    // press that both stamps the clock and drops the flag is settled before the
                    // timer that would otherwise re-raise it is read.
                    idle::idle_handler,
                    input::drain_chat_input,
                    // An addon's own line into the wire. AFTER the box's drain
                    // and in the same chain, so a `SendChatMessage` fired from a slash handler
                    // that the box's drain just ran goes out on the same frame.
                    input::drain_addon_chat_sends,
                    // …and its addon-lane twin. Same position in the chain, for
                    // the same reason: a `SendAddonMessage` fired from a handler that ran earlier
                    // in this chain goes out on this frame rather than the next.
                    input::drain_addon_message_sends,
                )
                    .chain()
                    .after(UiInput)
                    .in_set(crate::char_select::InWorldGated),
            )
            // The zone-channel auto-join (0288 P6): the client half of a handshake vmangos
            // deliberately leaves to us. In-world only, and it early-outs on an unchanged zone.
            //
            // Both session-end edges clear its state (1284): leaving the world at all, and a socket
            // drop that stays in-world for the reconnect. The disconnect twin is chained BEFORE the
            // walk so a drop and a walk landing on the same frame cannot re-diff against membership
            // the drop just invalidated.
            //
            // **After `AreaAuthoritySet`**, like the zone-text feed and the breath
            // classifier — the other two systems that act on the leaf area. The walk turns the zone
            // into packets, so reading last frame's answer is not a cosmetic lag: it joined the
            // previous character's capital at login and then left it again, and the leave took the
            // stock `ChatFrame_OnEvent`'s channel registration with it.
            //
            // The guild-recruitment cascade runs **after the walk**, where the
            // reference runs it — the tail of `ZoneChannelRefresh` — and reads the zone the walk
            // just published.
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
        // The per-character saved look — its own load/watch/save edges.
        settings::plugin(app);
        logging::plugin(app);
    }
}

/// **The chat module's session end** — what the reference gets for free by destroying its Lua
/// state, and we have to do by hand until that teardown lands (1288).
///
/// `shutdown_ui_state`'s own doc records the reference's logout tail: `PLAYER_LEAVING_WORLD` →
/// `PLAYER_LOGOUT` → the saved files → **destroy the Lua state**. That last step is the one
/// `crate::ui_script::IngameUiLoaded` exists to stand in for; while it does, every window in the
/// VM keeps its contents across a character switch. The director saw it as the previous
/// character's `Joined Channel:` lines still sitting under the new character's — chat scrollback
/// is simply the most visible tenant of a VM that should have been rebuilt.
///
/// So this clears what a fresh VM (and a fresh `ui_chat`) would have: both windows' lines, the
/// edit box's whole cross-open memory — sticky type, the `lastTell` ring, `toldTarget` — and the
/// undrained feed, whose queued lines were addressed to a character that is gone. It is the same
/// shape, and the same reasoning, as [`crate::ui_aura`]'s `end_session_aura_state`.
///
/// **Not on a reconnect**, deliberately, which is why this is not in [`channels`]'s disconnect
/// twin: channel membership is *server* state and dies with the server's `Player` object however
/// the socket ended, but a seamless same-character reconnect is benilla's own affordance —
/// the reference has no such thing, it goes to the login screen — and inside it the window keeping
/// its scrollback is the whole point.
fn end_session_chat(
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    mut log: ResMut<ChatLog>,
) {
    end_chat_session(script.map(NonSendMut::into_inner), &mut log);
}

/// [`end_session_chat`]'s body, callable without a `World` — the clear is the law, the system is
/// the wiring.
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
