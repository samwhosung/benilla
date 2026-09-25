//! The window model + router + composer: [`ChatWindows`] holds each docked
//! window's message-group registration (the ref client's own chat-cache defaults, quoted from the
//! pin's `WTF/.../chat-cache.txt`); [`route`] fans one [`ChatEvent`] across every subscribed
//! window; [`compose`] is the `ChatFrame_OnEvent` composition law transcribed — the `CHAT_*_GET`
//! patterns, the `<AFK>/<DND>/<GM>` flag prefix, the `|Hplayer:…|h[Name]|h` link (never on EMOTE
//! or monster lines), the `[Language]` header, the `[N. Name]` channel prefix with its " - Zone"
//! tail stripped (the SPEECH branch only — a notice prints arg4 whole, 1275), and the
//! `CHAT_<X>_NOTICE` channel-notice strings. **Every one of those formats is a KEY, resolved from
//! the player's own `GlobalStrings.lua` at render time** — and resolved the way
//! `ChatFrame_OnEvent` itself does it, by splicing the type into `CHAT_<TYPE>_GET` and the notice
//! token into `CHAT_<TOKEN>_NOTICE` rather than by carrying a table of English. Colors come from
//! the stock window's `ChatTypeInfo` table (1948), whose shipped defaults are transcribed in
//! [`super::event::default_color`].

use bevy::prelude::*;

use benilla_ui::strings::{fill, Arg};

use super::event::{event_name, notice_token, ChatEvent, ChatEventKind};

/// What the app keeps beside the reference's chat frames: the default language its composer
/// needs for the log-file line, and the log files themselves. The per-window registration that
/// lived here is the record's MESSAGES set now, read by the reference's own
/// `ChatFrame_RegisterForMessages`.
#[derive(Resource, Default)]
pub(crate) struct ChatWindows {
    /// The frame's own `this.defaultLanguage` — the name `GetDefaultLanguage()` answers, which is
    /// the **faction** tongue (Common for every Alliance race, Orcish for every Horde one).
    ///
    /// It lives here rather than being derived per line because that is where the reference keeps
    /// it: `ChatFrame.lua` stores it on the frame and the language-header test reads it from there
    /// ([`compose`]). Empty until the self descriptors and `Languages.dbc` are both up, which
    /// suppresses no header the reference would show — an empty default only ever makes the test
    /// *more* likely to print one.
    pub default_language: String,
    /// `LoggingChat`/`LoggingCombat`'s files ([`super::logging`]) — here because every
    /// rendered line passes [`route`], which is the one place to tee them.
    pub logs: super::logging::ChatLogFiles,
}

/// Route one event: fire the real `CHAT_MSG_*` at the VM — the reference's own `ChatFrame_OnEvent`
/// composes and prints it, in every window whose MESSAGES set carries the type, with
/// `ChatTypeInfo`'s colour, the whisper chime and the tab flash — and tee the
/// rendered line to the log files. A kind-less event (an unmodeled wire type) drops with a warn,
/// never silently.
///
/// The composer that used to print here survives for the log line only: `LoggingChat`'s file
/// wants the text the window shows, and the reference writes it C-side, not from Lua.
pub(crate) fn route(
    script: &mut benilla_ui::script::UiScript,
    windows: &mut ChatWindows,
    event: &ChatEvent,
) {
    let Some(kind) = event.kind else {
        warn!("chat: unroutable event (no kind): {:?}", event.text);
        return;
    };
    // The window shows what the reference's `ChatFrame_OnEvent` prints from the event — its
    // own composition, `ChatTypeInfo`'s colours, the per-window registration
    // (`ChatFrame_RegisterForMessages` over the record's MESSAGES set), the tell chime and the
    // tab flash. The app's transcription of that composition survives only for the log files,
    // which want the rendered line the window will show.
    let default_language = windows.default_language.clone();
    // The composer's strings are the VM's own — `getglobal` against the `GlobalStrings.lua` the
    // reference's Lua reads, so the log line and the window line cannot say different things.
    if let Some(line) = compose(event, kind, &default_language, &|key| {
        script.lua().globals().get::<String>(key).ok()
    }) {
        windows.logs.record(kind.is_combat_log(), &line);
    }
    script.fire_event(event_name(kind), event.script_args());
}

