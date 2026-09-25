//! The cast-bar feed: our own cast's wire edges become the `SPELLCAST_*` events stock
//! `CastingBarFrame.lua:6-13` registers for, named from `Spell.dbc`, and the VM's casting flag
//! is pushed each frame. The cast lifecycle it reads is `crate::spell`'s.

use std::time::Instant;

use bevy::prelude::*;

use benilla_ui::script::{ScriptValue, UiScript};

use crate::creature_anim::Casting;
use crate::net::{GuidIndex, SelfGuid};
use crate::spell::{inflight, PendingCast, QueuedMeleeSpell};
use crate::ui_action::Spells;
use crate::ui_unit::UnitFeed;

/// One edge of our own cast, queued by the spell's packet handlers and its local cancel.
pub(crate) enum CastBarEdge {
    /// `SMSG_SPELL_START` with a nonzero cast time; an instant opens no bar, as in the reference.
    Start { spell_id: u32, cast_time_ms: u32 },
    /// `SMSG_SPELL_GO`: the cast completed.
    Stop,
    /// `SMSG_CAST_RESULT` failure: the bar, if shown, reads "Failed".
    Failed,
    /// `SMSG_SPELL_FAILED_OTHER`: our cast was interrupted.
    Interrupted,
    /// `SMSG_SPELL_DELAYED`: pushback extends the cast by `delay_ms`; the bar slides, never
    /// cancels.
    Delayed { delay_ms: u32 },
    /// `MSG_CHANNEL_START`, sent by vmangos to the caster only; gated by [`channel_start_args`].
    ChannelStart { spell_id: u32, duration_ms: u32 },
    /// `MSG_CHANNEL_UPDATE`, also caster-only: the time left, 0 when the channel is over.
    ChannelUpdate { remaining_ms: u32 },
}

/// The queue of [`CastBarEdge`]s, drained by [`feed_cast_bar`].
#[derive(Resource, Default)]
pub(crate) struct CastBarFeed(pub(crate) Vec<CastBarEdge>);

/// `SPELLCAST_START`'s name argument: empty for an unknown spell, or when `AttributesEx3 & 0x4`
/// is set, where the reference pushes `""` (`0x6e7a2d`) and still fires, so the bar is a
/// full-length untitled one. The channel bar names differently ([`channel_start_args`]).
fn cast_bar_label(spells: Option<&crate::ui_action::Spells>, id: u32) -> String {
    spells
        .and_then(|s| s.catalog.get(id))
        .filter(|d| !d.no_casting_bar_text())
        .map(|d| d.name.clone())
        .unwrap_or_default()
}

/// `SPELLCAST_CHANNEL_START`'s arguments, `(duration, name)` (format `0x8432cc`, "%d%s"), or
/// `None` where `SpellChannelStart` (`0x6e7550`) fires no event: a zero duration, an id with no
/// `Spell.dbc` row, or `AttributesEx3 & 0x2000`. The name is the spell's own only with
/// `AttributesEx` bit 29 set, else the Lua global `CHANNELING` off the player's chain (`""` when
/// missing, and the event still fires).
///
/// This gates the event only: the reference's channel mirror (`0xceac58`) is written at the cast
/// send (`0x6e4f88`), never here, so `ActiveChannel` still arms for a suppressed channel.
fn channel_start_args(
    script: &UiScript,
    spells: Option<&crate::ui_action::Spells>,
    id: u32,
    duration_ms: u32,
) -> Option<Vec<ScriptValue>> {
    if duration_ms == 0 {
        return None;
    }
    // No row at that id, or a row that forbids the channel bar: no event.
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

/// Drain the queue into the stock events: `SPELLCAST_START(name, ms)`,
/// `SPELLCAST_CHANNEL_START(ms, name)` (reversed, as `CastingBarFrame.lua:80-84` reads it),
/// `SPELLCAST_CHANNEL_UPDATE(ms)` and the argless rest. A channel update of 0 is
/// `SPELLCAST_CHANNEL_STOP`: vmangos ends a channel that way, finished or interrupted.
///
/// First it pushes the flag `SpellStopCasting` (`0x6e6e80`) answers from: auto-repeat or the
/// in-flight slot, never a channel, taken after the local cancel. A queued strike counts, so the
/// first Esc stops it and `ToggleGameMenu` skips `ClearTarget()` (`UIParent.lua:1489-1492`).
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
    // The auto-repeat key (`0xceac30`), which the reference reads first, then the in-flight slot
    // the cancel resolves, so the flag and the cancel agree.
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
                // Traced here, where the `0x6e7550` gate is decided; other edges trace in
                // `spell::net`.
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
            // `0x6e75f0`: nonzero re-times the bar, zero ends it, with no attribute gate; for a
            // suppressed channel the stock Lua's `IsShown()` guards absorb these.
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

/// The cast-bar feed, drained before the VM ticks so an edge and its first `OnUpdate` share a
/// frame, and after the local cancel so a move or Esc kills the bar on the next tick.
pub(crate) struct UiCastPlugin;

impl Plugin for UiCastPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CastBarFeed>().add_systems(
            Update,
            // After the spell's local cancel, whose edges this drains the same frame.
            feed_cast_bar
                .in_set(UnitFeed)
                .after(crate::spell::LocalCancel),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spell 22810, "Opening - No Text", carries `SPELL_ATTR_EX3_NO_CASTING_BAR_TEXT`.
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
        // An unknown id, and no client data at all.
        assert_eq!(cast_bar_label(Some(&spells), 133), "");
        assert_eq!(cast_bar_label(None, 6478), "");
    }

    /// The attribute columns are those of the shipped 5875 rows.
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
                    // Blizzard rank 1: bit 29 clear.
                    (10, row("Blizzard", 0x1000_008c, 0)),
                    // Fishing: bit 29 set, one of nine rows that name themselves.
                    (7620, row("Fishing", 0x2100_4004, 0x0800_0000)),
                    // Blood Siphon: `AttributesEx3 & 0x2000` suppresses the event.
                    (24322, row("Blood Siphon", 0x2001_808c, 0x2000)),
                ]
                .into_iter()
                .collect(),
            ),
            ..crate::ui_action::Spells::empty_for_tests()
        };
        // The word is the Lua global, set by hand so the gates run without client data.
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

        // Duration first, the reverse of `SPELLCAST_START`.
        assert_eq!(
            channel_start_args(&script, Some(&spells), 10, 8_000).unwrap()[0],
            ScriptValue::Int(8_000),
            "fmt 0x8432cc is \"%d%s\" — duration first"
        );
    }

    /// `CHANNELING` is `GlobalStrings.lua:479`. Skips without client data.
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
