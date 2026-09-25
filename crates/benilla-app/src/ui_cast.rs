//! The cast-bar feed: the wire's cast edges → FrameXML events (decision 0137 phase 1).
//!
//! The spell's packet handlers and its local cancel (`crate::spell`) queue [`CastBarEdge`]s
//! (self-casts only — the producers filter on the self guid; the channel pair is self-only *on
//! the wire*), and the drain fires the reference client's FrameScript events into the script VM
//! — `SPELLCAST_START` and family, the exact contract stock
//! `Interface\FrameXML\CastingBarFrame.xml` registers for — and pushes the VM's casting flag.
//! The spell name rides the event (resolved here from the `Spell.dbc` catalog — the script VM
//! has no spell-catalog binding, deliberately: one lookup face).
//!
//! This is a feed and nothing else. The lifecycle it reads — the pending cast, the queued
//! strike, the running channel, the auto-repeat key — is the spell's (`crate::spell`, decision
//! 2328).

use std::time::Instant;

use bevy::prelude::*;

use benilla_ui::script::{ScriptValue, UiScript};

use crate::creature_anim::Casting;
use crate::net::{GuidIndex, SelfGuid};
use crate::spell::{inflight, PendingCast, QueuedMeleeSpell};
use crate::ui_action::Spells;
use crate::ui_unit::UnitFeed;

/// One edge of our own cast's lifecycle, queued by the net bridge for the cast bar.
pub(crate) enum CastBarEdge {
    /// `SMSG_SPELL_START` for the self guid with a nonzero timer (an instant shows no bar —
    /// its GO follows at once; the reference bar would only flicker).
    Start { spell_id: u32, cast_time_ms: u32 },
    /// `SMSG_SPELL_GO` for the self guid — the cast completed (the bar fills green and fades).
    Stop,
    /// `SMSG_CAST_RESULT` failure — the bar (if shown) turns red "Failed" (the red error line
    /// rides the same packet via `CastErrors`, independently).
    Failed,
    /// `SMSG_SPELL_FAILED_OTHER` for the self guid — our in-flight cast was interrupted.
    Interrupted,
    /// `SMSG_SPELL_DELAYED` — pushback: our cast took a hit and the server extended it by
    /// `delay_ms`. The bar slides its window out (spark jumps back), it does NOT cancel.
    Delayed { delay_ms: u32 },
    /// `MSG_CHANNEL_START` (self-only on the wire) — a channel opened; the bar counts *down*,
    /// and is labelled by [`channel_start_args`]'s law rather than the spell's name.
    ChannelStart { spell_id: u32, duration_ms: u32 },
    /// `MSG_CHANNEL_UPDATE` (self-only): time left; `0` = the channel is over.
    ChannelUpdate { remaining_ms: u32 },
}

/// The net bridge's cast-bar queue (the [`crate::ui_action::CastErrors`] pattern).
#[derive(Resource, Default)]
pub(crate) struct CastBarFeed(pub(crate) Vec<CastBarEdge>);

/// `SPELLCAST_START`'s **name argument** — what the reference `CastingBarFrame` puts straight into
/// `CastingBarText:SetText(...)` for a *cast*. At `0x6e7a2d`–`0x6e7a47` the handler tests
/// `AttributesEx3 & 0x4` and pushes **the empty string `0x882748`** when set, `Name[locale]` when
/// clear — the event still fires either way, carrying its duration, so the bar is a normal
/// full-length *untitled* bar rather than a hidden one.
///
/// So two ways to come back empty:
///
/// - **`AttributesEx3 & 0x4`** ([`SpellDisplay::no_casting_bar_text`], the emulators'
///   `SPELL_ATTR_EX3_NO_CASTING_BAR_TEXT`) — the attribute that exists precisely so an internal
///   spell with no player-facing name never puts one on the bar. Three shipped rows carry it, and
///   one is *named after the bit*: **22810 "Opening - No Text"**, the opener the client casts at
///   `LockType 13` ground containers. Printing the name regardless is what put that placeholder
///   over a gathered Hyacinth Mushroom.
/// - **no catalog row** (an unknown id, or no client data) — the empty string this always
///   returned in that case.
///
/// **This is the CAST bar's law and it does not generalise to the channel bar** — which is why
/// this is `SPELLCAST_START`'s label and not a shared helper. `0x6e7a2d` is the only
/// `SpellRec+0x24 & 4` test in the whole image (censused twice), and the channel handler
/// `0x6e7550` gates on entirely different bits. That law is [`channel_start_args`]'s, next door.
fn cast_bar_label(spells: Option<&crate::ui_action::Spells>, id: u32) -> String {
    spells
        .and_then(|s| s.catalog.get(id))
        .filter(|d| !d.no_casting_bar_text())
        .map(|d| d.name.clone())
        .unwrap_or_default()
}