/// `ChatFrame_OnEvent`'s composition, transcribed (ref ChatFrame.lua l.1369-1468 + the quoted
/// GlobalStrings). Returns `None` for a notice the 1.12 UI renders silently (MODE_CHANGE).
pub(crate) fn compose(
    event: &ChatEvent,
    kind: ChatEventKind,
    default_language: &str,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    use ChatEventKind as K;
    Some(match kind {
        // Verbatim families (l.1395-1402): the text IS the line. COMBAT_XP_GAIN rides the same
        // default handler tail (client-composed, no sender) — ref l.1425's fall-through.
        K::System
        | K::TextEmote
        | K::Skill
        | K::Loot
        | K::Money
        | K::CombatXpGain
        | K::CombatHonorGain
        | K::BgSystemNeutral
        | K::BgSystemAlliance
        | K::BgSystemHorde => event.text.clone(),
        // The whole combat-log block is verbatim too, and the reference says so by PREFIX rather
        // than by name: l.1397-1400 is two arms, `strsub(type,1,7) == "COMBAT_"` and
        // `strsub(type,1,6) == "SPELL_"`, each doing nothing but `AddMessage(arg1, …)`. The
        // sentence was already built by the time it became an event — that is what
        // [`super::combat`] is — so there is nothing left for the composer to do.
        k if k.is_combat_log() => event.text.clone(),
        // `format(TEXT(CHAT_IGNORED), arg2)` (l.1404). **`CHAT_IGNORED`, not
        // `ERR_IGNORING_YOU_S`** — both read "%s is ignoring you." in enUS and only the key says
        // which one the client actually shows here (2045's "assert the identifier, not the
        // sentence"); a locale that words them apart is where the difference surfaces.
        K::Ignored => fill(
            &get("CHAT_IGNORED").unwrap_or_default(),
            &[Arg::S(&event.sender)],
        ),
        // `format(CHAT_CHANNEL_LIST_GET .. arg1, arg4)` (l.1409) — the member list IS the message
        // and the channel fills the hole, with arg4 WHOLE: see [`strip_zone`] for why only the
        // speech branch runs the gsub. The reference does not escape arg1 in this arm.
        K::ChannelList => fill(
            &format!(
                "{}{}",
                get("CHAT_CHANNEL_LIST_GET").unwrap_or_default(),
                event.text
            ),
            &[Arg::S(&event.channel)],
        ),
        K::ChannelNotice | K::ChannelNoticeUser => {
            return compose_notice(event, kind, get);
        }
        // Everything else is the player/monster-line branch (l.1425-1467).
        _ => {
            // `pflag = TEXT(getglobal("CHAT_FLAG_"..arg6))` (l.1430) — `CHAT_FLAG_AFK`/`_DND`/
            // `_GM`, spliced from the flag the wire sent rather than matched against three
            // hardcoded angle-bracket tokens. An empty flag looks up nothing, as it does there.
            let pflag = if event.flag.is_empty() {
                String::new()
            } else {
                get(&format!("CHAT_FLAG_{}", event.flag)).unwrap_or_default()
            };
            let monster = matches!(
                kind,
                K::MonsterSay
                    | K::MonsterYell
                    | K::MonsterEmote
                    | K::MonsterWhisper
                    | K::RaidBossEmote
            );
            // The sender as rendered: hyperlinked `[Name]` for player lines (l.1451), bare for
            // monsters + RAID_BOSS_EMOTE (l.1437-1438) and for EMOTE (l.1450's `type ~= "EMOTE"`).
            let named = if event.sender.is_empty() {
                String::new()
            } else if monster || kind == K::Emote {
                format!("{pflag}{}", event.sender)
            } else {
                format!("{pflag}|Hplayer:{0}|h[{0}]|h", event.sender)
            };
            // The language header (l.1442-1448): non-empty, non-Universal (mapped to "" by the
            // bridge), and not our own default tongue.
            //
            // **That last clause used to read `!= "Common"`** — the comment was already right and
            // the code was not, which cost every Horde character a `[Orcish]` tag on ordinary
            // faction chat and stripped the tag from Common. The reference's test is
            // `arg3 ~= this.defaultLanguage` and `GetDefaultLanguage()` answers the **faction**
            // tongue, so it reads Orcish for a Horde body (`0x5ec890`; the tag is FrameXML's and
            // its condition is about the *default* language, never about whether the language is
            // understood — a character who knows both Common and Dwarvish still sees `[Dwarvish]`
            // on a line they read perfectly).
            //
            // The `~= "Universal"` clause of the reference's condition is deliberately absent:
            // "Universal" is in neither `Languages.dbc` nor `WoW.exe` nor `GlobalStrings.lua`, so
            // it is vestigial in 1.12 and the empty-string test is what actually suppresses
            // language 0 (see [`super::event::ChatEvent`]'s arg3 note).
            let header = if !event.language.is_empty() && event.language != default_language {
                format!("[{}] ", event.language)
            } else {
                String::new()
            };
            // `arg1 = gsub(arg1, "%%", "%%%%")` (l.1436), then
            // `format(CHAT_<TYPE>_GET .. languageHeader .. arg1, name)` — ONE substitution over
            // the pattern and the body concatenated, which is why the body has to be escaped
            // first: a `%s` a player typed must not eat the name. The monster family and
            // RAID_BOSS_EMOTE are the reference's deliberate exception (l.1434-1437): their
            // `%s` IS the name's hole (`CHAT_MONSTER_EMOTE_GET = ""`), so the escape is skipped
            // and the fill reaches into the text.
            let text = if monster {
                event.text.clone()
            } else {
                event.text.replace('%', "%%")
            };
            let body = fill(
                &format!("{}{header}{text}", get_pattern(kind, get)),
                &[Arg::S(&named)],
            );
            // The channel prefix (l.1462-1466): arg4 with its " - Zone" tail stripped,
            // bracketed. arg4 arrives already numbered ("2. Trade - City") once the channel
            // wiring (P6) assigns numbers.
            if !event.channel.is_empty() {
                format!("[{}] {body}", strip_zone(&event.channel))
            } else {
                body
            }
        }
    })
}

