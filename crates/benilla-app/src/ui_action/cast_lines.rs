//! A refused cast's line, its world half: the arms that need our bags, the item cache or a DBC name
//! table, and the combat-log line beside it. `ReportCastFailure 0x6e1a00` builds the text, shows it
//! (`DisplayError`, `0x6e21dd`) and logs it (`0x62c360`, `0x6e21fc`) inside the cast, so the line
//! is resolved where the report is signalled: inside the script call that raised it
//! ([`crate::script_calls`]), or at its place on the cast lane ([`crate::ui_cast`]). The text
//! layers are [`super::cast_fail`]'s.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::UiScript;

use crate::items::Items;
use crate::net::{NetCommands, ObjectStore, SelfPlayer};
use crate::query_cache::Ask;
use crate::ui_chat::combat::PendingCombat;

use super::cast_fail::{self, CastFailLine};
use super::{CastFail, Caster, Shown, Spells};

/// The DBC name tables the cast-fail argument arms read; each is absent without game data. The
/// shapeshift forms ride [`Spells`].
#[derive(SystemParam)]
pub(crate) struct FailNameTables<'w> {
    /// `SpellFocusObject.dbc`, for `0x5e`.
    focus: Option<Res<'w, crate::ui_tradeskill::SpellFocus>>,
    /// `AreaTable.dbc`, for `0x5d`.
    areas: Option<Res<'w, crate::area::AreaTableRes>>,
    /// `SpellMechanic.dbc`, for `0x8d`.
    mechanics: Option<Res<'w, super::SpellMechanics>>,
}

impl FailNameTables<'_> {
    fn args<'a>(&'a self, spells: Option<&'a Spells>) -> cast_fail::FailArgs<'a> {
        cast_fail::FailArgs {
            arg: None,
            focus: self.focus.as_deref().map(|f| &f.catalog),
            areas: self.areas.as_deref().map(|a| &a.0),
            mechanics: self.mechanics.as_deref().map(|m| &m.catalog),
            forms: spells.map(|s| &s.forms),
            found: None,
            redisplay: false,
            power: None,
        }
    }
}

/// One resolved refusal: the line, and the combat-log line the reporter writes after it.
pub(crate) struct ResolvedFail {
    pub(crate) shown: Shown,
    pub(crate) log: Option<PendingCombat>,
}

/// What a refused cast's line reads.
#[derive(SystemParam)]
pub(crate) struct CastFailLines<'w, 's> {
    spells: Option<Res<'w, Spells>>,
    sub_classes: Option<Res<'w, crate::ui_items::ItemSubClasses>>,
    names: FailNameTables<'w>,
    self_q: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
    items: Res<'w, Items>,
    commands: Res<'w, NetCommands>,
}

impl CastFailLines<'_, '_> {
    /// The spell catalog, which the cast bar's labels read too.
    pub(crate) fn spells(&self) -> Option<&Spells> {
        self.spells.as_deref()
    }

    /// `fail`'s line and its log line, `None` where the reference leaves before both now; an empty
    /// line shows nothing and still logs. A line whose item record is not cached waits on the
    /// record's answer ([`crate::items::RecordCallback::CastFailure`]).
    pub(crate) fn resolve(&mut self, script: &UiScript, fail: CastFail) -> Option<ResolvedFail> {
        let line = self.line(script, fail)?;
        // The reason, its wire word and both resolved buffers, so a declined arm is readable
        // from a probe run.
        debug!(
            "ui_action: cast fail — {:?} spell {} reason {:#04x} arg {:?} → {line:?}",
            fail.caster, fail.spell_id, fail.reason, fail.arg
        );
        let log = self.log_line(&line, fail);
        Some(ResolvedFail {
            shown: Shown::keyed(line.key, line.text),
            log,
        })
    }