/// The CHANNEL bar's own law — `SpellChannelStart 0x6e7550`, whole. Returns the
/// `SPELLCAST_CHANNEL_START` argument list, or `None` where the reference fires **no event at
/// all** and the bar never appears.
///
/// The handler is 147 bytes and every one of its gates is here, in its order (decomp
/// `FUN_006e7550`; the disassembly of `0x6e7550`):
///
/// ```text
/// 6e7571  test eax,eax / jle 6e75d8      ; (1) duration <= 0            -> NO EVENT
/// 6e7579  test eax,eax / jl              ; (2) id < 0                   -> NO EVENT
/// 6e757d  cmp eax,[0xc0d78c] / jg        ; (2) id past the table        -> NO EVENT
/// 6e758e  test eax,eax / je              ; (2) no SpellRec at that slot -> NO EVENT
/// 6e7595  test ch,0x20                   ; (3) AttributesEx3 & 0x2000   -> NO EVENT
/// 6e759a  test [rec+0x1c],0x20000000     ; (4) AttributesEx bit 29 ...
/// 6e75a9  mov eax,[rec+0x1e0+locale*4]   ;     ... set   -> Name[locale]
/// 6e75bc  ecx=0x870dd0 call 0x703bf0     ;     ... clear -> GetText("CHANNELING")
/// 6e75cb  push 0x156 / call 0x703f50     ; fmt 0x8432cc "%d%s" — (duration, name)
/// ```
///
/// **Bit 29 is the reverse default of the cast bar's naming rule.** A cast bar prints the spell's
/// name unless a bit says not to; a channel bar prints the literal word "Channeling" unless a bit
/// says to name the spell. Only **9** of the 323 channeled rows in the shipped 5875 `Spell.dbc`
/// opt in (the four Fishing ranks, Cannibalize, Dream Vision, Using Control Console, and the two
/// Blood Siphons that gate (3) hides anyway) — so on a real server this reads "Channeling" for
/// Blizzard, Arcane Missiles, Mind Flay, Drain Life/Soul/Mana, Rain of Fire, Hurricane,
/// Tranquility, Evocation and First Aid alike. Ours passed the plain `Spell.dbc` name to every
/// one of them until.
///
/// The word itself is read the way the reference reads it — `FrameScript_GetText` on the
/// GlobalStrings key, i.e. the Lua global `CHANNELING`, off the player's own chain — not from a
/// table of ours, so a localized or patched install gets its own word. A missing global resolves
/// to `""` (`0x882748`, the shared empty string; `GetText` never returns NULL), and the event
/// still fires: an untitled bar, not a hidden one.
///
/// **What this does NOT gate is [`ActiveChannel`]** (and so the action button's lit ring). The
/// reference's channel mirror is `SPELLCAST+0x10` (`0xceac58`), written at the cast SEND
/// (`0x6e4f88`) and never touched by this handler — so a Blood Siphon suppressed here still
/// counts as the current channel there, and ours keeps arming it from the packet for the same
/// reason.
fn channel_start_args(
    script: &UiScript,
    spells: Option<&crate::ui_action::Spells>,
    id: u32,
    duration_ms: u32,
) -> Option<Vec<ScriptValue>> {
    if duration_ms == 0 {
        return None;
    }
    // Gates (2) and (3): no record at that id, or the record forbids a channel bar outright. The
    // `filter` is the `je`/`jne` pair — both leave with the event unfired.
    let display = spells
        .and_then(|s| s.catalog.get(id))
        .filter(|d| !d.no_channel_bar())?;
    let name = if display.channel_bar_own_name() {
        display.name.clone()
    } else {
        script
            .lua()
            .globals()
            .get::<String>("CHANNELING")
            .unwrap_or_default()
    };
    Some(vec![
        ScriptValue::Int(i64::from(duration_ms)),
        ScriptValue::Str(name),
    ])
}