/// The `CHAT_<TYPE>_GET` prefix pattern for a kind — `getglobal("CHAT_"..type.."_GET")`, the
/// reference's own splice (l.1445-1453), where `type` is the event name minus its `CHAT_MSG_`
/// prefix. So this is a *derivation*, not a table: every kind the client fires has such a key by
/// construction, and the ones with nothing to prefix ship as `""` (the whole combat/spell family,
/// `CHAT_MONSTER_EMOTE_GET`, `CHAT_RAID_BOSS_EMOTE_GET`) rather than being absent.
///
/// A key the chain has no string for resolves to `""` — the same nothing an unprefixed line gets,
/// and the reference's own data-suppression posture.
fn get_pattern(kind: ChatEventKind, get: &dyn Fn(&str) -> Option<String>) -> String {
    let ty = event_name(kind)
        .strip_prefix("CHAT_MSG_")
        .unwrap_or_default();
    get(&format!("CHAT_{ty}_GET")).unwrap_or_default()
}

/// Strip the zone tail from a channel display name (`gsub(arg4, "%s%-%s.*", "")` —
/// "General - Elwynn Forest" → "General", "2. Trade - City" → "2. Trade").
///
/// **The speech branch is the ONLY caller, and that is the reference's own shape** (1275): the
/// gsub sits at l.1463, inside the `else` arm that builds a player/monster line, *after* every
/// notice arm has already returned. CHANNEL_NOTICE (l.1424), CHANNEL_NOTICE_USER (l.1416/1418)
/// and CHANNEL_LIST (l.1409) each pass **arg4 whole** into their format — so the real client's
/// join line reads "Joined Channel: [1. General - Elwynn Forest]" while a line spoken in that same
/// channel is prefixed "[1. General]". We stripped in all four and lost the tail from three.
fn strip_zone(channel: &str) -> &str {
    match channel.find(" - ") {
        Some(i) => &channel[..i],
        None => channel,
    }
}

/// The `SMSG_CHANNEL_NOTIFY` → chat line law: the notice byte becomes a **token**
/// ([`notice_token`], the client's own `0x49c60c` jump table) and the token names the string —
/// `getglobal("CHAT_"..arg1.."_NOTICE")`, l.1416/1424. `None` = the 1.12 UI shows nothing for this
/// notice: MODE_CHANGE has no token and no NOTICE string (flag-change chatter is silent), and
/// neither does a byte past the table.
///
/// **The argument list is the reference's, and its order never varies with the notice.**
/// CHANNEL_NOTICE passes arg4 alone; CHANNEL_NOTICE_USER passes arg4 then arg2, plus arg5 when the
/// packet carried a second name ("X kicked by Y"). One line reads the other way round —
/// `CHAT_INVITE_NOTICE = "%2$s has invited you to join the channel '%1$s'."` — and it gets there
/// with **positional specifiers in the string**, not with a special case at the call site. That is
/// the whole argument for resolving these by key: hand-typing the English silently hardcodes one
/// locale's word order.
///
/// `chan` is arg4 **whole**, zone tail and all — see [`strip_zone`] for why the notice arms are
/// not the gsub's callers.
pub(crate) fn compose_notice(
    event: &ChatEvent,
    kind: ChatEventKind,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let token = notice_token(event.notice_byte()?, event.slot_state)?;
    let template = get(&format!("CHAT_{token}_NOTICE"))?;
    let mut args = vec![Arg::S(event.channel.as_str())];
    if kind == ChatEventKind::ChannelNoticeUser {
        args.push(Arg::S(&event.sender));
        if !event.target.is_empty() {
            args.push(Arg::S(&event.target));
        }
    }
    let line = fill(&template, &args);
    (!line.is_empty()).then_some(line)
}