    /// The displayed line first, the log reads from it: the reference's order.
    fn line(&mut self, script: &UiScript, fail: CastFail) -> Option<CastFailLine> {
        let CastFail {
            spell_id,
            reason,
            caster,
            ..
        } = fail;
        let pet = caster == Caster::Pet;
        let spells = self.spells.as_deref();
        let d = spells.and_then(|s| s.catalog.get(spell_id));
        let get = |key: &str| script.lua().globals().get::<String>(key).ok();
        let subs = self.sub_classes.as_deref();
        // The world arms' names; [`cast_fail::cast_fail_text`] fills the text and picks the id.
        let found = match reason {
            // `0x19`-`0x1b` (`0x6e1db7`, through `0x6e2380`): the singular subclass name, "Must
            // have a Wand equipped"; a multi-bit mask names its `ItemSubClassMask.dbc` group, else
            // the first subclass, where the tooltip (`0x52eea7`) uses the plural and joins. The
            // pet's table fills it too (`0x6e904d`).
            0x19..=0x1b => d.zip(subs).and_then(|(d, subs)| {
                (d.equipped_item_class >= 0)
                    .then(|| {
                        subs.0.requirement_display_name(
                            d.equipped_item_class as u32,
                            d.equipped_item_subclass_mask,
                        )
                    })
                    .flatten()
            }),
            // `0x31` (`0x6e1e54`): the same helper for class 6 (Projectile), mask `1 << arg`, the
            // shift masked to five bits as x86 `shl` masks it (`0x6e1e5c`). The player's arm only:
            // the pet's table (`0x6e93d0`) has no `0x31` entry. vmangos never sends the word
            // (`Spell.cpp:4419-4443`), so against it this declines to the stem.
            0x31 if !pet => fail.arg.zip(subs).and_then(|(arg, subs)| {
                const ITEM_CLASS_PROJECTILE: u32 = 6;
                subs.0
                    .requirement_display_name(ITEM_CLASS_PROJECTILE, 1u32 << (arg & 31))
            }),
            // `0x78` and `0x5c` (`0x6e1e7f`), the player's arms only (the pet's table sends both to
            // its generic arm): the name of argument 1's item, the entry `0x6e4000` found, read
            // with no spell record. The server's refusal carries no word for either (vmangos
            // `Spell.cpp:4407-4431`), which reads -1 (`0x6e736e`): its lookup asks for an entry
            // whose refusal no node matches (`0x55bdf7`, `0x55be5d`), so neither line nor log ever
            // shows; a word of 0 asks nothing and shows nothing (`0x55ba42`).
            0x78 | 0x5c if !pet => {
                let failing = fail.arg.filter(|&entry| entry != 0)?;
                // The callback `0x6e29b0` looks the record up again with no callback of its own
                // (`0x6e2a27`), a refusal's finding none: the literal `UNKNOWN` (`0x6e2a49`).
                if fail.redisplay {
                    let cached = self.items.template(failing, 0, Ask::Read);
                    Some(cached.map_or_else(|| "UNKNOWN".to_string(), |i| i.name.clone()))
                } else {
                    // The failure handler's lookup with the callback `0x6e29b0` and the reason, no
                    // dedupe (`0x6e1e82`-`0x6e1ea4`): a miss leaves past the line and its log
                    // (`0x6e1eab`), and the record's answer shows it ([`crate::ui_cast`]).
                    let cached = self.items.template(failing, 0, Ask::Event(&self.commands));
                    match cached {
                        Some(info) => Some(info.name.clone()),
                        None => {
                            self.items.wait(
                                failing,
                                crate::items::RecordCallback::CastFailure {
                                    spell: spell_id,
                                    reason,
                                },
                            );
                            return None;
                        }
                    }
                }
            }
            _ => None,
        };
        cast_fail::cast_fail_text(
            caster,
            reason,
            d,
            cast_fail::FailArgs {
                arg: fail.arg,
                found: found.as_deref(),
                redisplay: fail.redisplay,
                power: self.self_q.iter().next().map(|s| s.0.unit_power_type()),
                ..self.names.args(spells)
            },
            &get,
        )
    }

    /// The combat-log line, after the display as in the reference (`DisplayError` at `0x6e21dd`,
    /// the log formatter `0x62c360` at `0x6e21fc`), from the argText buffer
    /// ([`CastFailLine::logged`]). Nothing is logged for a redisplay (`0x6e29b0` skips the log)
    /// or for the pet (`0x6e8eb0` never calls `0x62c360`). `SMSG_CAST_RESULT` goes to the caster
    /// alone, so the `OTHER` keys stay unused.
    fn log_line(&self, line: &CastFailLine, fail: CastFail) -> Option<PendingCombat> {
        if fail.caster == Caster::Pet || fail.redisplay {
            return None;
        }
        // `0x62c360` writes nothing for an id out of range or with no record, for `Attributes &
        // 0x180` (`0x62c4bd`) or for an empty name (`0x62c4d7`).
        let display = crate::ui_chat::combat::logged_spell(self.spells.as_deref(), fail.spell_id)?;
        // `0x62aff0`: `Attributes` bit 4 marks an ability, which "performs", not "casts".
        const ATTR_IS_ABILITY: u32 = 0x10;
        let family = if display.attributes & ATTR_IS_ABILITY != 0 {
            crate::ui_chat::combat::SPELLFAILPERFORM
        } else {
            crate::ui_chat::combat::SPELLFAILCAST
        };
        Some(PendingCombat {
            kind: crate::ui_chat::ChatEventKind::SpellFailedLocalPlayer,
            family,
            variant: crate::ui_chat::combat::Variant::SelfOther,
            subject: 0,
            object: 0,
            fills: crate::ui_chat::combat::Fills {
                spell: display.name.clone(),
                named: line.logged().to_string(),
                ..Default::default()
            },
            named: crate::ui_chat::combat::Named::Ready,
            tries: 0,
            // `0x62c360` raises no `COMBAT_TEXT_UPDATE`: it writes its line (`0x62c5b3`) and
            // returns.
            word: None,
        })
    }
}