/// Drain the queue into FrameScript events — the reference `CastingBarFrame` contract
/// (extracted 1.12 `CastingBarFrame.lua`): `SPELLCAST_START(name, ms)`,
/// `SPELLCAST_CHANNEL_START(ms, name)` — arg order reversed, and not a slip —
/// `SPELLCAST_CHANNEL_UPDATE(remaining_ms)`, and the argless STOP / FAILED / INTERRUPTED /
/// CHANNEL_STOP. A channel update of `0` fires CHANNEL_STOP — the server ends both the natural
/// finish and the interrupt that way.
///
/// An edge can resolve to **no event**: [`channel_start_args`] returns `None` for the three
/// conditions under which `0x6e7550` returns without firing, and the bar then never appears.
///
/// Also pushes the frame's stoppable mirror (running auto-repeat OR the [`Inflight`] slot — NOT
/// a channel, which `SpellStopCasting 0x6e6e80` answers nil for) into the
/// VM **after** [`local_self_cancel`] resolved, so the ESC chain's `SpellStopCasting()` reads
/// post-cancel truth when `UiInput` runs later this frame.
///
/// The mirror is what decides whether the press is EATEN, and that is half of decision 1049: a
/// queued strike makes `IsCasting` true in the reference, so `SpellStopCasting()` returns `1` and
/// `ToggleGameMenu`'s ladder never reaches `ClearTarget()` (`UIParent.lua` l.1489 vs l.1492).
/// Reading only the ordinary-cast half here is what dropped the target on the first Esc.
pub(crate) fn feed_cast_bar(
    script: Option<NonSendMut<UiScript>>,
    mut feed: ResMut<CastBarFeed>,
    spells: Option<Res<Spells>>,
    pending: Res<PendingCast>,
    queued_melee: Res<QueuedMeleeSpell>,
    auto_repeat: Res<crate::spell::AutoRepeatActive>,
    self_guid: Res<SelfGuid>,
    index: Res<GuidIndex>,
    casting: Query<&Casting>,
) {
    let Some(mut script) = script else {
        return;
    };
    let now = Instant::now();
    // The same slot the cancel resolves, so the mirror and the drain can never disagree about
    // whether a press had something to spend itself on — plus the auto-repeat key (`0xceac30`)
    // the ref reads first.
    let started = self_guid
        .0
        .as_ref()
        .and_then(|g| index.0.get(g))
        .and_then(|&e| casting.get(e).ok())
        .map(|c| c.spell_id);
    script.set_casting(
        auto_repeat.0.is_some() || inflight(&pending, &queued_melee, started, now).is_some(),
    );
    for edge in feed.0.drain(..) {
        let Some((event, args)): Option<(&str, Vec<ScriptValue>)> = (match edge {
            CastBarEdge::Start {
                spell_id,
                cast_time_ms,
            } => Some((
                "SPELLCAST_START",
                vec![
                    ScriptValue::Str(cast_bar_label(spells.as_deref(), spell_id)),
                    ScriptValue::Int(i64::from(cast_time_ms)),
                ],
            )),
            CastBarEdge::Stop => Some(("SPELLCAST_STOP", vec![])),
            CastBarEdge::Failed => Some(("SPELLCAST_FAILED", vec![])),
            CastBarEdge::Interrupted => Some(("SPELLCAST_INTERRUPTED", vec![])),
            CastBarEdge::Delayed { delay_ms } => Some((
                "SPELLCAST_DELAYED",
                vec![ScriptValue::Int(i64::from(delay_ms))],
            )),
            CastBarEdge::ChannelStart {
                spell_id,
                duration_ms,
            } => {
                let args = channel_start_args(&script, spells.as_deref(), spell_id, duration_ms);
                // The channel half of the cast trace. The other five edges are traced at RECV
                // (`spell::net`); these two are traced HERE because the decision that
                // wants watching — which of `0x6e7550`'s legs the spell took — is made here,
                // and the drain runs the same frame the packet lands.
                if *crate::net::CAST_TRACE {
                    match args.as_deref() {
                        Some([_, ScriptValue::Str(label)]) => info!(
                            "cast-trace: CHANNEL_START — spell {spell_id} {duration_ms}ms, \
                             bar label {label:?}"
                        ),
                        _ => info!(
                            "cast-trace: CHANNEL_START — spell {spell_id} {duration_ms}ms, \
                             NO BAR (0x6e7550 gate: zero duration, no Spell.dbc row, or \
                             AttributesEx3 & 0x2000)"
                        ),
                    }
                }
                args.map(|args| ("SPELLCAST_CHANNEL_START", args))
            }
            // `0x6e75f0`'s whole shape: nonzero re-times the window, zero ends it. It reads no
            // SpellRec, so neither leg has an attribute gate — a channel whose START was
            // suppressed still sends these, and the stock Lua's `IsShown()` guards absorb them.
            CastBarEdge::ChannelUpdate { remaining_ms } => {
                let over = remaining_ms == 0;
                if *crate::net::CAST_TRACE {
                    if over {
                        info!("cast-trace: CHANNEL_STOP — update 0, the channel is over");
                    } else {
                        info!("cast-trace: CHANNEL_UPDATE — {remaining_ms}ms left");
                    }
                }
                Some(if over {
                    ("SPELLCAST_CHANNEL_STOP", vec![])
                } else {
                    (
                        "SPELLCAST_CHANNEL_UPDATE",
                        vec![ScriptValue::Int(i64::from(remaining_ms))],
                    )
                })
            }
        }) else {
            continue;
        };
        script.fire_event(event, args);
    }
}

/// The cast-bar UI seam: the queue + its drain (before the VM ticks, so an edge and its first
/// OnUpdate land the same frame), and the local self-cancel resolved just ahead of the drain —
/// a controller move edge or an ESC `SpellStopCasting()` from the previous frame kills the bar
/// on this frame's VM tick (one engine frame from input to bar-death).
/// The feed only — the state it reads is `crate::spell`'s.
pub(crate) struct UiCastPlugin;

impl Plugin for UiCastPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CastBarFeed>().add_systems(
            Update,
            // After the spell's local cancel, whose edges this drains the same frame — the
            // `(local_self_cancel, feed_cast_bar).chain()` this plugin held until 2328.
            feed_cast_bar
                .in_set(UnitFeed)
                .after(crate::spell::LocalCancel),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **B247**: the bar's label honours `SPELL_ATTR_EX3_NO_CASTING_BAR_TEXT`.
    /// Spell 22810's Spell.dbc name — "Opening - No Text" — is Blizzard's own annotation of the
    /// attribute, and it reached a player's screen because we printed the name regardless.
    #[test]
    fn the_no_casting_bar_text_attribute_blanks_the_label() {
        use benilla_formats::{SpellCatalog, SpellDisplay};
        let named = |name: &str, attributes_ex3: u32| SpellDisplay {
            name: name.to_string(),
            attributes_ex3,
            ..Default::default()
        };
        let spells = crate::ui_action::Spells {
            catalog: SpellCatalog::from_displays(
                [
                    (6478, named("Opening", 0)),
                    (22810, named("Opening - No Text", 0x4)),
                ]
                .into_iter()
                .collect(),
            ),
            ..crate::ui_action::Spells::empty_for_tests()
        };
        assert_eq!(cast_bar_label(Some(&spells), 6478), "Opening");
        assert_eq!(
            cast_bar_label(Some(&spells), 22810),
            "",
            "the event still fires carrying its duration — a full-length UNTITLED bar, \
             not a hidden one (0x6e7a33 pushes the empty string, then fires as usual)"
        );
        // The pre-existing empty cases are untouched: an unknown id, and no client data at all.
        assert_eq!(cast_bar_label(Some(&spells), 133), "");
        assert_eq!(cast_bar_label(None, 6478), "");
    }

    /// The CHANNEL bar's law — [`channel_start_args`], the other half of `0x6e7550`, and the
    /// **opposite default** to the cast bar's above: a channel says "Channeling" unless
    /// `AttributesEx & 0x2000_0000` permits its own name. Shapes are the real shipped rows.
    #[test]
    fn the_channel_bar_says_channeling_unless_the_spell_may_name_itself() {
        use benilla_formats::{SpellCatalog, SpellDisplay};
        let row = |name: &str, attributes_ex: u32, attributes_ex3: u32| SpellDisplay {
            name: name.to_string(),
            attributes_ex,
            attributes_ex3,
            ..Default::default()
        };
        let spells = crate::ui_action::Spells {
            catalog: SpellCatalog::from_displays(
                [
                    // Blizzard rank 1 — the real 5875 columns (bit 29 clear).
                    (10, row("Blizzard", 0x1000_008c, 0)),
                    // Fishing — one of the nine that name themselves.
                    (7620, row("Fishing", 0x2100_4004, 0x0800_0000)),
                    // Blood Siphon — the whole-event suppressor, both shipped rows.
                    (24322, row("Blood Siphon", 0x2001_808c, 0x2000)),
                ]
                .into_iter()
                .collect(),
            ),
            ..crate::ui_action::Spells::empty_for_tests()
        };
        // `FrameScript_GetText` reads the Lua global, so the VM is the source of the word — the
        // chain-backed end-to-end is `ui_script::cast_tests`; here it is set by hand so the four
        // gates are exercised without client data.
        let script = UiScript::new().expect("VM");
        script.eval::<()>("CHANNELING = \"Channeling\"").unwrap();

        let name_of = |id: u32, ms: u32| -> Option<String> {
            channel_start_args(&script, Some(&spells), id, ms).map(|args| match &args[1] {
                ScriptValue::Str(s) => s.clone(),
                other => panic!("arg 2 is the name, got {other:?}"),
            })
        };
        assert_eq!(
            name_of(10, 8_000).as_deref(),
            Some("Channeling"),
            "Blizzard has AttributesEx bit 29 clear, so the bar reads the generic word"
        );
        assert_eq!(
            name_of(7620, 20_000).as_deref(),
            Some("Fishing"),
            "bit 29 set takes Name[locale] instead"
        );
        assert_eq!(
            name_of(24322, 8_000),
            None,
            "AttributesEx3 & 0x2000 fires NO event — a suppressed bar, not a blank one"
        );
        assert_eq!(name_of(10, 0), None, "0x6e7574: duration <= 0 -> no event");
        assert_eq!(
            name_of(133, 8_000),
            None,
            "0x6e7590: no SpellRec -> no event"
        );
        assert_eq!(
            channel_start_args(&script, None, 10, 8_000),
            None,
            "no client data is the same no-record leg"
        );

        // Arg ORDER is the reverse of SPELLCAST_START's, and the stock Lua reads it that way.
        assert_eq!(
            channel_start_args(&script, Some(&spells), 10, 8_000).unwrap()[0],
            ScriptValue::Int(8_000),
            "fmt 0x8432cc is \"%d%s\" — duration first"
        );
    }

    /// The word itself comes off the PLAYER's chain, not a table of ours — `FrameScript_GetText`
    /// on the GlobalStrings key, so a localized install gets its own. Pinned against the real
    /// `Interface\FrameXML\GlobalStrings.lua` (l.478 in the shipped enUS file, whose trailing
    /// comment is literally "Channeling tag for the channeling bar"). Skips without client data.
    #[test]
    fn the_channeling_word_comes_off_the_players_own_chain() {
        let _data = benilla_formats::wow_data_or_skip!();
        use benilla_formats::{SpellCatalog, SpellDisplay};
        let script = UiScript::new().expect("VM");
        crate::ui_script::test_ui::load_ui(&script, r"Interface\FrameXML\GlobalStrings.lua");
        let spells = crate::ui_action::Spells {
            catalog: SpellCatalog::from_displays(
                [(
                    10,
                    SpellDisplay {
                        name: "Blizzard".into(),
                        attributes_ex: 0x1000_008c,
                        ..Default::default()
                    },
                )]
                .into_iter()
                .collect(),
            ),
            ..crate::ui_action::Spells::empty_for_tests()
        };
        assert_eq!(
            channel_start_args(&script, Some(&spells), 10, 8_000).unwrap()[1],
            ScriptValue::Str("Channeling".into()),
        );
    }
}
